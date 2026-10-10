use crate::admission::AdmissionTransport;
use rigspark_core::{
    artificial_analysis::{
        PublisherConfig, PublisherMapping, WeightFormat, parse_publisher_artifact,
    },
    catalog::PinnedFileSource,
    generation_admission::MAX_METADATA_BYTES,
    sizing::ValidationError,
};
use serde::Serialize;
use std::{io, time::Duration};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum PublisherResolutionError {
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
