//! Harvests official ComfyUI templates into pinned, unverified workflow candidates.
//!
//! Every template at one pinned `Comfy-Org/workflow_templates` commit is compiled
//! against each available device class's `/object_info`. Every model file is pinned
//! to an exact Hugging Face commit, SHA-256 and size, and each result is linked to the
//! catalog entries whose primary weights it runs. Candidates never run until real
//! hardware verification promotes them. Every rejection is reported with a reason.

use crate::{
    admission::AdmissionTransport,
    workflow_compile::{
        Compiled, MAX_TEMPLATE_BYTES, MediaFormat, ModelSlot as CompiledSlot, ObjectInfo, compile,
    },
};
use rigspark_core::{
    generation::{FileRole, GenerationCatalog, GenerationFile, GenerationKind, WorkflowId},
    generation_admission::{HfModel, MAX_METADATA_BYTES},
    workflow::{DeviceClass, Registry, WorkflowFile},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const TEMPLATES_REPOSITORY: &str = "Comfy-Org/workflow_templates";
pub const MAIN_COMMIT_URL: &str =
    "https://api.github.com/repos/Comfy-Org/workflow_templates/commits/main";
const MAX_INDEX_BYTES: usize = 8 << 20;
const MAX_TEMPLATES: usize = 2048;

pub struct HarvestInput<'a> {
    /// Templates commit to harvest; `None` resolves the current `main`.
    pub commit: Option<&'a str>,
    pub object_info: &'a [(DeviceClass, ObjectInfo)],
    pub registry: &'a Registry,
    pub catalog: &'a GenerationCatalog,
    /// Most candidates one run may propose.
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateReport {
    pub template: String,
    pub kind: GenerationKind,
    pub device_classes: Vec<DeviceClass>,
    pub weights_bytes: u64,
    /// Catalog entries whose primary weights this workflow would run.
    pub matches: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarvestReport {
    pub commit: String,
    pub candidates: BTreeMap<String, CandidateReport>,
    pub rejected: BTreeMap<String, String>,
    pub skipped: BTreeMap<String, String>,
    pub failures: Vec<String>,
    pub complete: bool,
}

#[derive(Debug)]
pub struct Harvest {
    pub report: HarvestReport,
    pub candidates: Vec<WorkflowFile>,
}

#[derive(Deserialize)]
struct Category {
    #[serde(default)]
    templates: Vec<IndexTemplate>,
}

#[derive(Deserialize)]
struct IndexTemplate {
    name: String,
    #[serde(default, rename = "openSource")]
    open_source: Option<bool>,
    #[serde(default, rename = "minComfyUIVersion")]
    min_comfyui_version: Option<String>,
}

async fn get(
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
    url: &str,
    accept: Option<&'static str>,
    limit: usize,
) -> io::Result<Vec<u8>> {
    let url = Url::parse(url).map_err(io::Error::other)?;
    let fetched = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "workflow harvest cancelled"));
        }
        result = tokio::time::timeout(Duration::from_secs(120), transport.fetch(&url, accept, None, limit)) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "harvest request timed out"))??
        }
    };
    if !(200..300).contains(&fetched.status) {
        return Err(io::Error::other(format!(
            "{} answered HTTP {}",
            url.path(),
            fetched.status
        )));
    }
    Ok(fetched.body)
}

