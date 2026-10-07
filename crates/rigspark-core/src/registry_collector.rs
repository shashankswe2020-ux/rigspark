use crate::{
    catalog::{CatalogModel, require},
    sizing::{Architecture, ProjectorArtifact, ValidationError, parse_param_count, quant_bits},
};
use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MANIFEST_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json";

pub struct ModelLayer {
    pub disk_bytes: f64,
    pub sha256: String,
    pub projectors: Vec<ProjectorArtifact>,
}

impl ModelLayer {
    fn validate(&self) -> Result<(), ValidationError> {
        require(
            self.disk_bytes.is_finite()
                && self.disk_bytes > 0.0
                && self.disk_bytes.fract() == 0.0
                && self.disk_bytes <= 9_007_199_254_740_991.0,
            "invalid model layer size",
        )?;
        require(
            self.sha256.len() == 64 && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid model layer digest",
        )
    }
}

#[derive(Deserialize)]
struct Manifest {
    layers: Vec<Layer>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layer {
    media_type: String,
    digest: String,
    size: f64,
}

pub fn parse_reference(reference: &str) -> (&str, &str) {
    let reference = reference.trim();
    match reference.rsplit_once(':') {
        Some((path, tag)) if !path.is_empty() && !tag.contains('/') => (path, tag),
        _ => (reference, "latest"),
    }
}

pub fn quant_from_tag(tag: &str) -> Option<String> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[-_])(q\d[a-z0-9_]*|bf16|f16|f32)$")
            .expect("valid quant suffix pattern")
    });
    let captures = pattern.captures(tag)?;
    let token = captures.get(1)?.as_str();
    if token.starts_with(['q', 'Q']) {
        Some(format!("Q{}", &token[1..]))
    } else {
        Some(token.to_ascii_uppercase())
    }
}

/// Index of the quantization an entry's Ollama tag pulls: the tag's quant suffix, else the first.
pub fn pulled_quantization(model: &CatalogModel) -> Option<usize> {
    let (_, tag) = parse_reference(model.source.ollama.as_deref()?);
    match quant_from_tag(tag) {
        Some(target) => model
            .quantizations
            .iter()
            .position(|quant| quant.name == target),
        None => (!model.quantizations.is_empty()).then_some(0),
    }
}

pub fn parse_layer(raw: &str) -> Result<Option<ModelLayer>, ValidationError> {
    require(raw.len() <= MAX_MANIFEST_BYTES, "manifest exceeds 4 MiB")?;
    let manifest: Manifest =
        serde_json::from_str(raw).map_err(|error| ValidationError(error.to_string()))?;
    require(!manifest.layers.is_empty(), "manifest layers are empty")?;
    require(
        manifest.layers.iter().all(|layer| {
            layer.size.is_finite()
                && layer.size >= 0.0
                && layer.size.fract() == 0.0
                && layer.size <= 9_007_199_254_740_991.0
        }),
        "invalid manifest layer size",
    )?;
    require(manifest.layers.len() <= 10000, "too many manifest layers")?;
    let projectors = manifest
        .layers
        .iter()
        .filter(|layer| layer.media_type == "application/vnd.ollama.image.projector")
        .map(|layer| {
            let sha = layer
                .digest
                .strip_prefix("sha256:")
                .ok_or_else(|| ValidationError("invalid projector digest prefix".into()))?;
            require(
                layer.size > 0.0
                    && sha.len() == 64
                    && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid projector artifact",
            )?;
            Ok(ProjectorArtifact {
                bytes: layer.size as u64,
                sha256: sha.into(),
            })
        })
        .collect::<Result<Vec<_>, ValidationError>>()?;
    require(projectors.len() <= 16, "too many projector artifacts")?;
    require(
        manifest
            .layers
            .iter()
            .filter(|layer| layer.media_type == "application/vnd.ollama.image.model")
            .count()
            <= 1,
        "multiple model layers are unsupported",
    )?;
    let Some(layer) = manifest
        .layers
        .into_iter()
        .find(|layer| layer.media_type == "application/vnd.ollama.image.model")
    else {
        return Ok(None);
    };
    let digest = layer
        .digest
        .strip_prefix("sha256:")
        .ok_or_else(|| ValidationError("invalid model layer digest prefix".into()))?;
    let result = ModelLayer {
        disk_bytes: layer.size
            + projectors
                .iter()
                .map(|projector| projector.bytes as f64)
                .sum::<f64>(),
        sha256: digest.into(),
        projectors,
    };
    result.validate()?;
    Ok(Some(result))
}

pub fn apply_layer(
    model: &CatalogModel,
    layer: &ModelLayer,
) -> Result<Option<CatalogModel>, ValidationError> {
    model.validate()?;
    layer.validate()?;
    let Some(source) = &model.source.ollama else {
        return Ok(None);
    };
    let (_, tag) = parse_reference(source);
    let target = quant_from_tag(tag);
    let mut updated = model.clone();
    let mut changed = false;
    for (index, quant) in updated.quantizations.iter_mut().enumerate() {
        if !target
            .as_ref()
            .map_or(index == 0, |target| quant.name == *target)
            || (quant.disk_bytes == layer.disk_bytes
                && quant.sha256.as_deref() == Some(&layer.sha256)
                && quant.projectors == layer.projectors)
        {
            continue;
        }
        if quant.disk_bytes != layer.disk_bytes || !layer.projectors.is_empty() {
            let bits = quant_bits(&quant.name);
            require(
                bits.is_some() || !matches!(model.architecture, Architecture::Moe),
                "unknown MoE quantization",
            )?;
            let count = parse_param_count(&model.params)?;
            let projector_bytes = layer
                .projectors
                .iter()
                .map(|projector| projector.bytes as f64)
                .sum::<f64>();
            let resident = (layer.disk_bytes - projector_bytes)
                .max(bits.map(|bits| (count * bits / 8.0).ceil()).unwrap_or(0.0))
                + projector_bytes;
            let memory = resident + (resident * 0.15).ceil();
            quant.min_ram_bytes = memory;
            quant.min_vram_bytes = memory;
        }
        quant.disk_bytes = layer.disk_bytes;
        quant.sha256 = Some(layer.sha256.clone());
        quant.projectors = layer.projectors.clone();
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    updated.validate()?;
    Ok(Some(updated))
}
