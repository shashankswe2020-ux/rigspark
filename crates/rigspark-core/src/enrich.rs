use crate::{
    catalog::{Catalog, CatalogModel, LICENSES, Source, parse_document, require},
    reports::strip_control,
    sizing::{Architecture, Quantization, ValidationError, parse_param_count, quant_bits},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Backfill,
    Incremental,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawQuant {
    pub name: String,
    pub disk_bytes: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projectors: Vec<crate::sizing::ProjectorArtifact>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hf: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawModel {
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
    pub release_date: String,
    pub source: RawSource,
    pub quantizations: Vec<RawQuant>,
}
impl RawModel {
    fn validate_shape(&self) -> Result<(), ValidationError> {
        let integer = |value: f64| {
            value.is_finite()
                && value > 0.0
                && value.fract() == 0.0
                && value <= 9_007_199_254_740_991.0
        };
        require(
            [
                &self.id,
                &self.family,
                &self.params,
                &self.license,
                &self.release_date,
            ]
            .iter()
            .all(|text| !text.is_empty())
                && self
                    .active_params
                    .as_ref()
                    .is_none_or(|text| !text.is_empty())
                && integer(self.context_length)
                && !self.capabilities.is_empty()
                && self.capabilities.iter().all(|text| !text.is_empty())
                && [&self.source.ollama, &self.source.hf]
                    .into_iter()
                    .flatten()
                    .all(|text| !text.is_empty())
                && !self.quantizations.is_empty()
                && self.quantizations.iter().all(|quant| {
                    !quant.name.is_empty()
                        && integer(quant.disk_bytes)
                        && quant.sha256.as_ref().is_none_or(|text| !text.is_empty())
                }),
            "invalid registry payload",
        )
    }
    fn build(&self, prior: Option<&CatalogModel>) -> Result<CatalogModel, ValidationError> {
        let count = parse_param_count(&self.params)?;
        let mut quantizations = Vec::new();
        for quant in &self.quantizations {
            quantizations.push(sized_quantization(
                count,
                &self.architecture,
                &strip_control(&quant.name),
                quant.disk_bytes,
                quant.sha256.as_ref().map(|sha| strip_control(sha)),
                quant.projectors.clone(),
            )?);
        }
        let model = CatalogModel {
            id: strip_control(&self.id),
            family: strip_control(&self.family),
            params: strip_control(&self.params),
            architecture: self.architecture.clone(),
            active_params: self
                .active_params
                .as_ref()
                .map(|value| strip_control(value)),
            license: self.license.clone(),
            open_weight: self.open_weight,
            context_length: self.context_length,
            capabilities: self.capabilities.clone(),
            release_date: Some(self.release_date.clone()),
            added_at: None,
            provenance: crate::catalog::EntryProvenance::Curated,
            availability: prior.and_then(|model| model.availability),
            source: Source {
                ollama: self
                    .source
                    .ollama
                    .as_ref()
                    .map(|value| strip_control(value)),
                hf: self.source.hf.as_ref().map(|value| strip_control(value)),
                ..Default::default()
            },
            quantizations,
            kv_bytes_per_token: prior.and_then(|model| model.kv_bytes_per_token),
            benchmark_proxy: prior.and_then(|model| model.benchmark_proxy),
        };
        model.validate()?;
        Ok(model)
    }
}
/// Memory need for one quantization: resident weights (never below the bit-width floor) plus 15% headroom.
pub fn sized_quantization(
    params: f64,
    architecture: &Architecture,
    name: &str,
    disk_bytes: f64,
    sha256: Option<String>,
    projectors: Vec<crate::sizing::ProjectorArtifact>,
) -> Result<Quantization, ValidationError> {
    let bits = quant_bits(name);
    require(
        bits.is_some() || !matches!(architecture, Architecture::Moe),
        "unknown MoE quantization",
    )?;
    let projector_bytes = projectors
        .iter()
        .map(|projector| projector.bytes as f64)
        .sum::<f64>();
    require(
        projector_bytes < disk_bytes,
        "projector size exceeds aggregate weights",
    )?;
    let resident = (disk_bytes - projector_bytes)
        .max(bits.map(|bits| (params * bits / 8.0).ceil()).unwrap_or(0.0))
        + projector_bytes;
    let memory = resident + (resident * 0.15).ceil();
    Ok(Quantization {
        name: name.into(),
        disk_bytes,
        min_ram_bytes: memory,
        min_vram_bytes: memory,
        sha256,
        digest_verified: None,
        projectors,
    })
}
pub fn parse_candidates(raw: &str) -> Result<Vec<RawModel>, ValidationError> {
    let candidates: Vec<RawModel> = parse_document(raw)?;
    require(
        candidates.len() <= 10000,
        "registry candidate limit exceeded",
    )?;
    for candidate in &candidates {
        candidate.validate_shape()?;
    }
    Ok(candidates)
}
#[derive(Default, Debug, Serialize)]
pub struct EnrichDiff {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
    pub capped: Vec<String>,
}
#[derive(Serialize)]
pub struct EnrichResult {
    pub catalog: Catalog,
    pub diff: EnrichDiff,
}
pub fn enrich(
    existing: &Catalog,
    candidates: &[RawModel],
    mode: Mode,
    now: &str,
    maximum: Option<usize>,
) -> Result<EnrichResult, ValidationError> {
    require(
        (2..=crate::catalog::SCHEMA_VERSION).contains(&existing.schema_version)
            && maximum != Some(0)
            && candidates.len() <= 10000,
        "invalid enrichment options",
    )?;
    require(
        existing.generated_at.ends_with('Z')
            && OffsetDateTime::parse(&existing.generated_at, &Rfc3339).is_ok(),
        "invalid prior catalog timestamp",
    )?;
    let mut ids = std::collections::HashSet::new();
    for model in &existing.models {
        model.validate()?;
        require(ids.insert(model.id.as_str()), "duplicate prior catalog id")?;
    }
    let now = OffsetDateTime::parse(now, &Rfc3339)
        .map_err(|_| ValidationError("invalid enrichment clock".into()))?
        .to_offset(time::UtcOffset::UTC);
    let stamp = now
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .map_err(|_| ValidationError("invalid enrichment clock".into()))?;
    let today = &stamp[..10];
    let newest = existing
        .models
        .iter()
        .filter_map(|model| model.recency().map(|(day, _)| day))
        .max()
        .unwrap_or("");
    let mut result: BTreeMap<String, CatalogModel> = existing
        .models
        .iter()
        .map(|model| (model.id.clone(), model.clone()))
        .collect();
    let mut positions = BTreeMap::new();
    let mut unique: Vec<&RawModel> = Vec::new();
    for candidate in candidates {
        candidate.validate_shape()?;
        if let Some(index) = positions.get(&candidate.id) {
            unique[*index] = candidate;
        } else {
            positions.insert(candidate.id.clone(), unique.len());
            unique.push(candidate);
        }
    }
    let mut diff = EnrichDiff::default();
    for raw in unique {
        let prior = result.get(&raw.id);
        if !raw.open_weight || !LICENSES.contains(&raw.license.as_str()) {
            if result.remove(&raw.id).is_some() {
                diff.removed.push(raw.id.clone());
            } else if mode == Mode::Backfill {
                diff.skipped.push(raw.id.clone());
            }
            continue;
        }
        if mode == Mode::Incremental && prior.is_some() {
            continue;
        }
        if raw.release_date.as_str() > today {
            if prior.is_none() {
                diff.skipped.push(raw.id.clone());
            }
            continue;
        }
        if mode == Mode::Incremental && raw.release_date.as_str() <= newest {
            continue;
        }
        match raw.build(prior) {
            Ok(built) => {
                if let Some(prior) = prior {
                    let encoded = serde_json::to_value(&built)
                        .map_err(|error| ValidationError(error.to_string()))?;
                    let previous = serde_json::to_value(prior)
                        .map_err(|error| ValidationError(error.to_string()))?;
                    if encoded == previous {
                        continue;
                    }
                    diff.updated.push(raw.id.clone());
                } else {
                    diff.added.push(raw.id.clone());
                }
                result.insert(raw.id.clone(), built);
            }
            Err(_) => diff.skipped.push(raw.id.clone()),
        }
    }
    let mut models: Vec<_> = result.into_values().collect();
    models.sort_by(|left, right| {
        let day = |model: &CatalogModel| model.recency().map(|(day, _)| day.to_string());
        day(right)
            .cmp(&day(left))
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(maximum) = maximum
        && models.len() > maximum
    {
        diff.capped = models.drain(maximum..).map(|model| model.id).collect();
    }
    let changed = !diff.added.is_empty()
        || !diff.updated.is_empty()
        || !diff.removed.is_empty()
        || !diff.capped.is_empty();
    diff.added.retain(|id| !diff.capped.contains(id));
    diff.updated.retain(|id| !diff.capped.contains(id));
    Ok(EnrichResult {
        catalog: Catalog {
            schema_version: crate::catalog::SCHEMA_VERSION,
            generated_at: if changed {
                stamp
            } else {
                existing.generated_at.clone()
            },
            models,
        },
        diff,
    })
}
pub fn format_diff(diff: &EnrichDiff) -> String {
    let mut text = format!(
        "Refresh (dry-run):\n  added: {}\n  updated: {}\n  removed: {}\n  skipped: {}\n  capped: {}\n",
        diff.added.len(),
        diff.updated.len(),
        diff.removed.len(),
        diff.skipped.len(),
        diff.capped.len()
    );
    for (label, ids) in [
        ("added ids", &diff.added),
        ("updated ids", &diff.updated),
        ("removed ids", &diff.removed),
    ] {
        if !ids.is_empty() {
            let mut ids = ids.clone();
            ids.sort();
            text.push_str(&format!("  {label}: {}\n", ids.join(", ")));
        }
    }
    text.push_str("  No catalog file was written.\n\n");
    text
}
