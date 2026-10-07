//! Pure parsers and builders for automatic catalog admission from the Ollama library.
//! Network access lives in `rigspark-runtime`; everything here is deterministic.

use crate::{
    catalog::{CAPABILITIES, CatalogModel, EntryProvenance, LICENSES, Source, require},
    enrich::sized_quantization,
    registry_collector::ModelLayer,
    reports::strip_control,
    sizing::{Architecture, ValidationError, parse_param_count},
};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

pub const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_LICENSE_HEAD_BYTES: usize = 2048;
pub const MAX_GGUF_HEADER_BYTES: usize = 64 * 1024 * 1024;
const PARAMETER_TOLERANCE: f64 = 0.02;

fn pattern(cell: &'static OnceLock<Regex>, source: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(source).expect("valid admission pattern"))
}
fn repository_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub repository: String,
    /// Catalog capabilities sourced from the listing's chips, in catalog order.
    pub capabilities: Vec<String>,
}

fn capability(chip: &str) -> Option<&'static str> {
    match chip {
        "tools" => Some("tools"),
        "vision" => Some("vision"),
        "thinking" => Some("reasoning"),
        "embedding" => Some("embedding"),
        _ => None,
    }
}

/// Parses `ollama.com/library?sort=newest`, preserving the page's newest-first order.
pub fn parse_library(html: &str) -> Result<Vec<LibraryEntry>, ValidationError> {
    require(
        html.len() <= MAX_HTML_BYTES,
        "library listing exceeds 4 MiB",
    )?;
    static ANCHOR: OnceLock<Regex> = OnceLock::new();
    static CHIP: OnceLock<Regex> = OnceLock::new();
    let anchor = pattern(
        &ANCHOR,
        r#"<a\s+href="/library/([a-z0-9][a-z0-9._-]*)"\s+class="group w-full space-y-5"\s*>"#,
    );
    let chip = pattern(
        &CHIP,
        r#"<span\s+class="inline-flex items-center rounded-md[^"]*"\s*>\s*([a-z0-9.]{1,32})\s*</span>"#,
    );
    let starts: Vec<_> = anchor.captures_iter(html).collect();
    require(
        starts.len() <= 10000,
        "library listing has too many entries",
    )?;
    let mut entries: Vec<LibraryEntry> = Vec::new();
    for (index, captures) in starts.iter().enumerate() {
        let whole = captures.get(0).expect("match");
        let end = starts
            .get(index + 1)
            .map_or(html.len(), |next| next.get(0).expect("match").start());
        let repository = captures[1].to_string();
        if !repository_name(&repository)
            || entries.iter().any(|entry| entry.repository == repository)
        {
            continue;
        }
        let chips: Vec<&str> = chip
            .captures_iter(&html[whole.end()..end])
            .filter_map(|chip| capability(chip.get(1)?.as_str()))
            .collect();
        let capabilities = CAPABILITIES
            .iter()
            .filter(|known| chips.contains(known))
            .map(|known| known.to_string())
            .collect();
        entries.push(LibraryEntry {
            repository,
            capabilities,
        });
    }
    require(!entries.is_empty(), "library listing contains no models")?;
    Ok(entries)
}

/// Tags linked from `ollama.com/library/{repository}/tags`, sorted and deduplicated.
pub fn parse_tags(html: &str, repository: &str) -> Result<Vec<String>, ValidationError> {
    require(html.len() <= MAX_HTML_BYTES, "tags page exceeds 4 MiB")?;
    require(repository_name(repository), "invalid repository name")?;
    static LINK: OnceLock<Regex> = OnceLock::new();
    let link = pattern(
        &LINK,
        r#"href="/library/([a-z0-9][a-z0-9._-]*):([A-Za-z0-9._-]{1,128})""#,
    );
    let mut tags: Vec<String> = link
        .captures_iter(html)
        .filter(|captures| &captures[1] == repository)
        .map(|captures| captures[2].to_string())
        .collect();
    tags.sort();
    tags.dedup();
    require(tags.len() <= 2000, "tags page has too many tags")?;
    Ok(tags)
}

