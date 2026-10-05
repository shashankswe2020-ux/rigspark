//! Curated, offline catalog of local image/video generation models served by ComfyUI.
//!
//! Kept separate from the LLM catalog so LLM ranking, signing, and refresh are unaffected.
use crate::{
    catalog::{
        LICENSES, coordinates, date, digest, model_file, parse_document, require, timestamp,
    },
    reports::{strip_control, table},
    sizing::{HEADROOM, Hardware, ValidationError, memory_capacity},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GenerationKind {
    Image,
    Video,
}
impl GenerationKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            _ => None,
        }
    }
}

/// Built-in ComfyUI workflow graphs implemented by `rigspark-runtime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Workflow {
    FluxCheckpoint,
    WanT2v,
}
impl Workflow {
    pub fn kind(self) -> GenerationKind {
        match self {
            Self::FluxCheckpoint => GenerationKind::Image,
            Self::WanT2v => GenerationKind::Video,
        }
    }
    /// Exact file roles the workflow loads, in a stable order.
    pub fn roles(self) -> &'static [FileRole] {
        match self {
            Self::FluxCheckpoint => &[FileRole::Checkpoint],
            Self::WanT2v => &[FileRole::Diffusion, FileRole::TextEncoder, FileRole::Vae],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileRole {
    Checkpoint,
    Diffusion,
    TextEncoder,
    Vae,
}
impl FileRole {
    /// ComfyUI `models/` subfolder the loader node reads this role from.
    pub fn folder(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoints",
            Self::Diffusion => "diffusion_models",
            Self::TextEncoder => "text_encoders",
            Self::Vae => "vae",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationFile {
    pub role: FileRole,
    pub folder: String,
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationModel {
    pub id: String,
    pub family: String,
    pub kind: GenerationKind,
    pub params: String,
    pub license: String,
    pub open_weight: bool,
    pub release_date: String,
    #[serde(default)]
    pub default: bool,
    pub source: String,
    pub workflow: Workflow,
    pub workflow_source: String,
    pub files: Vec<GenerationFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationCatalog {
    pub schema_version: u8,
    pub generated_at: String,
    pub models: Vec<GenerationModel>,
}

fn https(value: &str) -> Result<(), ValidationError> {
    require(
        value.len() <= 2048
            && value.starts_with("https://")
            && value.len() > "https://".len()
            && !value
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace()),
        "source links must be HTTPS",
    )
}
fn model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:-".contains(&byte)
        })
        && GenerationKind::parse(value).is_none()
}

impl GenerationModel {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }
    pub fn largest_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).max().unwrap_or(0)
    }
    pub fn file(&self, role: FileRole) -> Option<&GenerationFile> {
        self.files.iter().find(|file| file.role == role)
    }
    fn validate(&self) -> Result<(), ValidationError> {
        require(model_id(&self.id), "invalid generation model id")?;
        require(
            !self.family.is_empty() && self.family.len() <= 128,
            "empty text field",
        )?;
        require(
            regex::Regex::new(r"^\d+(\.\d+)?[BMT]$").is_ok_and(|re| re.is_match(&self.params)),
            "invalid parameter label",
        )?;
        require(
            self.open_weight && LICENSES.contains(&self.license.as_str()),
            "generation catalog requires an allowlisted open-weight license",
        )?;
        date(&self.release_date)?;
        https(&self.source)?;
        https(&self.workflow_source)?;
        let roles: Vec<_> = self.files.iter().map(|file| file.role).collect();
        require(
            roles.len() == self.workflow.roles().len()
                && self
                    .workflow
                    .roles()
                    .iter()
                    .all(|role| roles.contains(role)),
            "files must match the workflow roles exactly",
        )?;
        require(
            self.workflow.kind() == self.kind,
            "workflow does not produce this model kind",
        )?;
        let mut total = 0u64;
        for file in &self.files {
            require(
                file.folder == file.role.folder(),
                "file folder does not match its role",
            )?;
            coordinates(&file.repo, &file.revision)?;
            model_file(&file.file, 512)?;
            require(
                file.file.ends_with(".safetensors"),
                "generation weights must be .safetensors files",
            )?;
            digest(&file.sha256)?;
            require(
                file.bytes > 0 && file.bytes <= MAX_SAFE_INTEGER,
                "invalid weight file size",
            )?;
            total = total
                .checked_add(file.bytes)
                .filter(|total| *total <= MAX_SAFE_INTEGER)
                .ok_or_else(|| ValidationError("invalid weight file size".into()))?;
        }
        Ok(())
    }
}

