//! Weekly admission of Comfy-Org image and video releases as fit-only generation entries.

use crate::admission::{AdmissionTransport, GENERATION_LISTING_URL};
use rigspark_core::{
    catalog::EntryProvenance,
    generation::{GENERATION_SCHEMA_VERSION, GenerationCatalog, GenerationModel},
    generation_admission::{
        FitOnlyInput, HfModel, MAX_METADATA_BYTES, MAX_SAFETENSORS_HEADER_BYTES, SafetensorsError,
        diffusion_files, fit_only_entries, kind_from_pipeline, license_from_tags, parse_listing,
        safetensors_params,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationAdmissionState {
    /// LFS sha256 of a diffusion file → exact parameter count from its safetensors header.
    #[serde(default)]
    pub params: BTreeMap<String, u64>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationOutcome {
    #[serde(skip)]
    pub catalog: Option<GenerationCatalog>,
    #[serde(skip)]
    pub state: GenerationAdmissionState,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub rejected: BTreeMap<String, String>,
    pub failures: Vec<String>,
    pub complete: bool,
}

async fn get(
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
    url: &str,
    range: Option<usize>,
    limit: usize,
) -> io::Result<Vec<u8>> {
    if cancel.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "generation admission cancelled",
        ));
    }
    let url = Url::parse(url).map_err(io::Error::other)?;
    let fetched = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "generation admission cancelled"));
        }
        result = tokio::time::timeout(Duration::from_secs(120), transport.fetch(&url, None, range, limit)) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "admission request timed out"))??
        }
    };
    let ok = match range {
        Some(_) => matches!(fetched.status, 200 | 206),
        None => (200..300).contains(&fetched.status),
    };
    if !ok {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("upstream status {}", fetched.status),
        ));
    }
    Ok(fetched.body)
}

async fn header_params(
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
    url: &str,
) -> io::Result<Result<u64, &'static str>> {
    let mut want = 64 * 1024;
    loop {
        let bytes = get(transport, cancel, url, Some(want), want).await?;
        match safetensors_params(&bytes) {
            Ok(params) => return Ok(Ok(params)),
            Err(SafetensorsError::Truncated(needed))
                if bytes.len() >= want && needed <= MAX_SAFETENSORS_HEADER_BYTES + 8 =>
            {
                want = needed as usize;
            }
            Err(SafetensorsError::Truncated(_)) => return Ok(Err("safetensors header truncated")),
            Err(SafetensorsError::Invalid(reason)) => return Ok(Err(reason)),
        }
    }
}

fn interrupted(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted
}

