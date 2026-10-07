use crate::{
    catalog::{Catalog, GgufSource, MlxSource, parse_document},
    enrich::{Mode, RawModel, enrich},
    sizing::{ValidationError, parse_param_count},
};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const BOOTSTRAP_CLOCK: &str = "2026-08-04T00:00:00.000Z";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Metadata {
    kv_bytes_per_token: Option<f64>,
    gguf: Option<GgufSource>,
    mlx: Option<MlxSource>,
}

pub fn build_catalog(candidates: &[RawModel], now: &str) -> Result<Catalog, ValidationError> {
    let seed = Catalog {
        schema_version: crate::catalog::SCHEMA_VERSION,
        generated_at: BOOTSTRAP_CLOCK.into(),
        models: Vec::new(),
    };
    let mut catalog = enrich(&seed, candidates, Mode::Backfill, now, None)?.catalog;
    let metadata: BTreeMap<String, Metadata> =
        parse_document(include_str!("../fixtures/bootstrap-metadata.json"))?;
    for model in &mut catalog.models {
        if let Some(entry) = metadata.get(&model.id) {
            model.kv_bytes_per_token = entry.kv_bytes_per_token;
            model.source.gguf = entry.gguf.clone();
            model.source.mlx = entry.mlx.clone();
        }
        model.benchmark_proxy = family_quality_offset(&model.family)
            .map(|_| derive_benchmark_proxy(&model.family, &model.params))
            .transpose()?;
    }
    let encoded =
        serde_json::to_string(&catalog).map_err(|error| ValidationError(error.to_string()))?;
    Catalog::parse(&encoded)
}

pub fn family_quality_offset(family: &str) -> Option<f64> {
    let offset = match family {
        "kimi-k2" | "deepseek-r1" | "phi4" => 0.12,
        "kimi-k2-thinking" => 0.14,
        "kimi-vl" | "qwen2.5" | "gemma3n" | "gemma3" | "phi3" => 0.06,
        "kimi-dev" | "kimi-linear" | "llama3.3" | "qwen2.5-coder" | "gemma4" | "phi3.5" => 0.08,
        "llama3.1" | "gemma2" => 0.04,
        "qwen3" | "deepseek-v3" | "phi4-mini" => 0.1,
        "mixtral" | "mistral-small" | "glm4" => 0.05,
        "mistral-nemo" | "yi" | "granite3.1" => 0.03,
        "mistral" | "olmo2" | "granite3-moe" => 0.02,
        "llama3.2" | "smollm2" => 0.0,
        _ => return None,
    };
    Some(offset)
}

pub fn derive_benchmark_proxy(family: &str, params: &str) -> Result<f64, ValidationError> {
    let offset = family_quality_offset(family).unwrap_or(0.0);
    let billions = parse_param_count(params)? / 1e9;
    let base = [
        (0.5, 0.05),
        (1.0, 0.12),
        (2.0, 0.2),
        (4.0, 0.28),
        (7.0, 0.36),
        (10.0, 0.44),
        (15.0, 0.52),
        (25.0, 0.58),
        (40.0, 0.64),
        (80.0, 0.7),
        (200.0, 0.78),
    ]
    .into_iter()
    .find_map(|(ceiling, score)| (billions < ceiling).then_some(score))
    .unwrap_or(0.85_f64);
    Ok(((base + offset).clamp(0.05, 0.99) * 100.0).round() / 100.0)
}