impl GenerationCatalog {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let mut result: Self = parse_document(raw)?;
        require(
            result.schema_version == 1 && !result.models.is_empty() && result.models.len() <= 256,
            "unsupported or empty generation catalog",
        )?;
        timestamp(&result.generated_at)?;
        let mut ids = HashSet::new();
        let mut defaults = HashSet::new();
        let mut kinds = HashSet::new();
        for model in &result.models {
            model.validate()?;
            require(ids.insert(&model.id), "duplicate generation model id")?;
            kinds.insert(model.kind);
            if model.default {
                require(
                    defaults.insert(model.kind),
                    "each kind needs exactly one default model",
                )?;
            }
        }
        require(
            kinds == defaults,
            "each kind needs exactly one default model",
        )?;
        for model in &mut result.models {
            model.family = strip_control(&model.family);
        }
        Ok(result)
    }
    pub fn bundled() -> Result<Self, ValidationError> {
        Self::parse(crate::GENERATION_JSON)
    }
    /// Resolves `image`/`video` to that kind's default model, otherwise an exact id.
    pub fn resolve(&self, query: &str) -> Result<&GenerationModel, ValidationError> {
        let found = match GenerationKind::parse(query) {
            Some(kind) => self
                .models
                .iter()
                .find(|model| model.kind == kind && model.default),
            None => self.models.iter().find(|model| model.id == query),
        };
        found.ok_or_else(|| {
            ValidationError(
                "unknown generation model; use image, video, or an id from `rigspark catalog --generation`"
                    .into(),
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FitVerdict {
    Yes,
    Slow,
    No,
}
impl FitVerdict {
    pub fn name(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::Slow => "slow",
            Self::No => "no",
        }
    }
}

/// Memory-only fit of a model's weights; generation speed is never estimated.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationFit {
    pub verdict: FitVerdict,
    pub reason: &'static str,
    pub memory_kind: &'static str,
    pub usable_bytes: f64,
    pub budget_bytes: f64,
    pub total_bytes: u64,
    pub largest_bytes: u64,
    /// Always `"unknown"`: no sourced generation throughput dataset exists.
    pub speed: &'static str,
}

fn system_ram_budget(hardware: &Hardware) -> f64 {
    let mut cpu = hardware.clone();
    cpu.gpu.clear();
    cpu.unified_memory = Some(false);
    memory_capacity(&cpu).1 * (1.0 - HEADROOM)
}

pub fn fit(model: &GenerationModel, hardware: &Hardware) -> GenerationFit {
    let (memory_kind, usable_bytes) = memory_capacity(hardware);
    let budget_bytes = usable_bytes * (1.0 - HEADROOM);
    let total_bytes = model.total_bytes();
    let largest_bytes = model.largest_bytes();
    let (verdict, reason) = if total_bytes as f64 <= budget_bytes {
        (FitVerdict::Yes, "all weights fit in memory together")
    } else if largest_bytes as f64 <= budget_bytes {
        (
            FitVerdict::Slow,
            "weights fit one stage at a time; ComfyUI swaps models between stages",
        )
    } else if memory_kind == "vram" && largest_bytes as f64 <= system_ram_budget(hardware) {
        (
            FitVerdict::Slow,
            "largest weight file exceeds VRAM; ComfyUI offloads to system RAM",
        )
    } else {
        (FitVerdict::No, "largest weight file exceeds usable memory")
    };
    GenerationFit {
        verdict,
        reason,
        memory_kind,
        usable_bytes,
        budget_bytes,
        total_bytes,
        largest_bytes,
        speed: "unknown",
    }
}

pub fn catalog_text(catalog: &GenerationCatalog, hardware: &Hardware) -> String {
    let mut models: Vec<_> = catalog.models.iter().collect();
    models.sort_by(|left, right| {
        left.kind
            .name()
            .cmp(right.kind.name())
            .then_with(|| right.release_date.cmp(&left.release_date))
            .then_with(|| left.id.cmp(&right.id))
    });
    let rows = models
        .into_iter()
        .map(|model| {
            let fit = fit(model, hardware);
            vec![
                model.id.clone(),
                model.kind.name().into(),
                model.params.clone(),
                format!("{:.1}", model.total_bytes() as f64 / 1073741824.0),
                fit.verdict.name().into(),
                fit.speed.into(),
                model.license.clone(),
                model.release_date.clone(),
            ]
        })
        .collect();
    format!(
        "Generation catalog (runtime: ComfyUI, local only; models: {})\n{}\n\
         Fit covers weight memory only; generation speed is unknown (no sourced data).\n\
         Run: rigspark generate image --prompt \"...\" --comfyui-dir <ComfyUI directory>\n",
        catalog.models.len(),
        table(
            &[
                ("Model", false),
                ("Kind", false),
                ("Params", false),
                ("Weights GiB", true),
                ("Fit", false),
                ("Speed", false),
                ("License", false),
                ("Release", false),
            ],
            rows,
        )
    )
}