/// Per size, Ollama's default tag (`27b`, `30b-a3b`), else that size's `-q4_K_M` tag.
pub fn select_variants(tags: &[String]) -> Vec<String> {
    static PLAIN: OnceLock<Regex> = OnceLock::new();
    static DEFAULT_QUANT: OnceLock<Regex> = OnceLock::new();
    let plain = pattern(&PLAIN, r"^\d+(\.\d+)?[bm](-a\d+(\.\d+)?b)?$");
    let default_quant = pattern(
        &DEFAULT_QUANT,
        r"(?i)^(\d+(\.\d+)?[bm](-a\d+(\.\d+)?b)?)-q4_k_m$",
    );
    let mut chosen: BTreeMap<String, String> = BTreeMap::new();
    for tag in tags {
        if plain.is_match(tag) {
            chosen.insert(tag.clone(), tag.clone());
        }
    }
    for tag in tags {
        if let Some(captures) = default_quant.captures(tag) {
            chosen
                .entry(captures[1].to_string())
                .or_insert_with(|| tag.clone());
        }
    }
    chosen.into_values().collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryConfig {
    pub model_family: String,
    pub model_type: String,
    pub file_type: String,
}
impl RegistryConfig {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        require(raw.len() <= MAX_CONFIG_BYTES, "model config exceeds 64 KiB")?;
        let config: Self =
            serde_json::from_str(raw).map_err(|error| ValidationError(error.to_string()))?;
        require(
            [&config.model_family, &config.model_type, &config.file_type]
                .iter()
                .all(|text| {
                    !text.is_empty() && text.len() <= 64 && !text.chars().any(char::is_control)
                }),
            "model config lacks family, size or file type",
        )?;
        Ok(config)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GgufValue {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    /// Arrays keep only their length; no admission fact needs their items.
    Array(u64),
    /// Text longer than 4 KiB (chat templates and similar), skipped unread.
    Omitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GgufError {
    /// The header continues beyond the bytes read; fetch more and retry.
    Truncated,
    Invalid(&'static str),
}

#[derive(Debug, Clone)]
pub struct Tensor {
    pub name: String,
    pub elements: u64,
}

#[derive(Debug, Clone)]
pub struct Gguf {
    metadata: BTreeMap<String, GgufValue>,
    tensors: Vec<Tensor>,
}

#[derive(Clone, Copy)]
enum Text {
    Name,
    Value,
    Skip,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl Cursor<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], GgufError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(GgufError::Invalid("offset overflow"))?;
        if end > MAX_GGUF_HEADER_BYTES {
            return Err(GgufError::Invalid("GGUF header exceeds 64 MiB"));
        }
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or(GgufError::Truncated)?;
        self.offset = end;
        Ok(slice)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        Ok(self.take(N)?.try_into().expect("exact length"))
    }
    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    /// Reads a GGUF string: names are capped at 1 KiB, values over 4 KiB are skipped.
    fn text(&mut self, kind: Text) -> Result<Option<String>, GgufError> {
        let length = self.u64()?;
        if length > 16 * 1024 * 1024 {
            return Err(GgufError::Invalid("GGUF string too long"));
        }
        let bytes = self.take(length as usize)?;
        let keep = match kind {
            Text::Name if bytes.len() > 1024 => {
                return Err(GgufError::Invalid("GGUF name too long"));
            }
            Text::Name => true,
            Text::Value => bytes.len() <= 4096,
            Text::Skip => false,
        };
        if !keep {
            return Ok(None);
        }
        String::from_utf8(bytes.to_vec())
            .map(Some)
            .map_err(|_| GgufError::Invalid("GGUF text is not UTF-8"))
    }
    fn scalar(&mut self, kind: u32) -> Result<GgufValue, GgufError> {
        Ok(match kind {
            0 => GgufValue::Unsigned(u64::from(self.array::<1>()?[0])),
            1 => GgufValue::Signed(i64::from(i8::from_le_bytes(self.array()?))),
            2 => GgufValue::Unsigned(u64::from(u16::from_le_bytes(self.array()?))),
            3 => GgufValue::Signed(i64::from(i16::from_le_bytes(self.array()?))),
            4 => GgufValue::Unsigned(u64::from(self.u32()?)),
            5 => GgufValue::Signed(i64::from(i32::from_le_bytes(self.array()?))),
            6 => GgufValue::Float(f64::from(f32::from_le_bytes(self.array()?))),
            7 => GgufValue::Bool(self.array::<1>()?[0] != 0),
            10 => GgufValue::Unsigned(self.u64()?),
            11 => GgufValue::Signed(i64::from_le_bytes(self.array()?)),
            12 => GgufValue::Float(f64::from_le_bytes(self.array()?)),
            _ => return Err(GgufError::Invalid("unknown GGUF value type")),
        })
    }
    fn value(&mut self, kind: u32, depth: u8) -> Result<GgufValue, GgufError> {
        match kind {
            8 => Ok(self
                .text(Text::Value)?
                .map_or(GgufValue::Omitted, GgufValue::Text)),
            9 => {
                if depth > 1 {
                    return Err(GgufError::Invalid("nested GGUF arrays are too deep"));
                }
                let item = self.u32()?;
                let length = self.u64()?;
                if length > 64 * 1024 * 1024 {
                    return Err(GgufError::Invalid("GGUF array too long"));
                }
                for _ in 0..length {
                    match item {
                        8 => {
                            self.text(Text::Skip)?;
                        }
                        9 => {
                            self.value(9, depth + 1)?;
                        }
                        other => {
                            self.scalar(other)?;
                        }
                    }
                }
                Ok(GgufValue::Array(length))
            }
            other => self.scalar(other),
        }
    }
}

/// Parses GGUF v2/v3 metadata and the tensor table (names and element counts only).
pub fn parse_gguf(bytes: &[u8]) -> Result<Gguf, GgufError> {
    let mut cursor = Cursor { bytes, offset: 0 };
    if cursor.take(4)? != b"GGUF" {
        return Err(GgufError::Invalid("not a GGUF file"));
    }
    if !matches!(cursor.u32()?, 2 | 3) {
        return Err(GgufError::Invalid("unsupported GGUF version"));
    }
    let tensor_count = cursor.u64()?;
    let metadata_count = cursor.u64()?;
    if tensor_count > 100_000 || metadata_count > 65_536 {
        return Err(GgufError::Invalid("implausible GGUF counts"));
    }
    let mut metadata = BTreeMap::new();
    for _ in 0..metadata_count {
        let key = cursor.text(Text::Name)?.expect("names are kept");
        let kind = cursor.u32()?;
        let value = cursor.value(kind, 0)?;
        if metadata.insert(key, value).is_some() {
            return Err(GgufError::Invalid("duplicate GGUF metadata key"));
        }
    }
    let mut tensors = Vec::new();
    for _ in 0..tensor_count {
        let name = cursor.text(Text::Name)?.expect("names are kept");
        let dimensions = cursor.u32()?;
        if !(1..=8).contains(&dimensions) {
            return Err(GgufError::Invalid("invalid tensor rank"));
        }
        let mut elements: u64 = 1;
        for _ in 0..dimensions {
            elements = elements
                .checked_mul(cursor.u64()?)
                .ok_or(GgufError::Invalid("tensor size overflow"))?;
        }
        cursor.u32()?;
        cursor.u64()?;
        tensors.push(Tensor { name, elements });
    }
    Ok(Gguf { metadata, tensors })
}

impl Gguf {
    pub fn architecture(&self) -> Option<&str> {
        self.text("general.architecture")
            .filter(|arch| repository_name(arch))
    }
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.metadata.get(key) {
            Some(GgufValue::Text(text)) => Some(text),
            _ => None,
        }
    }
    pub fn u64(&self, key: &str) -> Option<u64> {
        match self.metadata.get(key) {
            Some(GgufValue::Unsigned(value)) => Some(*value),
            Some(GgufValue::Signed(value)) => u64::try_from(*value).ok(),
            _ => None,
        }
    }
    fn arch_u64(&self, suffix: &str) -> Option<u64> {
        self.u64(&format!("{}.{suffix}", self.architecture()?))
    }
    pub fn total_elements(&self) -> u64 {
        self.tensors.iter().map(|tensor| tensor.elements).sum()
    }
}

/// f16 KV-cache bytes per token for uniform full attention; `None` when the architecture
/// mixes attention kinds, where the standard formula would fabricate a number.
pub fn kv_bytes_per_token(gguf: &Gguf) -> Option<f64> {
    let arch = gguf.architecture()?;
    let prefix = format!("{arch}.");
    let non_uniform = gguf.metadata.iter().any(|(key, value)| {
        key.strip_prefix(&prefix).is_some_and(|suffix| {
            suffix.starts_with("ssm.")
                || suffix == "full_attention_interval"
                || suffix.contains("sliding_window")
                || suffix == "attention.kv_lora_rank"
                || (suffix == "attention.head_count_kv" && matches!(value, GgufValue::Array(_)))
        })
    });
    if non_uniform {
        return None;
    }
    let layers = gguf.arch_u64("block_count")?;
    let kv_heads = gguf.arch_u64("attention.head_count_kv")?;
    let head_dim = || {
        let heads = gguf.arch_u64("attention.head_count")?;
        let width = gguf.arch_u64("embedding_length")?;
        (heads > 0 && width % heads == 0).then(|| width / heads)
    };
    let key = gguf.arch_u64("attention.key_length").or_else(head_dim)?;
    let value = gguf.arch_u64("attention.value_length").or_else(head_dim)?;
    let bytes = layers
        .checked_mul(kv_heads)?
        .checked_mul(key.checked_add(value)?)?
        .checked_mul(2)?;
    (bytes > 0).then_some(bytes as f64)
}

/// Active parameters per token for MoE: shared tensors plus the routed share of expert tensors.
pub fn active_params(gguf: &Gguf) -> Option<f64> {
    let experts = gguf.arch_u64("expert_count").filter(|count| *count > 0)?;
    let used = gguf
        .arch_u64("expert_used_count")
        .filter(|used| *used > 0 && *used <= experts)?;
    let routed: u64 = gguf
        .tensors
        .iter()
        .filter(|tensor| tensor.name.contains("_exps."))
        .map(|tensor| tensor.elements)
        .sum();
    if routed == 0 {
        return None;
    }
    let shared = gguf.total_elements() - routed;
    Some(shared as f64 + routed as f64 * used as f64 / experts as f64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseError {
    Missing,
    Unrecognised,
    Conflict,
}

fn gguf_license(id: &str, name: Option<&str>) -> Option<&'static str> {
    let id = id.trim().to_ascii_lowercase();
    let mapped = match id.as_str() {
        "apache-2.0" | "apache 2.0" => "apache-2.0",
        "mit" => "mit",
        "bsd-3-clause" => "bsd-3-clause",
        "cc-by-4.0" => "cc-by-4.0",
        "cc-by-sa-4.0" => "cc-by-sa-4.0",
        "gemma" => "gemma",
        "llama2" => "llama-2-community",
        "llama3" => "llama-3-community",
        "llama3.1" => "llama-3.1-community",
        "llama3.2" => "llama-3.2-community",
        "llama3.3" => "llama-3.3-community",
        "other" => return name.and_then(|name| gguf_license(name, None)),
        _ => return None,
    };
    LICENSES.contains(&mapped).then_some(mapped)
}

fn blob_license(head: &str) -> Option<&'static str> {
    let text = head
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let opening: String = text.chars().take(120).collect();
    let starts = |prefix: &str| text.starts_with(prefix);
    if starts("apache license") && opening.contains("2.0") {
        Some("apache-2.0")
    } else if (starts("mit license") || starts("copyright"))
        && text.contains("permission is hereby granted, free of charge")
    {
        Some("mit")
    } else if starts("llama 3.3 community license agreement") {
        Some("llama-3.3-community")
    } else if starts("llama 3.2 community license agreement") {
        Some("llama-3.2-community")
    } else if starts("llama 3.1 community license agreement") {
        Some("llama-3.1-community")
    } else if starts("meta llama 3 community license agreement") {
        Some("llama-3-community")
    } else if starts("llama 2 community license agreement") {
        Some("llama-2-community")
    } else if starts("gemma terms of use") {
        Some("gemma")
    } else {
        None
    }
}

/// The catalog license id, accepted only when sourced, open and unambiguous.
pub fn license_id(
    gguf: Option<&str>,
    gguf_name: Option<&str>,
    blob_head: Option<&str>,
) -> Result<&'static str, LicenseError> {
    let from_gguf = gguf.map(|id| gguf_license(id, gguf_name));
    let from_blob = blob_head
        .filter(|head| !head.trim().is_empty())
        .map(blob_license);
    match (from_gguf, from_blob) {
        (Some(Some(left)), Some(Some(right))) if left == right => Ok(left),
        (Some(Some(_)), Some(Some(_))) => Err(LicenseError::Conflict),
        (Some(Some(id)), None) | (None, Some(Some(id))) => Ok(id),
        (Some(None), _) | (_, Some(None)) => Err(LicenseError::Unrecognised),
        (None, None) => Err(LicenseError::Missing),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection(&'static str);
impl Rejection {
    pub fn reason(&self) -> &'static str {
        self.0
    }
}

/// Catalog capabilities from listing chips: `chat` unless embedding-only, then chips in catalog order.
pub fn catalog_capabilities(chips: &[String]) -> Vec<String> {
    let mut capabilities: Vec<String> = Vec::new();
    if !chips.iter().any(|cap| cap == "embedding") {
        capabilities.push("chat".into());
    }
    for known in CAPABILITIES {
        if known != "chat" && chips.iter().any(|cap| cap == known) {
            capabilities.push(known.into());
        }
    }
    capabilities
}

pub struct AdmissionInput<'a> {
    pub repository: &'a str,
    pub tag: &'a str,
    pub capabilities: &'a [String],
    pub layer: &'a ModelLayer,
    pub config: &'a RegistryConfig,
    pub gguf: &'a Gguf,
    pub license_head: Option<&'a str>,
    pub today: &'a str,
}

fn parameter_label(count: f64) -> String {
    let (value, unit) = if count >= 1e12 {
        (count / 1e12, "T")
    } else if count >= 1e9 {
        (count / 1e9, "B")
    } else {
        (count / 1e6, "M")
    };
    let text = format!("{value:.1}");
    format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
}

/// Builds a fully sourced `provenance: auto` entry, or explains why the variant is skipped.
pub fn build_entry(input: &AdmissionInput) -> Result<CatalogModel, Rejection> {
    let id = format!("{}:{}", input.repository, input.tag);
    let config = input.config;
    let gguf = input.gguf;
    let architecture = gguf
        .architecture()
        .ok_or(Rejection("GGUF architecture missing"))?;
    static PARAMS: OnceLock<Regex> = OnceLock::new();
    let params = config.model_type.trim();
    if !pattern(&PARAMS, r"^\d+(\.\d+)?[BMT]$").is_match(params) {
        return Err(Rejection("parameter count not sourced"));
    }
    let count = parse_param_count(params).map_err(|_| Rejection("parameter count not sourced"))?;
    let tensors = gguf.total_elements() as f64;
    if (tensors - count).abs() > count * PARAMETER_TOLERANCE {
        return Err(Rejection("parameter count disagrees with tensor table"));
    }
    let license = license_id(
        gguf.text("general.license"),
        gguf.text("general.license.name"),
        input.license_head,
    )
    .map_err(|error| {
        Rejection(match error {
            LicenseError::Missing => "license missing",
            LicenseError::Unrecognised => "license not recognised as open",
            LicenseError::Conflict => "license sources disagree",
        })
    })?;
    let context = gguf
        .u64(&format!("{architecture}.context_length"))
        .filter(|tokens| *tokens > 0)
        .ok_or(Rejection("context length missing"))?;
    let (architecture_kind, active) = if gguf
        .u64(&format!("{architecture}.expert_count"))
        .is_some_and(|experts| experts > 0)
    {
        let active = active_params(gguf).ok_or(Rejection("MoE active parameters not sourced"))?;
        (Architecture::Moe, Some(parameter_label(active)))
    } else {
        (Architecture::Dense, None)
    };
    let capabilities = catalog_capabilities(input.capabilities);
    let quant = sized_quantization(
        count,
        &architecture_kind,
        &config.file_type.to_ascii_uppercase(),
        input.layer.disk_bytes,
        Some(input.layer.sha256.clone()),
        input.layer.projectors.clone(),
    )
    .map_err(|_| Rejection("quantization cannot be sized"))?;
    let entry = CatalogModel {
        id: strip_control(&id),
        family: strip_control(&config.model_family),
        params: params.into(),
        architecture: architecture_kind,
        active_params: active,
        license: license.into(),
        open_weight: true,
        context_length: context as f64,
        capabilities,
        release_date: None,
        added_at: Some(input.today.into()),
        provenance: EntryProvenance::Auto,
        source: Source {
            ollama: Some(strip_control(&id)),
            ..Default::default()
        },
        quantizations: vec![quant],
        kv_bytes_per_token: kv_bytes_per_token(gguf),
        benchmark_proxy: None,
    };
    let encoded = json!({"schemaVersion":crate::catalog::SCHEMA_VERSION,"generatedAt":"2026-01-01T00:00:00.000Z","models":[entry]});
    crate::catalog::Catalog::parse(&encoded.to_string())
        .map_err(|_| Rejection("entry fails catalog validation"))?;
    Ok(entry)
}

/// The quality-gate observation for an entry, citing exactly the reads that sourced it.
pub fn observation(entry: &CatalogModel, checked_at: &str) -> Value {
    let reference = entry.source.ollama.as_deref().unwrap_or(&entry.id);
    let (repository, tag) = crate::registry_collector::parse_reference(reference);
    let path = if repository.contains('/') {
        repository.to_string()
    } else {
        format!("library/{repository}")
    };
    json!({
        "id": entry.id,
        "checkedAt": checked_at,
        "sources": [
            format!("https://registry.ollama.ai/v2/{path}/manifests/{tag}"),
            format!("https://ollama.com/{path}:{tag}"),
        ],
        "model": entry,
    })
}