/// Admits fit-only entries for new Comfy-Org text-to-image and text-to-video releases.
/// Curated entries are never modified; a failed listing aborts without changes.
pub async fn admit_generation(
    catalog: &GenerationCatalog,
    transport: &dyn AdmissionTransport,
    now: &str,
    state: GenerationAdmissionState,
    cancel: &CancellationToken,
) -> io::Result<GenerationOutcome> {
    let now = OffsetDateTime::parse(now, &Rfc3339)
        .map_err(|_| io::Error::other("invalid admission clock"))?
        .to_offset(time::UtcOffset::UTC);
    let stamp = now
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .map_err(io::Error::other)?;
    let today = &stamp[..10];
    let listing = get(
        transport,
        cancel,
        GENERATION_LISTING_URL,
        None,
        MAX_METADATA_BYTES,
    )
    .await?;
    let repos = parse_listing(
        std::str::from_utf8(&listing).map_err(|_| io::Error::other("listing is not UTF-8"))?,
    )
    .map_err(io::Error::other)?;

    let mut outcome = GenerationOutcome {
        complete: true,
        state,
        ..Default::default()
    };
    let curated: Vec<&GenerationModel> = catalog
        .models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Curated)
        .collect();
    let curated_ids: BTreeSet<&str> = curated.iter().map(|model| model.id.as_str()).collect();
    let curated_digests: BTreeSet<&str> = curated
        .iter()
        .flat_map(|model| model.files.iter())
        .map(|file| file.sha256.as_str())
        .collect();
    let prior: BTreeMap<&str, &GenerationModel> = catalog
        .models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Auto)
        .map(|model| (model.id.as_str(), model))
        .collect();
    let mut admitted: BTreeMap<String, GenerationModel> = BTreeMap::new();
    let mut unknown_sources: BTreeSet<String> = BTreeSet::new();

    for repo in &repos {
        let model = match get(
            transport,
            cancel,
            &format!("https://huggingface.co/api/models/{repo}?blobs=true"),
            None,
            MAX_METADATA_BYTES,
        )
        .await
        {
            Ok(raw) => match std::str::from_utf8(&raw).ok().map(HfModel::parse) {
                Some(Ok(model)) => model,
                _ => {
                    outcome
                        .failures
                        .push(format!("{repo}: metadata unparseable"));
                    outcome.complete = false;
                    unknown_sources.insert(format!("https://huggingface.co/{repo}"));
                    continue;
                }
            },
            Err(error) if interrupted(&error) => return Err(error),
            Err(error) => {
                outcome.failures.push(format!("{repo}: {error}"));
                outcome.complete = false;
                unknown_sources.insert(format!("https://huggingface.co/{repo}"));
                continue;
            }
        };
        let pipeline = match model.base_model() {
            Some(base) => match get(
                transport,
                cancel,
                &format!("https://huggingface.co/api/models/{base}"),
                None,
                MAX_METADATA_BYTES,
            )
            .await
            {
                Ok(raw) => serde_json::from_slice::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|base| base["pipeline_tag"].as_str().map(str::to_string)),
                Err(error) if interrupted(&error) => return Err(error),
                Err(_) => None,
            },
            None => model.pipeline_tag.clone(),
        };
        let Some(kind) = kind_from_pipeline(pipeline.as_deref()) else {
            continue;
        };
        let Some(license) = license_from_tags(&model.tags) else {
            outcome
                .rejected
                .insert(repo.clone(), "license not recognised as open".into());
            continue;
        };
        if !model.pinned() {
            outcome
                .rejected
                .insert(repo.clone(), "not a public pinned revision".into());
            continue;
        }
        let mut params: BTreeMap<String, u64> = BTreeMap::new();
        let mut failed = false;
        for sibling in diffusion_files(&model) {
            let Some(lfs) = &sibling.lfs else { continue };
            if curated_digests.contains(lfs.sha256.as_str()) {
                continue;
            }
            if let Some(count) = outcome.state.params.get(&lfs.sha256) {
                params.insert(sibling.rfilename.clone(), *count);
                continue;
            }
            let url = format!(
                "https://huggingface.co/{repo}/resolve/{}/{}",
                model.sha, sibling.rfilename
            );
            match header_params(transport, cancel, &url).await {
                Ok(Ok(count)) => {
                    outcome.state.params.insert(lfs.sha256.clone(), count);
                    params.insert(sibling.rfilename.clone(), count);
                }
                Ok(Err(reason)) => {
                    outcome
                        .rejected
                        .insert(format!("{repo}/{}", sibling.rfilename), reason.into());
                }
                Err(error) if interrupted(&error) => return Err(error),
                Err(error) => {
                    outcome
                        .failures
                        .push(format!("{repo}/{}: {error}", sibling.rfilename));
                    failed = true;
                }
            }
        }
        if failed {
            outcome.complete = false;
            unknown_sources.insert(format!("https://huggingface.co/{repo}"));
        }
        let entries = fit_only_entries(&FitOnlyInput {
            model: &model,
            kind,
            license,
            params: &|path| params.get(path).copied(),
            today,
        })
        .map_err(io::Error::other)?;
        for mut entry in entries {
            if curated_ids.contains(entry.id.as_str()) {
                continue;
            }
            if let Some(previous) = prior.get(entry.id.as_str()) {
                entry.added_at = previous.added_at.clone();
                entry.workflow = previous.workflow;
                entry.workflow_source = previous.workflow_source.clone();
            }
            admitted.insert(entry.id.clone(), entry);
        }
    }

    // Keep prior auto entries whose repository could not be read this run.
    for (id, model) in &prior {
        if !admitted.contains_key(*id) && unknown_sources.contains(&model.source) {
            admitted.insert(id.to_string(), (*model).clone());
        }
    }
    for id in admitted.keys() {
        if !prior.contains_key(id.as_str()) {
            outcome.added.push(id.clone());
        }
    }
    for id in prior.keys() {
        if !admitted.contains_key(*id) {
            outcome.removed.push(id.to_string());
        }
    }
    let models: Vec<GenerationModel> = catalog
        .models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Curated)
        .cloned()
        .chain(admitted.into_values())
        .collect();
    let changed = models != catalog.models;
    let encoded = json!({
        "schemaVersion": GENERATION_SCHEMA_VERSION,
        "generatedAt": if changed { stamp.clone() } else { catalog.generated_at.clone() },
        "models": models,
    });
    outcome.catalog =
        Some(GenerationCatalog::parse(&encoded.to_string()).map_err(io::Error::other)?);
    Ok(outcome)
}
