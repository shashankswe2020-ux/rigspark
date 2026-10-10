use super::{
    ArtifactIdentity, ArtifactObservation, IndexModel, MAX_IDENTITY_RECORDS, PublisherMapping,
    artifact_identity,
    coverage::{CoverageStatus, evaluate},
    validate_export_shards,
};
use crate::{
    catalog::{
        AdvisoryReason, Availability, Catalog, CatalogBackend, CatalogModel, EntryProvenance,
        LICENSES, PinnedFileSource, Source, require, timestamp,
    },
    sizing::{Architecture, Quantization, ValidationError},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Facts already resolved from official publisher metadata, not free-form leaderboard labels.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionArtifact {
    pub model_ids: Vec<String>,
    pub source: PinnedFileSource,
    pub license: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
}

fn validated(catalog: &Catalog) -> Result<Catalog, ValidationError> {
    let raw = serde_json::to_string(catalog).map_err(|error| ValidationError(error.to_string()))?;
    Catalog::parse(&raw)
}

pub fn admit(
    catalog: &Catalog,
    inventory: &[IndexModel],
    mappings: &[PublisherMapping],
    artifacts: &[AdmissionArtifact],
    now: &str,
) -> Result<Catalog, ValidationError> {
    timestamp(now)?;
    let catalog = validated(catalog)?;
    require(
        artifacts.len() <= MAX_IDENTITY_RECORDS,
        "too many admission artifacts",
    )?;
    let mut observations = Vec::new();
    let mut evidence: BTreeMap<ArtifactIdentity, AdmissionArtifact> = BTreeMap::new();
    for artifact in artifacts {
        require(
            LICENSES.contains(&artifact.license.as_str()),
            "unaccepted artifact license",
        )?;
        require(!artifact.model_ids.is_empty(), "artifact has no index rows")?;
        require(
            artifact
                .context_length
                .is_none_or(|value| (1..=9_007_199_254_740_991).contains(&value)),
            "invalid sourced context length",
        )?;
        artifact.source.validate_weights()?;
        validate_export_shards(&artifact.source)?;
        require(
            artifact
                .source
                .files
                .iter()
                .all(|file| file.file.ends_with(".gguf"))
                || artifact
                    .source
                    .files
                    .iter()
                    .all(|file| file.file.ends_with(".safetensors")),
            "admission export mixes weight formats",
        )?;
        let identity = artifact_identity(&artifact.source)?;
        let mut candidate = artifact.clone();
        candidate.source.revision = candidate.source.revision.to_ascii_lowercase();
        candidate
            .source
            .files
            .sort_by(|left, right| left.file.cmp(&right.file));
        for file in &mut candidate.source.files {
            file.sha256 = file.sha256.to_ascii_lowercase();
        }
        if let Some(previous) = evidence.get_mut(&identity) {
            require(
                previous.license == artifact.license
                    && previous.context_length == artifact.context_length,
                "conflicting evidence for identical artifact",
            )?;
            if candidate
                .source
                .files
                .iter()
                .map(|file| (&file.file, &file.sha256))
                .cmp(
                    previous
                        .source
                        .files
                        .iter()
                        .map(|file| (&file.file, &file.sha256)),
                )
                .is_lt()
            {
                *previous = candidate;
            }
        } else {
            evidence.insert(identity, candidate);
        }
        for id in &artifact.model_ids {
            require(
                observations.len() < MAX_IDENTITY_RECORDS,
                "too many admission observations",
            )?;
            observations.push(ArtifactObservation {
                model_id: id.clone(),
                source: artifact.source.clone(),
            });
        }
    }
    let coverage = evaluate(inventory, mappings, &observations, &catalog)?;
    require(
        coverage.unmatched.is_empty(),
        "unresolved index rows block catalog admission",
    )?;
    require(
        coverage.ambiguous_catalog_artifacts == 0,
        "ambiguous catalog artifacts block admission",
    )?;
    for artifact in &coverage.artifacts {
        let facts = evidence
            .get(&artifact.identity)
            .expect("observations originate from evidence");
        for id in &artifact.catalog_ids {
            let existing = catalog
                .models
                .iter()
                .find(|model| &model.id == id)
                .expect("coverage catalog id");
            require(
                existing.license == facts.license,
                "catalog and publisher licenses conflict",
            )?;
        }
    }
    if coverage.missing_catalog_artifacts == 0 {
        return Ok(catalog);
    }
    let mut result = catalog.clone();
    for model in &mut result.models {
        if model.availability.is_none() {
            // Preserve the existing default source rather than selecting a different backend.
            let backend = if model.source.ollama.is_some()
                && model
                    .quantizations
                    .iter()
                    .all(|quant| quant.sha256.is_some())
            {
                Some(CatalogBackend::Ollama)
            } else if model.source.ollama.is_none() && model.source.gguf.is_some() {
                Some(CatalogBackend::Llamacpp)
            } else if model.source.ollama.is_none() && model.source.mlx.is_some() {
                Some(CatalogBackend::Mlx)
            } else {
                None
            };
            let backend = backend.ok_or_else(|| {
                ValidationError(format!(
                    "v4 migration requires verified backend pins for {}",
                    model.id
                ))
            })?;
            model.availability = Some(Availability::Runnable { backend });
        }
    }
    let mut ids: BTreeSet<_> = result.models.iter().map(|model| model.id.clone()).collect();
    for entry in coverage
        .artifacts
        .iter()
        .filter(|entry| entry.status == CoverageStatus::MissingCatalog)
    {
        let facts = evidence.get(&entry.identity).expect("artifact evidence");
        let row = inventory
            .iter()
            .find(|row| row.id == entry.model_ids[0])
            .expect("validated inventory row");
        let base = format!(
            "aa-{}-{}-{}",
            row.release.slug,
            &entry.identity.revision[..12],
            &entry.identity.files[0].sha256[..16]
        );
        let mut id = base.clone();
        let mut suffix = 1_u32;
        while !ids.insert(id.clone()) {
            id = format!("{base}-{suffix}");
            suffix += 1;
        }
        let bytes = facts.source.validate_weights()?;
        let memory = bytes + (bytes * 0.15).ceil();
        result.models.push(CatalogModel {
            id,
            family: row.creator.slug.clone(),
            params: "unknown".into(),
            architecture: Architecture::Unknown,
            active_params: None,
            license: facts.license.clone(),
            open_weight: true,
            context_length: facts.context_length.map(|value| value as f64),
            capabilities: Vec::new(),
            release_date: None,
            added_at: Some(now[..10].into()),
            provenance: EntryProvenance::Auto,
            availability: Some(Availability::AdvisoryOnly {
                reason: AdvisoryReason::BackendSupportUnverified,
            }),
            source: Source {
                weights: Some(facts.source.clone()),
                ..Source::default()
            },
            quantizations: vec![Quantization {
                name: "unknown".into(),
                disk_bytes: bytes,
                min_ram_bytes: memory,
                min_vram_bytes: memory,
                sha256: None,
                digest_verified: None,
                projectors: Vec::new(),
            }],
            kv_bytes_per_token: None,
            benchmark_proxy: None,
        });
    }
    result.schema_version = 4;
    result.generated_at = now.into();
    let result = validated(&result)?;
    require(
        evaluate(inventory, mappings, &observations, &result)?.complete,
        "admission did not establish complete coverage",
    )?;
    Ok(result)
}
