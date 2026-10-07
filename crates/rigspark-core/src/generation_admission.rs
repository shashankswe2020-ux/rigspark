//! Pure parsers and builders for admitting Comfy-Org image and video releases as
//! fit-only generation entries. Network access lives in `rigspark-runtime`.

use crate::{
    catalog::{EntryProvenance, LICENSES, require},
    generation::{FileRole, GenerationFile, GenerationKind, GenerationModel},
    sizing::ValidationError,
};
use serde::Deserialize;
use serde_json::Value;

pub const PUBLISHER: &str = "Comfy-Org";
pub const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SAFETENSORS_HEADER_BYTES: u64 = 100 * 1024 * 1024;

/// Repositories from `huggingface.co/api/models?author=Comfy-Org&sort=createdAt`, newest first.
pub fn parse_listing(raw: &str) -> Result<Vec<String>, ValidationError> {
    require(
        raw.len() <= MAX_METADATA_BYTES,
        "model listing exceeds 4 MiB",
    )?;
    #[derive(Deserialize)]
    struct Item {
        id: String,
    }
    let items: Vec<Item> =
        serde_json::from_str(raw).map_err(|error| ValidationError(error.to_string()))?;
    require(items.len() <= 1000, "model listing has too many entries")?;
    Ok(items
        .into_iter()
        .map(|item| item.id)
        .filter(|id| repository(id) && id.starts_with(&format!("{PUBLISHER}/")))
        .collect())
}

fn repository(id: &str) -> bool {
    let mut parts = id.split('/');
    let valid = |part: &str| {
        !part.is_empty()
            && part.len() <= 96
            && part.as_bytes()[0].is_ascii_alphanumeric()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(name), None) if valid(owner) && valid(name))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Lfs {
    pub sha256: String,
    pub size: u64,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Sibling {
    pub rfilename: String,
    #[serde(default)]
    pub lfs: Option<Lfs>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct HfModel {
    pub id: String,
    pub sha: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub gated: Value,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub siblings: Vec<Sibling>,
    #[serde(default)]
    pub pipeline_tag: Option<String>,
}
impl HfModel {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        require(
            raw.len() <= MAX_METADATA_BYTES,
            "model metadata exceeds 4 MiB",
        )?;
        let model: Self =
            serde_json::from_str(raw).map_err(|error| ValidationError(error.to_string()))?;
        require(repository(&model.id), "invalid repository id")?;
        require(
            model.siblings.len() <= 10_000 && model.tags.len() <= 1_000,
            "model metadata has too many entries",
        )?;
        Ok(model)
    }
    /// Pinned revisions only: a 40-hex commit, not gated and not private.
    pub fn pinned(&self) -> bool {
        self.sha.len() == 40
            && self.sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !self.private
            && matches!(self.gated, Value::Null | Value::Bool(false))
    }
    pub fn base_model(&self) -> Option<String> {
        self.tags
            .iter()
            .filter_map(|tag| tag.strip_prefix("base_model:"))
            .find(|value| repository(value))
            .map(str::to_string)
    }
}

pub fn license_from_tags(tags: &[String]) -> Option<&'static str> {
    let mut licenses = tags.iter().filter_map(|tag| tag.strip_prefix("license:"));
    let license = licenses.next()?;
    if licenses.next().is_some() {
        return None;
    }
    LICENSES.iter().copied().find(|known| *known == license)
}

