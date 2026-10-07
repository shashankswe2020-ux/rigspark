use crate::sizing::{
    Architecture, KvCacheType, Model, Quantization, ValidationError, typed_kv_bytes_per_token,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};

pub const BACKENDS: [&str; 4] = ["ollama", "llamacpp", "mlx", "lmstudio"];
pub const CAPABILITIES: [&str; 6] = ["chat", "code", "vision", "reasoning", "tools", "embedding"];
pub const VENDORS: [&str; 5] = ["apple", "nvidia", "amd", "intel", "none"];
pub(crate) const LICENSES: [&str; 18] = [
    "apache-2.0",
    "mit",
    "modified-mit",
    "bsd-3-clause",
    "llama-2-community",
    "llama-3-community",
    "llama-3.1-community",
    "llama-3.2-community",
    "llama-3.3-community",
    "gemma",
    "qwen",
    "qwen-research",
    "tongyi-qianwen",
    "deepseek",
    "yi-license",
    "cc-by-4.0",
    "cc-by-sa-4.0",
    "openrail",
];

pub fn require(ok: bool, message: &str) -> Result<(), ValidationError> {
    if ok {
        Ok(())
    } else {
        Err(ValidationError(message.into()))
    }
}
fn matches(value: &str, pattern: &str) -> bool {
    Regex::new(pattern).is_ok_and(|regex| regex.is_match(value))
}
fn integer(value: f64, minimum: f64) -> bool {
    value.is_finite()
        && value.fract() == 0.0
        && value >= minimum
        && value <= 9_007_199_254_740_991.0
}
fn nonempty(value: &str) -> Result<(), ValidationError> {
    require(!value.is_empty(), "empty text field")
}
pub(crate) fn timestamp(value: &str) -> Result<(), ValidationError> {
    require(
        value.ends_with('Z') && OffsetDateTime::parse(value, &Rfc3339).is_ok(),
        "invalid generatedAt timestamp",
    )
}
pub fn date(value: &str) -> Result<Date, ValidationError> {
    Date::parse(
        value,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map_err(|_| ValidationError("invalid calendar date".into()))
}
fn no_nulls(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Array(values) => values.iter().all(no_nulls),
        serde_json::Value::Object(values) => values.values().all(no_nulls),
        _ => true,
    }
}
pub fn parse_document<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, ValidationError> {
    require(raw.len() <= 16 * 1024 * 1024, "dataset exceeds 16 MiB")?;
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| ValidationError(error.to_string()))?;
    require(no_nulls(&value), "null fields are not allowed")?;
    serde_json::from_value(value).map_err(|error| ValidationError(error.to_string()))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GgufSource {
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub sha256: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MlxFile {
    pub file: String,
    pub sha256: String,
    pub bytes: f64,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MlxSource {
    pub repo: String,
    pub revision: String,
    pub files: Vec<MlxFile>,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gguf: Option<GgufSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mlx: Option<MlxSource>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryProvenance {
    #[default]
    Curated,
    Auto,
}
impl EntryProvenance {
    pub(crate) fn is_curated(&self) -> bool {
        matches!(self, Self::Curated)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RecencyBasis {
    Released,
    Added,
}
pub const SCHEMA_VERSION: u8 = 3;
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogModel {
    pub id: String,
    pub family: String,
    pub params: String,
    pub architecture: Architecture,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_params: Option<String>,
    pub license: String,
    pub open_weight: bool,
    pub context_length: f64,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
    #[serde(default, skip_serializing_if = "EntryProvenance::is_curated")]
    pub provenance: EntryProvenance,
    pub source: Source,
    pub quantizations: Vec<Quantization>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kv_bytes_per_token: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub benchmark_proxy: Option<f64>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u8,
    pub generated_at: String,
    pub models: Vec<CatalogModel>,
}

pub(crate) fn coordinates(repo: &str, revision: &str) -> Result<(), ValidationError> {
    require(
        repo.len() <= 200
            && matches(
                repo,
                r"^[a-zA-Z0-9][a-zA-Z0-9._-]*/[a-zA-Z0-9][a-zA-Z0-9._-]*$",
            ),
        "invalid source repository",
    )?;
    require(
        matches(revision, r"^[0-9a-fA-F]{40}$"),
        "invalid pinned source revision",
    )
}
pub(crate) fn model_file(file: &str, max: usize) -> Result<(), ValidationError> {
    require(
        !file.is_empty()
            && file.len() <= max
            && !file
                .chars()
                .any(|ch| ch.is_control() || "*?[]{}\\%".contains(ch))
            && file
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
        "unsafe model file path",
    )
}
pub(crate) fn digest(value: &str) -> Result<(), ValidationError> {
    require(
        matches(value, r"^[0-9a-fA-F]{64}$"),
        "invalid SHA-256 digest",
    )
}

/// The date recency is measured from: the sourced release date, else the admission date.
pub fn recency<'a>(
    release_date: Option<&'a str>,
    added_at: Option<&'a str>,
) -> Option<(&'a str, RecencyBasis)> {
    release_date
        .map(|day| (day, RecencyBasis::Released))
        .or_else(|| added_at.map(|day| (day, RecencyBasis::Added)))
}
/// Display form of [`recency`]: the release date, `added <date>`, or `unknown`.
pub fn recency_label(recency: Option<(&str, RecencyBasis)>) -> String {
    match recency {
        Some((day, RecencyBasis::Released)) => day.to_string(),
        Some((day, RecencyBasis::Added)) => format!("added {day}"),
        None => "unknown".into(),
    }
}

impl CatalogModel {
    pub fn recency(&self) -> Option<(&str, RecencyBasis)> {
        recency(self.release_date.as_deref(), self.added_at.as_deref())
    }
    pub fn recency_label(&self) -> String {
        recency_label(self.recency())
    }
    pub fn sizing(&self) -> Model {
        Model {
            id: self.id.clone(),
            params: self.params.clone(),
            architecture: self.architecture.clone(),
            context_length: self.context_length,
            kv_bytes_per_token: self.kv_bytes_per_token,
            quantizations: self.quantizations.clone(),
        }
    }
    /// Re-sizes the sourced f16 KV rate for a cache type; unknown geometry stays unknown.
    pub fn with_kv_cache(&self, kind: KvCacheType) -> Result<Self, ValidationError> {
        let mut model = self.clone();
        if let Some(rate) = self.kv_bytes_per_token {
            model.kv_bytes_per_token =
                Some(typed_kv_bytes_per_token(rate as u64, kind, kind)? as f64);
        }
        Ok(model)
    }
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        nonempty(&self.id)?;
        nonempty(&self.family)?;
        require(
            matches(&self.params, r"^\d+(\.\d+)?[BMT]$"),
            "invalid parameter label",
        )?;
        require(
            self.open_weight
                && !self.license.is_empty()
                && LICENSES.contains(&self.license.as_str()),
            "catalog requires open-weight license",
        )?;
        require(
            integer(self.context_length, 1.0)
                && !self.capabilities.is_empty()
                && self
                    .capabilities
                    .iter()
                    .all(|cap| CAPABILITIES.contains(&cap.as_str())),
            "invalid context or capabilities",
        )?;
        if let Some(released) = &self.release_date {
            date(released)?;
        }
        if let Some(added) = &self.added_at {
            date(added)?;
        }
        match self.provenance {
            EntryProvenance::Curated => require(
                self.release_date.is_some(),
                "curated entry needs releaseDate",
            )?,
            // Auto entries carry only sourced facts and must stay integrity-pinned.
            EntryProvenance::Auto => require(
                self.added_at.is_some()
                    && self.benchmark_proxy.is_none()
                    && self.source.ollama.is_some()
                    && self
                        .quantizations
                        .iter()
                        .all(|quant| quant.sha256.is_some()),
                "auto entry needs addedAt, a pinned Ollama digest and no benchmarkProxy",
            )?,
        }
        require(
            matches!(self.architecture, Architecture::Moe) == self.active_params.is_some(),
            "activeParams must exist only for MoE",
        )?;
        if let Some(active) = &self.active_params {
            require(
                matches(active, r"^\d+(\.\d+)?[BMT]$"),
                "invalid activeParams",
            )?;
        }
        if let Some(rate) = self.kv_bytes_per_token {
            require(integer(rate, 1.0), "invalid KV geometry")?;
        }
        if let Some(proxy) = self.benchmark_proxy {
            require((0.0..=1.0).contains(&proxy), "invalid benchmark proxy")?;
        }
        require(
            !self.quantizations.is_empty(),
            "model must have quantizations",
        )?;
        for quant in &self.quantizations {
            nonempty(&quant.name)?;
            require(
                integer(quant.disk_bytes, 1.0)
                    && integer(quant.min_ram_bytes, 1.0)
                    && integer(quant.min_vram_bytes, 0.0),
                "invalid quantization sizes",
            )?;
            if let Some(sha) = &quant.sha256 {
                digest(sha)?;
            }
            require(quant.projectors.len() <= 16, "too many projector artifacts")?;
            let mut projector_digests = std::collections::HashSet::new();
            let mut projector_bytes = 0_u64;
            for projector in &quant.projectors {
                digest(&projector.sha256)?;
                require(
                    projector.bytes > 0
                        && projector.bytes <= 9_007_199_254_740_991
                        && projector_digests.insert(&projector.sha256),
                    "invalid projector artifact",
                )?;
                projector_bytes = projector_bytes
                    .checked_add(projector.bytes)
                    .ok_or_else(|| ValidationError("projector size overflow".into()))?;
            }
            require(
                quant.projectors.is_empty()
                    || (self.source.ollama.is_some()
                        && quant.sha256.is_some()
                        && (projector_bytes as f64) < quant.disk_bytes),
                "projector artifacts require pinned Ollama weights and aggregate size",
            )?;
        }
        let source = &self.source;
        require(
            source.ollama.is_some()
                || source.hf.is_some()
                || source.gguf.is_some()
                || source.mlx.is_some(),
            "missing model source",
        )?;
        for text in [&source.ollama, &source.hf].into_iter().flatten() {
            nonempty(text)?;
        }
        if let Some(gguf) = &source.gguf {
            coordinates(&gguf.repo, &gguf.revision)?;
            model_file(&gguf.file, 255)?;
            digest(&gguf.sha256)?;
        }
        if let Some(mlx) = &source.mlx {
            coordinates(&mlx.repo, &mlx.revision)?;
            require(
                (3..=256).contains(&mlx.files.len()) && self.quantizations.len() == 1,
                "invalid MLX manifest cardinality",
            )?;
            let mut paths = HashSet::new();
            let mut total = 0.0;
            for file in &mlx.files {
                model_file(&file.file, 512)?;
                digest(&file.sha256)?;
                require(
                    !matches(&file.file, r"(?i)\.(py|pyc|pyo|so|dylib|dll|bundle)$"),
                    "executable MLX file forbidden",
                )?;
                require(
                    integer(file.bytes, 1.0) && paths.insert(file.file.as_str()),
                    "invalid or duplicate MLX file",
                )?;
                total += file.bytes;
                require(integer(total, 1.0), "MLX total overflows")?;
            }
            require(
                paths.contains("config.json")
                    && paths.contains("tokenizer_config.json")
                    && paths.iter().any(|path| path.ends_with(".safetensors")),
                "incomplete MLX manifest",
            )?;
            require(
                total == self.quantizations[0].disk_bytes,
                "MLX manifest size mismatch",
            )?;
        }
        Ok(())
    }
}
impl Catalog {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let mut result: Self = parse_document(raw)?;
        require(
            (2..=SCHEMA_VERSION).contains(&result.schema_version) && !result.models.is_empty(),
            "unsupported or empty catalog",
        )?;
        timestamp(&result.generated_at)?;
        for model in &result.models {
            require(
                result.schema_version >= 3
                    || (model.provenance.is_curated() && model.added_at.is_none()),
                "schema v2 catalogs cannot carry v3 fields",
            )?;
            model.validate()?;
        }
        for model in &mut result.models {
            use crate::reports::strip_control;
            model.id = strip_control(&model.id);
            model.family = strip_control(&model.family);
            for value in [&mut model.source.ollama, &mut model.source.hf]
                .into_iter()
                .flatten()
            {
                *value = strip_control(value);
            }
            if let Some(source) = &mut model.source.gguf {
                source.file = strip_control(&source.file);
            }
            if let Some(source) = &mut model.source.mlx {
                for file in &mut source.files {
                    file.file = strip_control(&file.file);
                }
            }
            for quant in &mut model.quantizations {
                quant.name = strip_control(&quant.name);
            }
        }
        let mut ids = HashSet::new();
        for model in &result.models {
            require(ids.insert(&model.id), "duplicate catalog model id")?;
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    pub value: f64,
    pub trust_tier: String,
    pub basis_bytes_per_token: f64,
    pub url: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerfSources {
    pub bandwidth: String,
    pub efficiency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency_by_backend: Option<BTreeMap<String, Provenance>>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerfClass {
    pub id: String,
    pub label: String,
    pub vendor: String,
    pub kind: String,
    #[serde(rename = "memBandwidthGBps")]
    pub mem_bandwidth_gbps: f64,
    pub efficiency: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency_by_backend: Option<BTreeMap<String, f64>>,
    pub min_bytes: f64,
    pub max_bytes: f64,
    pub sources: PerfSources,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerfDataset {
    pub schema_version: u8,
    pub generated_at: String,
    pub classes: Vec<PerfClass>,
}
impl PerfDataset {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let mut result: Self = parse_document(raw)?;
        require(
            result.schema_version == 1 && !result.classes.is_empty(),
            "unsupported or empty performance data",
        )?;
        timestamp(&result.generated_at)?;
        for (index, class) in result.classes.iter().enumerate() {
            require(
                matches(&class.id, r"^[a-z0-9-]+$")
                    && !class.label.is_empty()
                    && VENDORS.contains(&class.vendor.as_str())
                    && ["discrete", "unified", "cpu"].contains(&class.kind.as_str()),
                "invalid performance class",
            )?;
            require(
                class.mem_bandwidth_gbps > 0.0
                    && class.efficiency > 0.0
                    && class.efficiency <= 1.0
                    && integer(class.min_bytes, 0.0)
                    && integer(class.max_bytes, 1.0)
                    && class.max_bytes > class.min_bytes,
                "invalid performance figures",
            )?;
            nonempty(&class.sources.bandwidth)?;
            nonempty(&class.sources.efficiency)?;
            let scalars = class.efficiency_by_backend.clone().unwrap_or_default();
            let provenance = class
                .sources
                .efficiency_by_backend
                .clone()
                .unwrap_or_default();
            require(
                scalars.keys().eq(provenance.keys()),
                "efficiency provenance mismatch",
            )?;
            for (backend, scalar) in scalars {
                let source = &provenance[&backend];
                require(
                    BACKENDS.contains(&backend.as_str())
                        && scalar > 0.0
                        && scalar <= 1.0
                        && source.value == scalar
                        && ["session-verified", "spec-grade"].contains(&source.trust_tier.as_str())
                        && source.basis_bytes_per_token > 0.0
                        && url::Url::parse(&source.url).is_ok(),
                    "invalid efficiency provenance",
                )?;
            }
            for prior in &result.classes[..index] {
                require(prior.id != class.id, "duplicate performance id")?;
                require(
                    prior.vendor != class.vendor
                        || prior.kind != class.kind
                        || prior.min_bytes >= class.max_bytes
                        || class.min_bytes >= prior.max_bytes,
                    "overlapping performance ranges",
                )?;
            }
        }
        for class in &mut result.classes {
            use crate::reports::strip_control;
            class.label = strip_control(&class.label);
            class.sources.bandwidth = strip_control(&class.sources.bandwidth);
            class.sources.efficiency = strip_control(&class.sources.efficiency);
            if let Some(sources) = &mut class.sources.efficiency_by_backend {
                for source in sources.values_mut() {
                    source.url = strip_control(&source.url);
                }
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Serialize)]
pub struct ResolutionError {
    pub code: &'static str,
    pub message: String,
    pub candidates: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct Resolved<'a> {
    pub model: &'a CatalogModel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quant: Option<&'a Quantization>,
}
fn resolution_error(message: String, candidates: Vec<String>, validation: bool) -> ResolutionError {
    ResolutionError {
        code: if validation {
            "VALIDATION_ERROR"
        } else {
            "MODEL_RESOLUTION_ERROR"
        },
        message,
        candidates,
    }
}
fn finalize(resolved: Resolved<'_>) -> Result<Resolved<'_>, ResolutionError> {
    for id in [
        Some(&resolved.model.id),
        resolved.model.source.ollama.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if id.contains("..") || !matches(id, r"^[a-z0-9._:/][a-z0-9._:/-]*$") {
            return Err(resolution_error(
                "unsafe catalog model id".into(),
                vec![],
                true,
            ));
        }
    }
    Ok(resolved)
}
pub fn resolve<'a>(catalog: &'a Catalog, input: &str) -> Result<Resolved<'a>, ResolutionError> {
    let query = input.trim().to_lowercase();
    if query.is_empty() || query.contains("..") || !matches(&query, r"^[a-z0-9._:/-]+$") {
        return Err(resolution_error("invalid model name".into(), vec![], true));
    }
    if let Some(model) = catalog
        .models
        .iter()
        .find(|model| model.id.to_lowercase() == query)
    {
        return finalize(Resolved { model, quant: None });
    }
    let quant_matches: Vec<Resolved<'_>> = catalog
        .models
        .iter()
        .flat_map(|model| {
            model.quantizations.iter().filter_map(|quant| {
                (format!("{}-{}", model.id, quant.name).to_lowercase() == query).then_some(
                    Resolved {
                        model,
                        quant: Some(quant),
                    },
                )
            })
        })
        .collect();
    let family: Vec<Resolved<'_>> = catalog
        .models
        .iter()
        .filter(|model| model.family.to_lowercase() == query)
        .map(|model| Resolved { model, quant: None })
        .collect();
    let prefix: Vec<Resolved<'_>> = catalog
        .models
        .iter()
        .filter(|model| {
            model.id.to_lowercase().starts_with(&query)
                || model.family.to_lowercase().starts_with(&query)
        })
        .map(|model| Resolved { model, quant: None })
        .collect();
    for mut tier in [quant_matches, family, prefix] {
        if tier.len() == 1 {
            return finalize(tier.remove(0));
        }
        if !tier.is_empty() {
            let mut candidates: Vec<String> =
                tier.iter().map(|entry| entry.model.id.clone()).collect();
            candidates.sort();
            return Err(resolution_error(
                format!(
                    "\"{query}\" is ambiguous; candidates: {}",
                    candidates.join(", ")
                ),
                candidates,
                false,
            ));
        }
    }
    Err(resolution_error(
        format!("no model matches \"{query}\""),
        vec![],
        false,
    ))
}
