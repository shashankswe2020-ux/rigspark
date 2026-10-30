use crate::admission::{AdmissionTransport, Fetched};
use rigspark_core::{
    admission::{Gguf, GgufError, active_params, kv_bytes_per_token, license_id, parse_gguf},
    catalog::{Catalog, CatalogModel, EntryProvenance, GgufSource, Source},
    catalog_request::Repository,
    enrich::sized_quantization,
    generation::GenerationCatalog,
    generation_admission::{HfModel, MAX_METADATA_BYTES, kind_from_pipeline, license_from_tags},
    sizing::Architecture,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{io, time::Duration};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;
use url::Url;

mod generation;

const GGUF_STEPS: [usize; 3] = [8 * 1024 * 1024, 32 * 1024 * 1024, 64 * 1024 * 1024];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestDisposition {
    Eligible,
    NeedsRunnableArtifact,
    NeedsWorkflow,
    NeedsMaintainer,
    Rejected,
    AlreadyAvailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestOutcome {
    pub schema_version: u8,
    pub repository: String,
    pub disposition: RequestDisposition,
    pub reason_code: &'static str,
    pub summary: String,
    pub model_ids: Vec<String>,
    pub evidence_urls: Vec<String>,
    pub changed_files: Vec<&'static str>,
    #[serde(skip)]
    pub catalog: Option<Catalog>,
    #[serde(skip)]
    pub observation: Option<Value>,
}

impl RequestOutcome {
    fn no_change(
        repository: &Repository,
        disposition: RequestDisposition,
        reason_code: &'static str,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: 1,
            repository: repository.as_str().into(),
            disposition,
            reason_code,
            summary: summary.into(),
            model_ids: Vec::new(),
            evidence_urls: vec![repository.url()],
            changed_files: Vec::new(),
            catalog: None,
            observation: None,
        }
    }
}

pub(crate) async fn get(
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
    raw: &str,
    range: Option<usize>,
    limit: usize,
) -> io::Result<Vec<u8>> {
    if cancel.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "catalog request cancelled",
        ));
    }
    let url = Url::parse(raw).map_err(io::Error::other)?;
    let Fetched { status, body } = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "catalog request cancelled"));
        }
        result = tokio::time::timeout(
            Duration::from_secs(180),
            transport.fetch(&url, None, range, limit),
        ) => result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "catalog request timed out"))??,
    };
    let accepted = if range.is_some() {
        matches!(status, 200 | 206)
    } else {
        (200..300).contains(&status)
    };
    if !accepted {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("upstream status {status}"),
        ));
    }
    Ok(body)
}

pub(crate) async fn gguf(
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
    url: &str,
) -> io::Result<Result<Gguf, &'static str>> {
    for step in GGUF_STEPS {
        let bytes = get(transport, cancel, url, Some(step), step).await?;
        match parse_gguf(&bytes) {
            Ok(parsed) => return Ok(Ok(parsed)),
            Err(GgufError::Truncated) if bytes.len() >= step => {}
            Err(GgufError::Truncated) => return Ok(Err("GGUF header truncated")),
            Err(GgufError::Invalid(reason)) => return Ok(Err(reason)),
        }
    }
    Ok(Err("GGUF header exceeds 64 MiB"))
}