fn interrupted(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_template(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Nodes that read user media (images, video, audio, latents) need inputs RigSpark does
/// not collect yet; model loaders (`*Loader*`) are fine.
pub fn needs_media_input(classes: &BTreeSet<String>) -> Option<&str> {
    classes
        .iter()
        .find(|class| class.starts_with("Load") && !class.contains("Loader"))
        .map(String::as_str)
}

/// Every node type a UI-format template declares, including inside subgraphs.
pub fn template_classes(text: &str) -> BTreeSet<String> {
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let mut classes = BTreeSet::new();
    let mut collect = |nodes: &serde_json::Value| {
        for node in nodes.as_array().into_iter().flatten() {
            if let Some(class) = node["type"].as_str() {
                classes.insert(class.to_owned());
            }
        }
    };
    collect(&value["nodes"]);
    for subgraph in value["definitions"]["subgraphs"]
        .as_array()
        .into_iter()
        .flatten()
    {
        collect(&subgraph["nodes"]);
    }
    classes
}

fn role(directory: &str) -> Option<FileRole> {
    match directory {
        "diffusion_models" | "unet" => Some(FileRole::Diffusion),
        "text_encoders" | "clip" => Some(FileRole::TextEncoder),
        "vae" => Some(FileRole::Vae),
        "checkpoints" => Some(FileRole::Checkpoint),
        "loras" => Some(FileRole::Lora),
        _ => None,
    }
}

enum Pin {
    Done(GenerationFile),
    Rejected(String),
    Failed(String),
}

/// Resolves a template manifest URL to an exact pinned Hugging Face file.
async fn pin(
    slot: &CompiledSlot,
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
    cache: &mut HashMap<String, HfModel>,
) -> io::Result<Pin> {
    let Some(role) = role(&slot.directory) else {
        return Ok(Pin::Rejected(format!(
            "model directory {} is not supported yet",
            slot.directory
        )));
    };
    let Ok(url) = Url::parse(&slot.url) else {
        return Ok(Pin::Rejected(format!("{} has an invalid URL", slot.name)));
    };
    let segments: Vec<&str> = url
        .path_segments()
        .map(Iterator::collect)
        .unwrap_or_default();
    let query_ok = matches!(url.query(), None | Some("download=true"));
    let [owner, repo, "resolve", reference, path @ ..] = segments.as_slice() else {
        return Ok(Pin::Rejected(format!(
            "{} is not a Hugging Face resolve URL",
            slot.name
        )));
    };
    if url.scheme() != "https"
        || url.host_str() != Some("huggingface.co")
        || !query_ok
        || path.is_empty()
        || path
            .iter()
            .any(|segment| segment.contains('%') || segment.is_empty())
    {
        return Ok(Pin::Rejected(format!(
            "{} is not a plain Hugging Face resolve URL",
            slot.name
        )));
    }
    let file = path.join("/");
    if path.last() != Some(&slot.name.as_str()) {
        return Ok(Pin::Rejected(format!(
            "manifest URL for {} names a different file",
            slot.name
        )));
    }
    if !file.ends_with(".safetensors") {
        return Ok(Pin::Rejected(format!(
            "{} is not a .safetensors file",
            slot.name
        )));
    }
    let metadata = match *reference {
        "main" => format!("https://huggingface.co/api/models/{owner}/{repo}?blobs=true"),
        commit if valid_commit(commit) => {
            format!("https://huggingface.co/api/models/{owner}/{repo}/revision/{commit}?blobs=true")
        }
        other => {
            return Ok(Pin::Rejected(format!(
                "{} uses unsupported revision {other}",
                slot.name
            )));
        }
    };
    let model = match cache.get(&metadata) {
        Some(model) => model.clone(),
        None => {
            let raw = match get(transport, cancel, &metadata, None, MAX_METADATA_BYTES).await {
                Ok(raw) => raw,
                Err(error) if interrupted(&error) => return Err(error),
                Err(error) => return Ok(Pin::Failed(format!("{owner}/{repo}: {error}"))),
            };
            let Some(Ok(model)) = std::str::from_utf8(&raw).ok().map(HfModel::parse) else {
                return Ok(Pin::Failed(format!("{owner}/{repo}: metadata unparseable")));
            };
            cache.insert(metadata.clone(), model.clone());
            model
        }
    };
    if !model.pinned() || (*reference != "main" && !model.sha.eq_ignore_ascii_case(reference)) {
        return Ok(Pin::Rejected(format!(
            "{owner}/{repo} is not a public pinned revision"
        )));
    }
    let Some(lfs) = model
        .siblings
        .iter()
        .find(|sibling| sibling.rfilename == file)
        .and_then(|sibling| sibling.lfs.as_ref())
    else {
        return Ok(Pin::Rejected(format!(
            "{file} is not published in {owner}/{repo}@{}",
            &model.sha[..7]
        )));
    };
    if lfs.sha256.len() != 64
        || !lfs.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || lfs.size == 0
    {
        return Ok(Pin::Rejected(format!("{file} has no valid LFS digest")));
    }
    Ok(Pin::Done(GenerationFile {
        role,
        folder: role.folder().into(),
        repo: format!("{owner}/{repo}"),
        revision: model.sha.to_ascii_lowercase(),
        file,
        sha256: lfs.sha256.to_ascii_lowercase(),
        bytes: lfs.size,
    }))
}

fn primary(role: FileRole) -> bool {
    matches!(role, FileRole::Diffusion | FileRole::Checkpoint)
}

/// Builds and validates the candidate workflow file for one compiled template.
fn candidate(
    id: &WorkflowId,
    template: &IndexTemplate,
    commit: &str,
    compiled: &Compiled,
    devices: &[DeviceClass],
    weights: &[GenerationFile],
) -> Result<WorkflowFile, String> {
    let (kind, format) = match compiled.output.format {
        MediaFormat::Png => ("image", "png"),
        MediaFormat::Webp => ("video", "webp"),
        MediaFormat::Mp4 => ("video", "mp4"),
    };
    let models: Vec<_> = compiled
        .models
        .iter()
        .zip(weights)
        .map(|(slot, weight)| {
            json!({"node": slot.node, "input": slot.input, "role": weight.role, "weight": weight})
        })
        .collect();
    let value = json!({
        "id": id,
        "kind": kind,
        "status": "candidate",
        "source": format!(
            "https://github.com/{TEMPLATES_REPOSITORY}/blob/{commit}/templates/{}.json",
            template.name
        ),
        "provenance": {
            "repository": TEMPLATES_REPOSITORY,
            "commit": commit,
            "template": template.name,
            "minComfyuiVersion": template.min_comfyui_version,
        },
        "deviceClasses": devices,
        "graph": compiled.graph,
        "bindings": {
            "prompt": [compiled.prompt],
            "seed": compiled.seeds,
            "output": {"node": compiled.output.node, "format": format},
        },
        "models": models,
    });
    let mut value = value;
    if template.min_comfyui_version.is_none() {
        value["provenance"]
            .as_object_mut()
            .map(|provenance| provenance.remove("minComfyuiVersion"));
    }
    WorkflowFile::parse(&value.to_string()).map_err(|error| error.0)
}

/// Harvests one pinned templates commit into workflow candidates and a full report.
pub async fn harvest(
    input: &HarvestInput<'_>,
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
) -> io::Result<Harvest> {
    if input.object_info.is_empty() {
        return Err(io::Error::other("harvest needs at least one object_info"));
    }
    let commit = match input.commit {
        Some(commit) => commit.to_ascii_lowercase(),
        None => {
            let raw = get(
                transport,
                cancel,
                MAIN_COMMIT_URL,
                Some("application/vnd.github.sha"),
                128,
            )
            .await?;
            String::from_utf8(raw)
                .map_err(|_| io::Error::other("templates commit is not text"))?
                .trim()
                .to_ascii_lowercase()
        }
    };
    if !valid_commit(&commit) {
        return Err(io::Error::other(
            "templates commit must be 40 hex characters",
        ));
    }
    let raw = |path: &str| {
        format!(
            "https://raw.githubusercontent.com/{TEMPLATES_REPOSITORY}/{commit}/templates/{path}"
        )
    };
    let index = get(transport, cancel, &raw("index.json"), None, MAX_INDEX_BYTES).await?;
    let categories: Vec<Category> = serde_json::from_slice(&index)
        .map_err(|error| io::Error::other(format!("index: {error}")))?;
    let mut templates: BTreeMap<String, IndexTemplate> = BTreeMap::new();
    for template in categories
        .into_iter()
        .flat_map(|category| category.templates)
    {
        templates.entry(template.name.clone()).or_insert(template);
    }
    if templates.len() > MAX_TEMPLATES {
        return Err(io::Error::other("template index is too large"));
    }

    let mut report = HarvestReport {
        commit: commit.clone(),
        complete: true,
        ..Default::default()
    };
    let mut candidates = Vec::new();
    let mut cache = HashMap::new();
    'templates: for (name, template) in &templates {
        if !valid_template(name) {
            report
                .rejected
                .insert(name.clone(), "invalid template name".into());
            continue;
        }
        if template.open_source == Some(false) {
            report.rejected.insert(
                name.clone(),
                "not open source (Partner API template)".into(),
            );
            continue;
        }
        if candidates.len() >= input.limit {
            report
                .skipped
                .insert(name.clone(), "candidate limit reached for this run".into());
            continue;
        }
        let body = match get(
            transport,
            cancel,
            &raw(&format!("{name}.json")),
            None,
            MAX_TEMPLATE_BYTES,
        )
        .await
        {
            Ok(body) => body,
            Err(error) if interrupted(&error) => return Err(error),
            Err(error) => {
                report.failures.push(format!("{name}: {error}"));
                continue;
            }
        };
        let Ok(text) = String::from_utf8(body) else {
            report
                .rejected
                .insert(name.clone(), "template is not UTF-8".into());
            continue;
        };
        if let Some(class) = needs_media_input(&template_classes(&text)) {
            report.rejected.insert(
                name.clone(),
                format!("needs a media input ({class}); only prompt-only workflows are harvested"),
            );
            continue;
        }
        let mut compiled: Option<Compiled> = None;
        let mut devices = Vec::new();
        let mut errors = Vec::new();
        for (device, info) in input.object_info {
            match compile(name, &text, info) {
                Ok(result) => {
                    if compiled
                        .as_ref()
                        .is_some_and(|first| first.graph != result.graph)
                    {
                        report.rejected.insert(
                            name.clone(),
                            "compiled graph differs by device class".into(),
                        );
                        continue 'templates;
                    }
                    devices.push(*device);
                    compiled.get_or_insert(result);
                }
                Err(error) => errors.push(format!("{} ({device:?})", error.reason)),
            }
        }
        let Some(compiled) = compiled else {
            report.rejected.insert(name.clone(), errors.join("; "));
            continue;
        };
        let mut weights = Vec::new();
        for slot in &compiled.models {
            match pin(slot, transport, cancel, &mut cache).await? {
                Pin::Done(file) => weights.push(file),
                Pin::Rejected(reason) => {
                    report.rejected.insert(name.clone(), reason);
                    continue 'templates;
                }
                Pin::Failed(reason) => {
                    report.failures.push(format!("{name}: {reason}"));
                    continue 'templates;
                }
            }
        }
        let id = WorkflowId::new(&format!(
            "{}-{}",
            name.to_ascii_lowercase().replace('_', "-"),
            &commit[..7]
        ))
        .map_err(|error| io::Error::other(error.0))?;
        let file = match candidate(&id, template, &commit, &compiled, &devices, &weights) {
            Ok(file) => file,
            Err(reason) => {
                report.rejected.insert(name.clone(), reason);
                continue;
            }
        };
        if let Some(existing) = input.registry.files().iter().find(|existing| {
            existing
                .provenance
                .as_ref()
                .is_some_and(|provenance| &provenance.template == name)
                && existing.graph == file.graph
                && existing.models == file.models
        }) {
            report
                .skipped
                .insert(name.clone(), format!("unchanged since {}", existing.id));
            continue;
        }
        let primaries: BTreeSet<&str> = weights
            .iter()
            .filter(|weight| primary(weight.role))
            .map(|weight| weight.sha256.as_str())
            .collect();
        let matches: Vec<String> = input
            .catalog
            .models
            .iter()
            .filter(|model| {
                model.kind == file.kind
                    && model.files.iter().any(|entry| {
                        primary(entry.role) && primaries.contains(entry.sha256.as_str())
                    })
            })
            .map(|model| model.id.clone())
            .collect();
        report.candidates.insert(
            id.to_string(),
            CandidateReport {
                template: name.clone(),
                kind: file.kind,
                device_classes: devices,
                weights_bytes: weights.iter().map(|weight| weight.bytes).sum(),
                matches,
            },
        );
        candidates.push(file);
    }
    report.complete = report.failures.is_empty();
    Ok(Harvest { report, candidates })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_classes_include_subgraph_nodes() {
        let text = r#"{"nodes":[{"type":"SaveImage"}],"definitions":{"subgraphs":[{"nodes":[{"type":"LoadImage"}]}]}}"#;
        let classes = template_classes(text);
        assert_eq!(needs_media_input(&classes), Some("LoadImage"));
        assert!(classes.contains("SaveImage"));
    }

    #[test]
    fn media_reading_nodes_are_detected_but_model_loaders_are_not() {
        let classes = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(
            needs_media_input(&classes(&["LoadImage", "UNETLoader"])),
            Some("LoadImage")
        );
        assert_eq!(
            needs_media_input(&classes(&["LoadAudio"])),
            Some("LoadAudio")
        );
        assert_eq!(
            needs_media_input(&classes(&["CheckpointLoaderSimple", "LoraLoaderModelOnly"])),
            None
        );
    }
}
