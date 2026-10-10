use crate::admission::AdmissionTransport;
use rigspark_core::{
    artificial_analysis::{
        ArtifactObservation, INDEX_URL, IndexModel, MAX_IDENTITY_RECORDS, MAX_INDEX_HTML_BYTES,
        coverage::{Coverage, evaluate},
        match_publishers, parse_index_html,
    },
    catalog::{Catalog, require},
};
use rigspark_core::{
    artificial_analysis::{
        PublisherConfig, PublisherMapping, WeightFormat, parse_publisher_artifact,
    },
    catalog::PinnedFileSource,
    generation_admission::MAX_METADATA_BYTES,
    sizing::ValidationError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io, time::Duration};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublisherSelection {
    pub publisher: PublisherMapping,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventorySource {
    pub url: &'static str,
    pub checked_at: String,
    pub sha256: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublisherEvidence {
    pub model_ids: Vec<String>,
    pub artifact: ResolvedPublisherArtifact,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedCoverage {
    pub schema_version: u8,
    pub source: InventorySource,
    pub inventory: Vec<IndexModel>,
    pub evidence: Vec<PublisherEvidence>,
    #[serde(flatten)]
    pub coverage: Coverage,
}

pub async fn collect_coverage(
    catalog: &Catalog,
    selections: &[PublisherSelection],
    transport: &dyn AdmissionTransport,
    checked_at: &str,
    cancel: &CancellationToken,
) -> Result<CollectedCoverage, PublisherResolutionError> {
    require(
        checked_at.len() <= 64
            && checked_at.ends_with('Z')
            && OffsetDateTime::parse(checked_at, &Rfc3339).is_ok(),
        "invalid Artificial Analysis capture timestamp",
    )?;
    require(
        selections.len() <= MAX_IDENTITY_RECORDS,
        "too many publisher selections",
    )?;
    let mut ordered = BTreeMap::new();
    for selection in selections {
        selection.publisher.validate()?;
        require(
            (1..=256).contains(&selection.files.len()),
            "invalid publisher export selection size",
        )?;
        let mut files = selection.files.clone();
        files.sort();
        let publisher = &selection.publisher;
        ordered.insert(
            (
                &publisher.creator_slug,
                &publisher.release_slug,
                &publisher.repo,
                files,
            ),
            publisher,
        );
    }
    let bytes = fetch(
        transport,
        &Url::parse(INDEX_URL)?,
        MAX_INDEX_HTML_BYTES,
        cancel,
    )
    .await?;
    let raw = std::str::from_utf8(&bytes)
        .map_err(|_| ValidationError("Artificial Analysis inventory is not UTF-8".into()))?;
    let mut inventory = parse_index_html(raw)?;
    inventory.sort_by(|left, right| left.id.cmp(&right.id));
    let mappings: Vec<_> = ordered.values().map(|mapping| (*mapping).clone()).collect();
    let matched = match_publishers(&inventory, &mappings)?;
    let rows_by_release: BTreeMap<_, _> = matched
        .matched
        .iter()
        .map(|entry| {
            (
                (entry.creator_slug.as_str(), entry.release_slug.as_str()),
                &entry.model_ids,
            )
        })
        .collect();
    let mut observations = Vec::new();
    let mut evidence = Vec::new();
    for ((creator, release, _, files), publisher) in ordered {
        let Some(ids) = rows_by_release.get(&(creator.as_str(), release.as_str())) else {
            continue;
        };
        let artifact = resolve_publisher_artifact(publisher, &files, transport, cancel)
            .await
            .map_err(|source| PublisherResolutionError::PublisherSource {
                repo: publisher.repo.clone(),
                source: Box::new(source),
            })?;
        for id in *ids {
            require(
                observations.len() < MAX_IDENTITY_RECORDS,
                "too many resolved artifact observations",
            )?;
            observations.push(ArtifactObservation {
                model_id: id.clone(),
                source: artifact.source.clone(),
            });
        }
        evidence.push(PublisherEvidence {
            model_ids: (*ids).clone(),
            artifact,
        });
    }
    let coverage = evaluate(&inventory, &mappings, &observations, catalog)?;
    Ok(CollectedCoverage {
        schema_version: 1,
        source: InventorySource {
            url: INDEX_URL,
            checked_at: checked_at.into(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        },
        inventory,
        evidence,
        coverage,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum PublisherResolutionError {
    #[error("publisher {repo}: {source}")]
    PublisherSource {
        repo: String,
        source: Box<PublisherResolutionError>,
    },
    #[error("publisher resolution cancelled")]
    Cancelled,
    #[error("publisher request timed out")]
    Timeout,
    #[error("publisher request failed: {0}")]
    Transport(#[from] io::Error),
    #[error("publisher returned HTTP {0}")]
    Http(u16),
    #[error("publisher response exceeds {0} bytes")]
    Oversized(usize),
    #[error("invalid publisher evidence: {0}")]
    Invalid(#[from] ValidationError),
    #[error("invalid publisher URL: {0}")]
    Url(#[from] url::ParseError),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPublisherArtifact {
    pub source: PinnedFileSource,
    pub license: String,
    pub format: WeightFormat,
    pub metadata_url: String,
    pub config_url: Option<String>,
    pub model_type: Option<String>,
    pub context_length: Option<u64>,
}

/// Resolves an explicitly selected export from a reviewed publisher association.
/// Safetensors and GGUF evidence does not imply that any backend can run the artifact.
pub async fn resolve_publisher_artifact(
    mapping: &PublisherMapping,
    files: &[String],
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
) -> Result<ResolvedPublisherArtifact, PublisherResolutionError> {
    mapping.validate()?;
    let metadata_url = Url::parse(&format!(
        "https://huggingface.co/api/models/{}?blobs=true",
        mapping.repo
    ))?;
    let metadata = fetch(transport, &metadata_url, MAX_METADATA_BYTES, cancel).await?;
    let raw = std::str::from_utf8(&metadata)
        .map_err(|_| ValidationError("publisher metadata is not UTF-8".into()))?;
    let artifact = parse_publisher_artifact(mapping, files, raw)?;
    let (config_url, model_type, context_length) = if artifact.has_config {
        let url = Url::parse(&format!(
            "https://huggingface.co/{}/resolve/{}/config.json",
            artifact.source.repo, artifact.source.revision
        ))?;
        let bytes = fetch(transport, &url, PublisherConfig::MAX_BYTES, cancel).await?;
        let config = PublisherConfig::parse(&bytes)?;
        (Some(url.into()), config.model_type, config.context_length)
    } else {
        (None, None, None)
    };
    Ok(ResolvedPublisherArtifact {
        source: artifact.source,
        license: artifact.license,
        format: artifact.format,
        metadata_url: metadata_url.into(),
        config_url,
        model_type,
        context_length,
    })
}

async fn fetch(
    transport: &dyn AdmissionTransport,
    url: &Url,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, PublisherResolutionError> {
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(PublisherResolutionError::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(120), transport.fetch(url, None, None, limit)) => {
            result.map_err(|_| PublisherResolutionError::Timeout)??
        }
    };
    if response.status != 200 {
        return Err(PublisherResolutionError::Http(response.status));
    }
    if response.body.len() > limit {
        return Err(PublisherResolutionError::Oversized(limit));
    }
    Ok(response.body)
}
