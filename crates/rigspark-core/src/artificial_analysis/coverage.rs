use super::{
    ArtifactIdentity, ArtifactObservation, IndexModel, PublisherMapping, artifact_groups,
    artifact_identity, match_publishers,
};
use crate::{
    catalog::{Availability, Catalog, CatalogBackend, PinnedFile, PinnedFileSource, require},
    sizing::ValidationError,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageStatus {
    Runnable,
    AdvisoryOnly,
    MissingCatalog,
    AmbiguousCatalog,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnmatchedReason {
    MissingPublisher,
    UnresolvedArtifact,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmatchedRow {
    pub model_id: String,
    pub reason: UnmatchedReason,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactCoverage {
    pub identity: ArtifactIdentity,
    pub model_ids: Vec<String>,
    pub catalog_ids: Vec<String>,
    pub status: CoverageStatus,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub complete: bool,
    pub index_rows: usize,
    pub open_weights_rows: usize,
    pub unique_artifacts: usize,
    pub covered_runnable: usize,
    pub covered_advisory_only: usize,
    pub missing_catalog_artifacts: usize,
    pub ambiguous_catalog_artifacts: usize,
    pub unmatched_rows: usize,
    pub proprietary_rows: usize,
    pub collapsed_configurations: usize,
    pub proprietary_excluded: Vec<String>,
    pub unmatched: Vec<UnmatchedRow>,
    pub artifacts: Vec<ArtifactCoverage>,
}

/// Consumes validated catalog data and publisher observations, never display-name matches.
pub fn evaluate(
    rows: &[IndexModel],
    mappings: &[PublisherMapping],
    observations: &[ArtifactObservation],
    catalog: &Catalog,
) -> Result<Coverage, ValidationError> {
    let matched = match_publishers(rows, mappings)?;
    let groups = artifact_groups(rows, mappings, observations)?;
    let open_weights_rows = rows.len() - matched.excluded.len();
    require(
        open_weights_rows > 0,
        "Artificial Analysis inventory has no open-weight rows",
    )?;
    let mut catalog_artifacts: BTreeMap<ArtifactIdentity, BTreeMap<String, bool>> = BTreeMap::new();
    for model in &catalog.models {
        let advisory = model.is_advisory_only();
        let eligible = |backend| match model.availability {
            None | Some(Availability::AdvisoryOnly { .. }) => true,
            Some(Availability::Runnable { backend: selected }) => {
                selected == backend || selected == CatalogBackend::Lmstudio
            }
        };
        let mut sources = Vec::new();
        if let Some(weights) = &model.source.weights {
            sources.push(weights.clone());
        }
        if let Some(mlx) = &model.source.mlx
            && eligible(CatalogBackend::Mlx)
        {
            let mut weights = mlx.clone();
            weights
                .files
                .retain(|file| file.file.ends_with(".safetensors"));
            sources.push(weights);
        }
        if let Some(gguf) = &model.source.gguf
            && eligible(CatalogBackend::Llamacpp)
        {
            for quant in &model.quantizations {
                let matches = quant
                    .sha256
                    .as_ref()
                    .is_some_and(|sha| sha.eq_ignore_ascii_case(&gguf.sha256))
                    || (quant.sha256.is_none() && model.quantizations.len() == 1);
                if matches && quant.projectors.is_empty() {
                    sources.push(PinnedFileSource {
                        repo: gguf.repo.clone(),
                        revision: gguf.revision.clone(),
                        files: vec![PinnedFile {
                            file: gguf.file.clone(),
                            sha256: gguf.sha256.clone(),
                            bytes: quant.disk_bytes,
                        }],
                    });
                }
            }
        }
        for source in sources {
            catalog_artifacts
                .entry(artifact_identity(&source)?)
                .or_default()
                .insert(model.id.clone(), advisory);
        }
    }
    let missing_publishers: BTreeSet<_> = matched.unmatched.iter().collect();
    let mut row_artifacts: BTreeMap<&str, Vec<&ArtifactIdentity>> = BTreeMap::new();
    for group in &groups {
        for id in &group.model_ids {
            row_artifacts.entry(id).or_default().push(&group.identity);
        }
    }
    let mut unmatched: Vec<_> = rows
        .iter()
        .filter(|row| row.is_open_weights && !row_artifacts.contains_key(row.id.as_str()))
        .map(|row| UnmatchedRow {
            model_id: row.id.clone(),
            reason: if missing_publishers.contains(&row.id) {
                UnmatchedReason::MissingPublisher
            } else {
                UnmatchedReason::UnresolvedArtifact
            },
        })
        .collect();
    unmatched.sort_by(|left, right| left.model_id.cmp(&right.model_id));
    let signatures: BTreeSet<_> = row_artifacts.values().collect();
    let collapsed_configurations = row_artifacts.len() - signatures.len();
    let artifacts: Vec<_> = groups
        .into_iter()
        .map(|group| {
            let entries = catalog_artifacts.get(&group.identity);
            let catalog_ids: Vec<_> = entries
                .into_iter()
                .flat_map(|entries| entries.keys().cloned())
                .collect();
            let status = match entries {
                None => CoverageStatus::MissingCatalog,
                Some(entries) if entries.len() > 1 => CoverageStatus::AmbiguousCatalog,
                Some(entries) if entries.values().all(|advisory| *advisory) => {
                    CoverageStatus::AdvisoryOnly
                }
                Some(_) => CoverageStatus::Runnable,
            };
            ArtifactCoverage {
                identity: group.identity,
                model_ids: group.model_ids,
                catalog_ids,
                status,
            }
        })
        .collect();
    let covered_runnable = artifacts
        .iter()
        .filter(|artifact| artifact.status == CoverageStatus::Runnable)
        .count();
    let covered_advisory_only = artifacts
        .iter()
        .filter(|artifact| artifact.status == CoverageStatus::AdvisoryOnly)
        .count();
    Ok(Coverage {
        complete: unmatched.is_empty()
            && covered_runnable + covered_advisory_only == artifacts.len(),
        index_rows: rows.len(),
        open_weights_rows,
        unique_artifacts: artifacts.len(),
        covered_runnable,
        covered_advisory_only,
        missing_catalog_artifacts: artifacts
            .iter()
            .filter(|artifact| artifact.status == CoverageStatus::MissingCatalog)
            .count(),
        ambiguous_catalog_artifacts: artifacts
            .iter()
            .filter(|artifact| artifact.status == CoverageStatus::AmbiguousCatalog)
            .count(),
        unmatched_rows: unmatched.len(),
        proprietary_rows: matched.excluded.len(),
        collapsed_configurations,
        proprietary_excluded: matched.excluded,
        unmatched,
        artifacts,
    })
}