/// Text-prompted generation only; editing, audio, 3D and depth pipelines are not admitted.
pub fn kind_from_pipeline(pipeline: Option<&str>) -> Option<GenerationKind> {
    match pipeline? {
        "text-to-image" => Some(GenerationKind::Image),
        "text-to-video" => Some(GenerationKind::Video),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetensorsError {
    /// More header bytes are needed than were read.
    Truncated(u64),
    Invalid(&'static str),
}

/// Exact parameter count from a safetensors header (8-byte length + JSON tensor table).
pub fn safetensors_params(bytes: &[u8]) -> Result<u64, SafetensorsError> {
    let length = u64::from_le_bytes(
        bytes
            .get(..8)
            .ok_or(SafetensorsError::Truncated(8))?
            .try_into()
            .expect("eight bytes"),
    );
    if length == 0 || length > MAX_SAFETENSORS_HEADER_BYTES {
        return Err(SafetensorsError::Invalid(
            "implausible safetensors header length",
        ));
    }
    let end = 8 + length as usize;
    let header = bytes
        .get(8..end)
        .ok_or(SafetensorsError::Truncated(end as u64))?;
    let table: serde_json::Map<String, Value> = serde_json::from_slice(header)
        .map_err(|_| SafetensorsError::Invalid("safetensors header is not a JSON object"))?;
    let mut total: u64 = 0;
    for (name, tensor) in table {
        if name == "__metadata__" {
            continue;
        }
        let shape = tensor["shape"]
            .as_array()
            .ok_or(SafetensorsError::Invalid("tensor without shape"))?;
        let mut elements: u64 = 1;
        for dim in shape {
            elements = elements
                .checked_mul(
                    dim.as_u64()
                        .ok_or(SafetensorsError::Invalid("invalid tensor dimension"))?,
                )
                .ok_or(SafetensorsError::Invalid("tensor size overflow"))?;
        }
        total = total
            .checked_add(elements)
            .ok_or(SafetensorsError::Invalid("tensor size overflow"))?;
    }
    if total == 0 {
        return Err(SafetensorsError::Invalid(
            "safetensors header has no tensors",
        ));
    }
    Ok(total)
}

pub struct FitOnlyInput<'a> {
    pub model: &'a HfModel,
    pub kind: GenerationKind,
    pub license: &'static str,
    /// Exact parameter count of a diffusion file (from its safetensors header), if read.
    pub params: &'a dyn Fn(&str) -> Option<u64>,
    pub today: &'a str,
}

fn folder(path: &str) -> Option<FileRole> {
    let segments: Vec<&str> = path.split('/').collect();
    let parent = segments.get(segments.len().checked_sub(2)?)?;
    match *parent {
        "diffusion_models" => Some(FileRole::Diffusion),
        "text_encoders" => Some(FileRole::TextEncoder),
        "vae" => Some(FileRole::Vae),
        _ => None,
    }
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".safetensors").unwrap_or(name)
}

/// Variants that condition on something other than a text prompt.
fn text_prompted(path: &str) -> bool {
    let name = stem(path).to_ascii_lowercase();
    ![
        "edit",
        "lora",
        "inpaint",
        "controlnet",
        "control_",
        "fill",
        "depth",
        "canny",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

fn generation_file(model: &HfModel, sibling: &Sibling, role: FileRole) -> Option<GenerationFile> {
    let lfs = sibling.lfs.as_ref()?;
    (lfs.sha256.len() == 64
        && lfs.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        && lfs.size > 0)
        .then(|| GenerationFile {
            role,
            folder: role.folder().into(),
            repo: model.id.clone(),
            revision: model.sha.clone(),
            file: sibling.rfilename.clone(),
            sha256: lfs.sha256.to_ascii_lowercase(),
            bytes: lfs.size,
        })
}

/// One fit-only entry per text-prompted diffusion file, paired with the smallest published
/// text encoder and VAE (the least memory the release can run with).
pub fn fit_only_entries(input: &FitOnlyInput) -> Result<Vec<GenerationModel>, ValidationError> {
    let model = input.model;
    require(model.pinned(), "repository is not a public pinned revision")?;
    let family = model
        .id
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let files: Vec<(&Sibling, FileRole)> = model
        .siblings
        .iter()
        .filter(|sibling| sibling.rfilename.ends_with(".safetensors"))
        .filter_map(|sibling| folder(&sibling.rfilename).map(|role| (sibling, role)))
        .collect();
    let smallest = |role: FileRole| {
        files
            .iter()
            .filter(|(_, candidate)| *candidate == role)
            .filter_map(|(sibling, _)| generation_file(model, sibling, role))
            .min_by(|left, right| {
                left.bytes
                    .cmp(&right.bytes)
                    .then_with(|| left.file.cmp(&right.file))
            })
    };
    let companions: Vec<GenerationFile> = [FileRole::TextEncoder, FileRole::Vae]
        .into_iter()
        .filter_map(smallest)
        .collect();
    let mut entries = Vec::new();
    for (sibling, role) in &files {
        if *role != FileRole::Diffusion || !text_prompted(&sibling.rfilename) {
            continue;
        }
        let Some(diffusion) = generation_file(model, sibling, FileRole::Diffusion) else {
            continue;
        };
        let Some(params) = (input.params)(&sibling.rfilename) else {
            continue;
        };
        let id = format!("{family}:{}", stem(&sibling.rfilename).to_ascii_lowercase());
        let mut entry_files = vec![diffusion];
        entry_files.extend(companions.iter().cloned());
        entries.push(GenerationModel {
            id,
            family: family.clone(),
            kind: input.kind,
            params: crate::admission::parameter_label(params as f64),
            license: input.license.into(),
            open_weight: true,
            release_date: None,
            added_at: Some(input.today.into()),
            provenance: EntryProvenance::Auto,
            default: false,
            source: format!("https://huggingface.co/{}", model.id),
            workflow: None,
            workflow_source: None,
            files: entry_files,
        });
    }
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(entries)
}