pub async fn evaluate_request(
    catalog: &Catalog,
    generation_catalog: &GenerationCatalog,
    repository: &Repository,
    transport: &dyn AdmissionTransport,
    now: &str,
    cancel: &CancellationToken,
) -> io::Result<RequestOutcome> {
    if let Some(outcome) = generation::direct_match(generation_catalog, repository) {
        return Ok(outcome);
    }
    if catalog.models.iter().any(|model| {
        model.source.hf.as_deref() == Some(repository.as_str())
            || model
                .source
                .gguf
                .as_ref()
                .is_some_and(|source| source.repo == repository.as_str())
            || model
                .source
                .mlx
                .as_ref()
                .is_some_and(|source| source.repo == repository.as_str())
    }) {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::AlreadyAvailable,
            "ALREADY_AVAILABLE",
            "The requested repository is already represented in the catalog.",
        ));
    }

    let metadata_url = format!(
        "https://huggingface.co/api/models/{}?blobs=true",
        repository.as_str()
    );
    let metadata = get(transport, cancel, &metadata_url, None, MAX_METADATA_BYTES).await?;
    let model = HfModel::parse(
        std::str::from_utf8(&metadata)
            .map_err(|_| io::Error::other("Hugging Face metadata is not UTF-8"))?,
    )
    .map_err(io::Error::other)?;
    if model.id != repository.as_str() || !model.pinned() {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::Rejected,
            "PUBLIC_PINNED_REVISION_REQUIRED",
            "The repository must be public, ungated, and pinned by Hugging Face.",
        ));
    }
    if let Some(kind) = kind_from_pipeline(model.pipeline_tag.as_deref()) {
        return generation::evaluate(generation_catalog, repository, kind, transport, cancel).await;
    }
    if model.pipeline_tag.as_deref() != Some("text-generation") {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::Rejected,
            "UNSUPPORTED_PIPELINE",
            "The repository is not identified as text generation, text-to-image, or text-to-video.",
        ));
    }
    let Some(tag_license) = license_from_tags(&model.tags) else {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::Rejected,
            "OPEN_LICENSE_REQUIRED",
            "The repository does not declare one unambiguous supported open-weight license.",
        ));
    };
    let Some((file, artifact)) = model
        .siblings
        .iter()
        .filter_map(|sibling| {
            let lower = sibling.rfilename.to_ascii_lowercase();
            (lower.ends_with("q4_k_m.gguf")).then_some((sibling, sibling.lfs.as_ref()?))
        })
        .min_by_key(|(sibling, artifact)| (artifact.size, sibling.rfilename.as_str()))
    else {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::NeedsRunnableArtifact,
            "RUNNABLE_ARTIFACT_MISSING",
            "No pinned Q4_K_M GGUF artifact was published in the requested repository.",
        ));
    };
    if artifact.size == 0
        || artifact.size > 9_007_199_254_740_991
        || artifact.sha256.len() != 64
        || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::Rejected,
            "ARTIFACT_INTEGRITY_INVALID",
            "The selected GGUF artifact lacks valid size or SHA-256 evidence.",
        ));
    }
    let artifact_url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        repository.as_str(),
        model.sha,
        file.rfilename
    );
    let parsed = match gguf(transport, cancel, &artifact_url).await? {
        Ok(parsed) => parsed,
        Err(reason) => {
            return Ok(RequestOutcome::no_change(
                repository,
                RequestDisposition::Rejected,
                "GGUF_METADATA_INVALID",
                format!("The selected GGUF cannot be admitted: {reason}."),
            ));
        }
    };
    let today = today(now)?;
    let selected = SelectedGguf {
        file: &file.rfilename,
        sha256: &artifact.sha256,
        bytes: artifact.size,
        quant: "Q4_K_M",
    };
    let entry = match build_entry(repository, &model, &parsed, &selected, tag_license, &today) {
        Ok(entry) => entry,
        Err(reason) => {
            return Ok(RequestOutcome::no_change(
                repository,
                RequestDisposition::Rejected,
                "MODEL_FACTS_INCOMPLETE",
                reason,
            ));
        }
    };
    if catalog.models.iter().any(|model| model.id == entry.id) {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::Rejected,
            "MODEL_ID_CONFLICT",
            format!("The generated model id {} already exists.", entry.id),
        ));
    }

    let mut models = catalog.models.clone();
    let model_id = entry.id.clone();
    models.push(entry.clone());
    models.sort_by(|left, right| left.id.cmp(&right.id));
    let stamp = stamp(now)?;
    let encoded = json!({
        "schemaVersion": rigspark_core::catalog::required_schema_version(&models),
        "generatedAt": stamp,
        "models": models,
    });
    let updated = Catalog::parse(&encoded.to_string()).map_err(io::Error::other)?;
    let mut sources = vec![repository.url(), artifact_url];
    if let Some(base) = &entry.source.hf {
        let base_url = format!("https://huggingface.co/{base}");
        if !sources.contains(&base_url) {
            sources.push(base_url);
        }
    }
    let observation = json!({
        "id": entry.id,
        "checkedAt": stamp,
        "sources": sources,
        "model": entry,
    });
    Ok(RequestOutcome {
        schema_version: 1,
        repository: repository.as_str().into(),
        disposition: RequestDisposition::Eligible,
        reason_code: "ELIGIBLE",
        summary: format!("Validated source-backed catalog entry {model_id}."),
        model_ids: vec![model_id],
        evidence_urls: sources,
        changed_files: vec![
            "crates/rigspark-core/data/models.json",
            "docs/references/catalog-quality-evidence.json",
        ],
        catalog: Some(updated),
        observation: Some(observation),
    })
}

