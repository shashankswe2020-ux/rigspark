use super::{RequestDisposition, RequestOutcome, get};
use crate::admission::AdmissionTransport;
use rigspark_core::{
    catalog_request::Repository,
    generation::{FileRole, GenerationCatalog, GenerationKind, GenerationModel},
    generation_admission::{HfModel, MAX_METADATA_BYTES, PUBLISHER},
};
use std::{collections::BTreeMap, io};
use tokio_util::sync::CancellationToken;

const MAX_PACKAGES: usize = 32;

fn primary_repo(model: &GenerationModel, repo: &str) -> bool {
    model.files.iter().any(|file| {
        matches!(file.role, FileRole::Diffusion | FileRole::Checkpoint) && file.repo == repo
    })
}

fn matched(
    repository: &Repository,
    models: &[&GenerationModel],
    evidence_urls: Vec<String>,
    lineage_only: bool,
) -> RequestOutcome {
    let runnable = models
        .iter()
        .filter(|model| model.runnable().is_ok())
        .count();
    let mut outcome = if runnable == 0 {
        RequestOutcome::no_change(
            repository,
            RequestDisposition::NeedsWorkflow,
            "GENERATION_WORKFLOW_MISSING",
            format!(
                "{} related generation variants are already cataloged as fit-only. \
                 RigSpark has no built-in ComfyUI workflow for these entries. \
                 Keep this request open. Repository-level lineage does not identify \
                 an individual variant in a multi-model package.",
                models.len()
            ),
        )
    } else if lineage_only {
        RequestOutcome::no_change(
            repository,
            RequestDisposition::NeedsMaintainer,
            "GENERATION_VARIANT_REVIEW_REQUIRED",
            "Pinned package metadata references the requested repository, but repository-level \
             lineage does not prove which runnable variant implements it. Keep this request \
             open for variant review.",
        )
    } else {
        RequestOutcome::no_change(
            repository,
            RequestDisposition::AlreadyAvailable,
            "ALREADY_AVAILABLE",
            format!(
                "{} matching generation variants are already cataloged; \
                 {runnable} have a built-in ComfyUI workflow.",
                models.len()
            ),
        )
    };
    outcome.model_ids = models.iter().map(|model| model.id.clone()).collect();
    outcome.model_ids.sort();
    outcome.evidence_urls = evidence_urls;
    outcome
}

pub(super) fn direct_match(
    catalog: &GenerationCatalog,
    repository: &Repository,
) -> Option<RequestOutcome> {
    let models: Vec<_> = catalog
        .models
        .iter()
        .filter(|model| {
            model.source == repository.url() || primary_repo(model, repository.as_str())
        })
        .collect();
    (!models.is_empty()).then(|| matched(repository, &models, vec![repository.url()], false))
}

pub(super) async fn evaluate(
    catalog: &GenerationCatalog,
    repository: &Repository,
    kind: GenerationKind,
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
) -> io::Result<RequestOutcome> {
    // Only the already-cataloged primary weights can establish a packaging relationship.
    // A shared text encoder or similar repository name is not evidence of the same model.
    let mut packages = BTreeMap::new();
    for model in catalog.models.iter().filter(|model| model.kind == kind) {
        for file in &model.files {
            if matches!(file.role, FileRole::Diffusion | FileRole::Checkpoint)
                && file
                    .repo
                    .split_once('/')
                    .is_some_and(|(owner, _)| owner == PUBLISHER)
            {
                packages
                    .entry((file.repo.as_str(), file.revision.as_str()))
                    .or_insert_with(Vec::new)
                    .push((model, file));
            }
        }
    }
    if packages.len() > MAX_PACKAGES {
        return Ok(RequestOutcome::no_change(
            repository,
            RequestDisposition::NeedsMaintainer,
            "GENERATION_LOOKUP_LIMIT",
            "Generation source lookup exceeds 32 pinned packages; maintainer review is required.",
        ));
    }
    let mut matches = BTreeMap::new();
    let mut evidence = vec![repository.url()];
    for ((repo, revision), entries) in packages {
        let url =
            format!("https://huggingface.co/api/models/{repo}/revision/{revision}?blobs=true");
        let raw = get(transport, cancel, &url, None, MAX_METADATA_BYTES).await?;
        let metadata = HfModel::parse(
            std::str::from_utf8(&raw)
                .map_err(|_| io::Error::other("generation source metadata is not UTF-8"))?,
        )
        .map_err(io::Error::other)?;
        if metadata.id != repo || metadata.sha != revision || !metadata.pinned() {
            return Err(io::Error::other(
                "generation source revision mismatch or inaccessible",
            ));
        }
        if !metadata
            .base_models()
            .iter()
            .any(|base| base == repository.as_str())
        {
            continue;
        }
        let mut found = false;
        for (model, file) in entries {
            if metadata.siblings.iter().any(|sibling| {
                sibling.rfilename == file.file
                    && sibling
                        .lfs
                        .as_ref()
                        .is_some_and(|lfs| lfs.sha256 == file.sha256 && lfs.size == file.bytes)
            }) {
                matches.insert(&model.id, model);
                found = true;
            }
        }
        if found {
            evidence.push(url);
        }
    }
    if !matches.is_empty() {
        return Ok(matched(
            repository,
            &matches.into_values().collect::<Vec<_>>(),
            evidence,
            true,
        ));
    }
    Ok(RequestOutcome::no_change(
        repository,
        RequestDisposition::NeedsMaintainer,
        "GENERATION_ADMISSION_REVIEW_REQUIRED",
        format!(
            "This is a {} model. No existing pinned primary generation artifact proves \
             coverage of this repository. Keep the request open for generation admission \
             review; text GGUF admission does not apply. No files were changed.",
            kind.name()
        ),
    ))
}