pub(crate) struct SelectedGguf<'a> {
    pub(crate) file: &'a str,
    pub(crate) sha256: &'a str,
    pub(crate) bytes: u64,
    pub(crate) quant: &'a str,
}

pub(crate) fn build_entry(
    repository: &Repository,
    model: &HfModel,
    gguf: &Gguf,
    artifact: &SelectedGguf<'_>,
    tag_license: &str,
    today: &str,
) -> Result<CatalogModel, String> {
    let architecture = gguf
        .architecture()
        .ok_or_else(|| "GGUF architecture is missing.".to_string())?;
    if let Some(raw) = gguf.text("general.license") {
        let gguf_license = license_id(Some(raw), gguf.text("general.license.name"), None)
            .map_err(|_| "GGUF license is not supported.".to_string())?;
        if gguf_license != tag_license {
            return Err("Hugging Face and GGUF license evidence disagree.".into());
        }
    }
    let context = gguf
        .u64(&format!("{architecture}.context_length"))
        .filter(|tokens| *tokens > 0)
        .ok_or_else(|| "GGUF context length is missing.".to_string())?;
    let total = gguf.total_elements();
    if total == 0 {
        return Err("GGUF tensor count is missing.".into());
    }
    let (architecture_kind, active_params) = if gguf
        .u64(&format!("{architecture}.expert_count"))
        .is_some_and(|count| count > 0)
    {
        let active = active_params(gguf)
            .ok_or_else(|| "MoE active parameter count is not sourced.".to_string())?;
        (Architecture::Moe, Some(parameter_label(active)))
    } else {
        (Architecture::Dense, None)
    };
    let quantization = sized_quantization(
        total as f64,
        &architecture_kind,
        artifact.quant,
        artifact.bytes as f64,
        Some(artifact.sha256.to_owned()),
        Vec::new(),
    )
    .map_err(|error| error.to_string())?;
    let source_hf = model
        .base_model()
        .unwrap_or_else(|| repository.as_str().to_owned());
    let id = format!(
        "{}:{}",
        source_hf.to_ascii_lowercase(),
        artifact.quant.to_ascii_lowercase()
    );
    Ok(CatalogModel {
        id,
        family: architecture.into(),
        params: parameter_label(total as f64),
        architecture: architecture_kind,
        active_params,
        license: tag_license.into(),
        open_weight: true,
        context_length: context as f64,
        capabilities: vec!["chat".into()],
        release_date: None,
        added_at: Some(today.into()),
        provenance: EntryProvenance::Auto,
        source: Source {
            hf: Some(source_hf),
            gguf: Some(GgufSource {
                repo: repository.as_str().into(),
                revision: model.sha.clone(),
                file: artifact.file.into(),
                sha256: artifact.sha256.into(),
            }),
            ..Default::default()
        },
        quantizations: vec![quantization],
        kv_bytes_per_token: kv_bytes_per_token(gguf),
        benchmark_proxy: None,
        drafter: None,
    })
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

fn clock(raw: &str) -> io::Result<OffsetDateTime> {
    OffsetDateTime::parse(raw, &Rfc3339)
        .map(|clock| clock.to_offset(time::UtcOffset::UTC))
        .map_err(|_| io::Error::other("invalid request clock"))
}

fn stamp(raw: &str) -> io::Result<String> {
    clock(raw)?
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .map_err(io::Error::other)
}

pub(crate) fn today(raw: &str) -> io::Result<String> {
    Ok(stamp(raw)?[..10].into())
}
