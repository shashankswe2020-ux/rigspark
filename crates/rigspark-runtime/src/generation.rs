//! Local image/video generation through an attached, loopback-only ComfyUI server.
//!
//! Only built-in workflows composed of allowlisted core nodes are ever submitted, so paid
//! ComfyUI Partner (API) nodes and cloud services can never be reached through rigspark.
use crate::{
    acquire::{Acquisition, Artifact, DownloadTransport},
    http::{HttpError, Request, Transport, read_json},
};
use rigspark_core::generation::{
    FileRole, FitVerdict, GenerationCatalog, GenerationFit, GenerationKind, GenerationModel,
    Workflow, fit,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_PORT: u16 = 8188;
pub const MAX_PROMPT_BYTES: usize = 16 * 1024;
pub const MAX_SEED: u64 = 9_007_199_254_740_991;
const MAX_OUTPUT_BYTES: u64 = 1024 * 1024 * 1024;
const WEIGHT_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
/// Core ComfyUI nodes used by the built-in workflows; anything else is refused.
pub const LOCAL_NODES: [&str; 21] = [
    "BasicGuider",
    "BasicScheduler",
    "CheckpointLoaderSimple",
    "CLIPLoader",
    "CLIPTextEncode",
    "DualCLIPLoader",
    "EmptyHunyuanLatentVideo",
    "EmptyLatentImage",
    "EmptySD3LatentImage",
    "KSampler",
    "KSamplerSelect",
    "ModelSamplingAuraFlow",
    "ModelSamplingSD3",
    "RandomNoise",
    "SamplerCustomAdvanced",
    "SaveAnimatedWEBP",
    "SaveImage",
    "UNETLoader",
    "VAEDecode",
    "VAELoader",
    "Wan22ImageToVideoLatent",
];
/// Wan 2.1's published default negative prompt (Apache-2.0), as used by the official
/// ComfyUI example: https://comfyanonymous.github.io/ComfyUI_examples/wan/
const WAN_NEGATIVE_PROMPT: &str = "色调艳丽，过曝，静态，细节模糊不清，字幕，风格，作品，画作，画面，静止，整体发灰，最差质量，低质量，JPEG压缩残留，丑陋的，残缺的，多余的手指，画得不好的手部，画得不好的脸部，畸形的，毁容的，形态畸形的肢体，手指融合，静止不动的画面，杂乱的背景，三条腿，背景人很多，倒着走";

#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error("invalid generation request: {0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Catalog(String),
    #[error("{0}")]
    NoFit(String),
    #[error("hardware detection failed: {0}")]
    Hardware(String),
    #[error(
        "ComfyUI is not reachable at {0}; start ComfyUI on loopback (default port 8188) and retry"
    )]
    Unreachable(String),
    #[error("weight installation failed: {0}")]
    Install(String),
    #[error(
        "ComfyUI does not list {0}; make sure --comfyui-dir points at the running ComfyUI installation"
    )]
    NotVisible(String),
    #[error("workflow node `{0}` is not an allowlisted local node; refusing to submit")]
    NonLocalNode(String),
    #[error("ComfyUI rejected the workflow: {0}")]
    Rejected(String),
    #[error("ComfyUI generation failed: {0}")]
    Failed(String),
    #[error("invalid ComfyUI response: {0}")]
    Response(&'static str),
    #[error("output already exists: {0}")]
    Exists(PathBuf),
    #[error("could not write output: {0}")]
    Output(String),
    #[error("generation timed out")]
    Timeout,
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Http(HttpError),
}
impl From<HttpError> for GenerationError {
    fn from(error: HttpError) -> Self {
        match error {
            HttpError::Cancelled => Self::Cancelled,
            other => Self::Http(other),
        }
    }
}

pub fn output_extension(kind: GenerationKind) -> &'static str {
    match kind {
        GenerationKind::Image => "png",
        GenerationKind::Video => "webp",
    }
}
fn save_node(workflow: Workflow) -> &'static str {
    match workflow {
        Workflow::FluxCheckpoint | Workflow::FluxSplit => "9",
        Workflow::WanT2v | Workflow::Wan22Ti2v => "28",
        Workflow::QwenImage => "60",
    }
}

/// Random seed in the JSON-safe integer range, reported back so runs are reproducible.
pub fn random_seed() -> u64 {
    uuid::Uuid::new_v4().as_u64_pair().0 & MAX_SEED
}

pub fn validate_prompt(prompt: &str) -> Result<(), GenerationError> {
    if prompt.trim().is_empty()
        || prompt.len() > MAX_PROMPT_BYTES
        || prompt
            .chars()
            .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err(GenerationError::Invalid(
            "prompt must be 1..=16384 bytes of text without control characters",
        ));
    }
    Ok(())
}

/// ComfyUI reports its compute device per `/system_stats`; Apple GPUs report `mps`.
pub fn apple_mps(devices: &[String]) -> bool {
    devices.iter().any(|device| device == "mps")
}

/// Wan's official `uni_pc` sampler diverged into noise on Apple MPS in live testing (ComfyUI
/// 0.38, 30 steps), while `euler` at the official size and length stayed coherent.
fn wan_sampler(devices: &[String]) -> &'static str {
    if apple_mps(devices) {
        "euler"
    } else {
        "uni_pc"
    }
}

/// Builds the API-format graph published in the official ComfyUI examples.
pub fn workflow(
    model: &GenerationModel,
    names: &HashMap<FileRole, String>,
    prompt: &str,
    seed: u64,
    devices: &[String],
) -> Result<Value, GenerationError> {
    let name = |role| {
        names
            .get(&role)
            .cloned()
            .ok_or(GenerationError::Invalid("missing installed weight name"))
    };
    let workflow = model
        .runnable()
        .map_err(|error| GenerationError::Catalog(error.to_string()))?;
    Ok(match workflow {
        // https://comfyanonymous.github.io/ComfyUI_examples/flux/ (schnell fp8 checkpoint)
        Workflow::FluxCheckpoint => json!({
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": prompt, "clip": ["30", 1]}},
            "8": {"class_type": "VAEDecode", "inputs": {"samples": ["31", 0], "vae": ["30", 2]}},
            "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "rigspark", "images": ["8", 0]}},
            "27": {"class_type": "EmptySD3LatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}},
            "30": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": name(FileRole::Checkpoint)?}},
            "31": {"class_type": "KSampler", "inputs": {
                "seed": seed, "steps": 4, "cfg": 1.0, "sampler_name": "euler", "scheduler": "simple",
                "denoise": 1.0, "model": ["30", 0], "positive": ["6", 0], "negative": ["33", 0],
                "latent_image": ["27", 0]}},
            "33": {"class_type": "CLIPTextEncode", "inputs": {"text": "", "clip": ["30", 1]}},
        }),
        // https://comfyanonymous.github.io/ComfyUI_examples/flux/ (regular full version)
        Workflow::FluxSplit => json!({
            "5": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": prompt, "clip": ["11", 0]}},
            "8": {"class_type": "VAEDecode", "inputs": {"samples": ["13", 0], "vae": ["10", 0]}},
            "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "rigspark", "images": ["8", 0]}},
            "10": {"class_type": "VAELoader", "inputs": {"vae_name": name(FileRole::Vae)?}},
            "11": {"class_type": "DualCLIPLoader", "inputs": {
                "clip_name1": name(FileRole::TextEncoder)?,
                "clip_name2": name(FileRole::Clip)?,
                "type": "flux"}},
            "12": {"class_type": "UNETLoader", "inputs": {
                "unet_name": name(FileRole::Diffusion)?, "weight_dtype": "default"}},
            "13": {"class_type": "SamplerCustomAdvanced", "inputs": {
                "noise": ["25", 0], "guider": ["22", 0], "sampler": ["16", 0],
                "sigmas": ["17", 0], "latent_image": ["5", 0]}},
            "16": {"class_type": "KSamplerSelect", "inputs": {"sampler_name": "euler"}},
            "17": {"class_type": "BasicScheduler", "inputs": {
                "scheduler": "simple", "steps": 4, "denoise": 1.0, "model": ["12", 0]}},
            "22": {"class_type": "BasicGuider", "inputs": {
                "model": ["12", 0], "conditioning": ["6", 0]}},
            "25": {"class_type": "RandomNoise", "inputs": {"noise_seed": seed}},
        }),
        // https://comfyanonymous.github.io/ComfyUI_examples/wan/ (text to video, 1.3B)
        Workflow::WanT2v => json!({
            "3": {"class_type": "KSampler", "inputs": {
                "seed": seed, "steps": 30, "cfg": 6.0, "sampler_name": wan_sampler(devices), "scheduler": "simple",
                "denoise": 1.0, "model": ["48", 0], "positive": ["6", 0], "negative": ["7", 0],
                "latent_image": ["40", 0]}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": prompt, "clip": ["38", 0]}},
            "7": {"class_type": "CLIPTextEncode", "inputs": {"text": WAN_NEGATIVE_PROMPT, "clip": ["38", 0]}},
            "8": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["39", 0]}},
            "28": {"class_type": "SaveAnimatedWEBP", "inputs": {
                "filename_prefix": "rigspark", "fps": 16.0, "lossless": false, "quality": 90,
                "method": "default", "images": ["8", 0]}},
            "37": {"class_type": "UNETLoader", "inputs": {"unet_name": name(FileRole::Diffusion)?, "weight_dtype": "default"}},
            "38": {"class_type": "CLIPLoader", "inputs": {"clip_name": name(FileRole::TextEncoder)?, "type": "wan", "device": "default"}},
            "39": {"class_type": "VAELoader", "inputs": {"vae_name": name(FileRole::Vae)?}},
            "40": {"class_type": "EmptyHunyuanLatentVideo", "inputs": {"width": 832, "height": 480, "length": 49, "batch_size": 1}},
            "48": {"class_type": "ModelSamplingSD3", "inputs": {"shift": 8.0, "model": ["37", 0]}},
        }),
        // https://comfyanonymous.github.io/ComfyUI_examples/wan22/ (5B text to video)
        Workflow::Wan22Ti2v => json!({
            "3": {"class_type": "KSampler", "inputs": {
                "seed": seed, "steps": 30, "cfg": 5.0, "sampler_name": wan_sampler(devices), "scheduler": "simple",
                "denoise": 1.0, "model": ["48", 0], "positive": ["6", 0], "negative": ["7", 0],
                "latent_image": ["55", 0]}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": prompt, "clip": ["38", 0]}},
            "7": {"class_type": "CLIPTextEncode", "inputs": {"text": WAN_NEGATIVE_PROMPT, "clip": ["38", 0]}},
            "8": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["39", 0]}},
            "28": {"class_type": "SaveAnimatedWEBP", "inputs": {
                "filename_prefix": "rigspark", "fps": 24.0, "lossless": false, "quality": 90,
                "method": "default", "images": ["8", 0]}},
            "37": {"class_type": "UNETLoader", "inputs": {"unet_name": name(FileRole::Diffusion)?, "weight_dtype": "default"}},
            "38": {"class_type": "CLIPLoader", "inputs": {"clip_name": name(FileRole::TextEncoder)?, "type": "wan", "device": "default"}},
            "39": {"class_type": "VAELoader", "inputs": {"vae_name": name(FileRole::Vae)?}},
            "48": {"class_type": "ModelSamplingSD3", "inputs": {"shift": 8.0, "model": ["37", 0]}},
            "55": {"class_type": "Wan22ImageToVideoLatent", "inputs": {
                "width": 1280, "height": 704, "length": 41, "batch_size": 1, "vae": ["39", 0]}},
        }),
        // https://comfyanonymous.github.io/ComfyUI_examples/qwen_image/ (basic workflow)
        Workflow::QwenImage => json!({
            "3": {"class_type": "KSampler", "inputs": {
                "seed": seed, "steps": 20, "cfg": 2.5, "sampler_name": "euler", "scheduler": "simple",
                "denoise": 1.0, "model": ["66", 0], "positive": ["6", 0], "negative": ["7", 0],
                "latent_image": ["58", 0]}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": prompt, "clip": ["38", 0]}},
            "7": {"class_type": "CLIPTextEncode", "inputs": {"text": " ", "clip": ["38", 0]}},
            "8": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["39", 0]}},
            "37": {"class_type": "UNETLoader", "inputs": {"unet_name": name(FileRole::Diffusion)?, "weight_dtype": "default"}},
            "38": {"class_type": "CLIPLoader", "inputs": {"clip_name": name(FileRole::TextEncoder)?, "type": "qwen_image", "device": "default"}},
            "39": {"class_type": "VAELoader", "inputs": {"vae_name": name(FileRole::Vae)?}},
            "58": {"class_type": "EmptySD3LatentImage", "inputs": {"width": 1328, "height": 1328, "batch_size": 1}},
            "60": {"class_type": "SaveImage", "inputs": {"filename_prefix": "rigspark", "images": ["8", 0]}},
            "66": {"class_type": "ModelSamplingAuraFlow", "inputs": {"shift": 3.1, "model": ["37", 0]}},
        }),
    })
}

/// Fails closed unless every node is an allowlisted core local node.
pub fn ensure_local_only(workflow: &Value) -> Result<(), GenerationError> {
    let nodes = workflow
        .as_object()
        .filter(|nodes| !nodes.is_empty() && nodes.len() <= 64)
        .ok_or(GenerationError::Invalid(
            "workflow must be a non-empty node map",
        ))?;
    for node in nodes.values() {
        let class = node
            .get("class_type")
            .and_then(Value::as_str)
            .ok_or(GenerationError::Invalid("workflow node has no class_type"))?;
        if !LOCAL_NODES.contains(&class) || !node.get("inputs").is_some_and(Value::is_object) {
            return Err(GenerationError::NonLocalNode(
                class.chars().take(128).collect(),
            ));
        }
    }
    Ok(())
}

pub struct GenerationRequest<'request> {
    pub model: &'request GenerationModel,
    pub prompt: &'request str,
    pub seed: u64,
    pub endpoint: &'request str,
    pub comfyui_dir: &'request Path,
    /// Exact output file; when absent a name is derived inside `output_dir`.
    pub output: Option<&'request Path>,
    pub output_dir: &'request Path,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledWeight {
    pub role: FileRole,
    pub name: String,
    pub bytes: u64,
    pub cached: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationOutcome {
    pub model: String,
    pub kind: GenerationKind,
    pub path: PathBuf,
    pub bytes: u64,
    pub prompt_id: String,
    pub seed: u64,
    pub weights: Vec<InstalledWeight>,
}

pub type Events = Arc<dyn Fn(String) + Send + Sync>;

/// What ComfyUI reports about itself before a run.
#[derive(Debug, Clone, Default)]
pub struct ComfyStatus {
    pub devices: Vec<String>,
    /// Smallest free device memory, falling back to free system RAM; `None` when unreported.
    pub free_bytes: Option<u64>,
}

/// Weight file names a workflow graph loads, with `\` normalised to `/`.
fn model_files(graph: &Value) -> std::collections::BTreeSet<String> {
    graph
        .as_object()
        .into_iter()
        .flat_map(|nodes| nodes.values())
        .filter_map(|node| node.get("inputs").and_then(Value::as_object))
        .flat_map(|inputs| inputs.values())
        .filter_map(Value::as_str)
        .filter(|value| value.ends_with(".safetensors"))
        .map(|value| value.replace('\\', "/"))
        .collect()
}

pub struct ComfyUi<'runtime> {
    pub http: &'runtime dyn Transport,
    pub download: &'runtime dyn DownloadTransport,
    pub poll_interval: Duration,
    pub deadline: Duration,
    pub events: Option<Events>,
}

fn prompt_id(value: &Value) -> Result<String, GenerationError> {
    value
        .get("prompt_id")
        .and_then(Value::as_str)
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .map(str::to_owned)
        .ok_or(GenerationError::Response("missing or invalid prompt_id"))
}
fn summary(value: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        parts.push(message.to_owned());
    }
    if let Some(nodes) = value.get("node_errors").and_then(Value::as_object) {
        for (node, detail) in nodes.iter().take(8) {
            let class = detail
                .get("class_type")
                .and_then(Value::as_str)
                .unwrap_or("?");
            for error in detail
                .get("errors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(4)
            {
                let text = error
                    .get("details")
                    .or_else(|| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                parts.push(format!("node {node} ({class}): {text}"));
            }
        }
    }
    if let Some(message) = value.get("exception_message").and_then(Value::as_str) {
        let node = value
            .get("node_type")
            .and_then(Value::as_str)
            .unwrap_or("?");
        parts.push(format!("{node}: {message}"));
    }
    let text = if parts.is_empty() {
        "no details reported".to_owned()
    } else {
        parts.join("; ")
    };
    text.chars()
        .filter(|ch| !ch.is_control())
        .take(1000)
        .collect()
}
fn safe_output_name(value: &str, extension: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('.')
        && !value.contains(['/', '\\', ':'])
        && !value.chars().any(char::is_control)
        && value
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case(extension))
}
fn safe_subfolder(value: &str) -> bool {
    value.is_empty()
        || (value.len() <= 255
            && !value.starts_with('/')
            && !value.contains(['\\', ':'])
            && !value.chars().any(char::is_control)
            && value
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."))
}
fn has_magic(kind: GenerationKind, head: &[u8]) -> bool {
    match kind {
        GenerationKind::Image => head.starts_with(b"\x89PNG\r\n\x1a\n"),
        GenerationKind::Video => {
            head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP"
        }
    }
}

impl ComfyUi<'_> {
    fn emit(&self, message: String) {
        if let Some(events) = &self.events {
            events(message);
        }
    }

    async fn get_json(
        &self,
        endpoint: &str,
        path: &str,
        cancel: &CancellationToken,
        limit: usize,
    ) -> Result<Value, GenerationError> {
        let request = Request::new(endpoint, path, None, None)?;
        Ok(read_json(self.http, request, cancel, limit).await?)
    }

    /// Reads a JSON body regardless of status so ComfyUI validation errors can be shown.
    async fn post_json(
        &self,
        endpoint: &str,
        path: &str,
        body: Value,
        cancel: &CancellationToken,
    ) -> Result<(u16, Value), GenerationError> {
        let request = Request::new(endpoint, path, Some(body), None)?;
        let operation = async {
            let response = self.http.send(request).await?;
            let mut bytes = Vec::new();
            response
                .body
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| HttpError::Transport)?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err(HttpError::Limit);
            }
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).map_err(|_| HttpError::Response)?
            };
            Ok((response.status, value))
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(GenerationError::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(120), operation) =>
                Ok(result.map_err(|_| HttpError::Timeout)??),
        }
    }

    /// Confirms the listener is ComfyUI and returns its reported devices and free memory.
    pub async fn ready(
        &self,
        endpoint: &str,
        cancel: &CancellationToken,
    ) -> Result<ComfyStatus, GenerationError> {
        match self
            .get_json(endpoint, "/system_stats", cancel, 1024 * 1024)
            .await
        {
            Ok(stats) if stats.get("system").is_some_and(Value::is_object) => {
                let devices: Vec<&Value> = stats
                    .get("devices")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .take(16)
                    .collect();
                let free_bytes = devices
                    .iter()
                    .filter_map(|device| device.get("vram_free").and_then(Value::as_u64))
                    .min()
                    .or_else(|| stats.pointer("/system/ram_free").and_then(Value::as_u64));
                Ok(ComfyStatus {
                    devices: devices
                        .iter()
                        .filter_map(|device| device.get("type").and_then(Value::as_str))
                        .filter(|kind| {
                            kind.len() <= 32
                                && kind.bytes().all(|byte| byte.is_ascii_alphanumeric())
                        })
                        .map(str::to_owned)
                        .collect(),
                    free_bytes,
                })
            }
            Ok(_) => Err(GenerationError::Response("listener is not ComfyUI")),
            Err(GenerationError::Cancelled) => Err(GenerationError::Cancelled),
            Err(GenerationError::Http(HttpError::Invalid)) => Err(GenerationError::Invalid(
                "ComfyUI endpoint must be loopback HTTP",
            )),
            Err(_) => Err(GenerationError::Unreachable(endpoint.to_owned())),
        }
    }

    /// Installs (or re-verifies) every pinned weight under `<comfyui>/models/<folder>/rigspark`.
    pub async fn install(
        &self,
        model: &GenerationModel,
        comfyui_dir: &Path,
        cancel: &CancellationToken,
    ) -> Result<Vec<InstalledWeight>, GenerationError> {
        let models = comfyui_dir.join("models");
        if !std::fs::metadata(&models).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(GenerationError::Invalid(
                "ComfyUI directory must contain a models/ folder",
            ));
        }
        let workflow = model
            .runnable()
            .map_err(|error| GenerationError::Catalog(error.to_string()))?;
        let mut installed = Vec::new();
        for role in workflow.roles() {
            let file = model
                .file(*role)
                .ok_or(GenerationError::Invalid("model is missing a workflow file"))?;
            let artifact = Artifact {
                backend: "comfyui".into(),
                repo: file.repo.clone(),
                revision: file.revision.clone(),
                file: file.file.clone(),
                sha256: file.sha256.clone(),
                bytes: file.bytes,
            };
            let (owner, repository) = file
                .repo
                .split_once('/')
                .ok_or(GenerationError::Invalid("invalid weight repository"))?;
            let name = format!(
                "rigspark/comfyui/{owner}/{repository}@{}/{}",
                file.revision, file.file
            );
            let mut acquisition = Acquisition::new(models.join(role.folder()).join("rigspark"))
                .map_err(GenerationError::Install)?
                .with_timeout(WEIGHT_TIMEOUT);
            if let Some(events) = self.events.clone() {
                let last = Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
                acquisition = acquisition.with_progress(move |done, total, file| {
                    let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
                    if last.swap(percent / 5, std::sync::atomic::Ordering::Relaxed) != percent / 5 {
                        events(format!(
                            "Weights {file}: {percent}% of {:.1} GiB",
                            total as f64 / 1073741824.0
                        ));
                    }
                });
            }
            let acquired = acquisition
                .acquire(&artifact, self.download, cancel)
                .await
                .map_err(|error| {
                    if cancel.is_cancelled() {
                        GenerationError::Cancelled
                    } else {
                        GenerationError::Install(error)
                    }
                })?;
            if !acquired
                .path
                .ends_with(name.trim_start_matches("rigspark/"))
            {
                return Err(GenerationError::Install(
                    "unexpected weight location".into(),
                ));
            }
            installed.push(InstalledWeight {
                role: *role,
                name,
                bytes: acquired.bytes,
                cached: acquired.cached,
            });
        }
        Ok(installed)
    }

    /// Replaces each expected name with ComfyUI's own spelling (e.g. `\` on Windows).
    async fn resolve_names(
        &self,
        endpoint: &str,
        weights: &mut [InstalledWeight],
        cancel: &CancellationToken,
    ) -> Result<(), GenerationError> {
        for weight in weights {
            let listing = self
                .get_json(
                    endpoint,
                    &format!("/models/{}", weight.role.folder()),
                    cancel,
                    4 * 1024 * 1024,
                )
                .await?;
            let listed = listing
                .as_array()
                .ok_or(GenerationError::Response("model listing is not an array"))?
                .iter()
                .filter_map(Value::as_str)
                .find(|entry| entry.replace('\\', "/") == weight.name)
                .ok_or_else(|| GenerationError::NotVisible(weight.name.clone()))?;
            weight.name = listed.to_owned();
        }
        Ok(())
    }

    /// Weight files of the most recent ComfyUI prompt, best effort.
    async fn previous_files(
        &self,
        endpoint: &str,
        cancel: &CancellationToken,
    ) -> Option<std::collections::BTreeSet<String>> {
        let request = Request::new(endpoint, "/history", None, None)
            .ok()?
            .with_query(&[("max_items", "1")])
            .ok()?;
        let history = read_json(self.http, request, cancel, 16 * 1024 * 1024)
            .await
            .ok()?;
        let graph = history
            .as_object()?
            .values()
            .next_back()?
            .get("prompt")?
            .get(2)?;
        Some(model_files(graph)).filter(|files| !files.is_empty())
    }

    /// Unloads another workflow's weights before switching models when memory is shared (Apple
    /// MPS) or free device memory is below this model's weights. Live testing on a 36 GB M4 Max:
    /// Wan swapped and sampled ~4x slower beside a resident FLUX even with more free memory than
    /// Wan's weights, because activations and upcast encoders need far more than the files.
    async fn make_room(
        &self,
        endpoint: &str,
        status: &ComfyStatus,
        needed: u64,
        graph: &Value,
        cancel: &CancellationToken,
    ) -> Result<(), GenerationError> {
        let shared = apple_mps(&status.devices);
        let short = status.free_bytes.is_some_and(|free| free < needed);
        if !shared && !short {
            return Ok(());
        }
        let Some(previous) = self.previous_files(endpoint, cancel).await else {
            return Ok(());
        };
        if previous == model_files(graph) {
            return Ok(());
        }
        let free = status.free_bytes.map_or_else(
            || "unknown".to_owned(),
            |free| format!("{:.1} GiB", free as f64 / 1073741824.0),
        );
        self.emit(format!(
            "Freeing ComfyUI memory before switching models ({free} free, {:.1} GiB of weights needed{})",
            needed as f64 / 1073741824.0,
            if shared { ", shared Apple memory" } else { "" }
        ));
        let (code, _) = self
            .post_json(
                endpoint,
                "/free",
                json!({"unload_models": true, "free_memory": true}),
                cancel,
            )
            .await?;
        if !(200..300).contains(&code) {
            self.emit(format!(
                "ComfyUI declined to free memory (HTTP {code}); continuing"
            ));
        }
        Ok(())
    }

    async fn cancel_remote(&self, endpoint: &str, id: &str) {
        let fresh = CancellationToken::new();
        for (path, body) in [
            ("/queue", json!({"delete": [id]})),
            ("/interrupt", json!({"prompt_id": id})),
        ] {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                self.post_json(endpoint, path, body, &fresh),
            )
            .await;
        }
    }

    async fn wait(
        &self,
        endpoint: &str,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<Value, GenerationError> {
        let started = tokio::time::Instant::now();
        loop {
            if started.elapsed() > self.deadline {
                return Err(GenerationError::Timeout);
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(GenerationError::Cancelled),
                _ = tokio::time::sleep(self.poll_interval) => {}
            }
            let history = self
                .get_json(
                    endpoint,
                    &format!("/history/{id}"),
                    cancel,
                    16 * 1024 * 1024,
                )
                .await?;
            let Some(entry) = history.get(id) else {
                continue;
            };
            let status = entry.get("status");
            if status
                .and_then(|status| status.get("status_str"))
                .and_then(Value::as_str)
                == Some("error")
            {
                let detail = status
                    .and_then(|status| status.get("messages"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .find(|message| {
                        message.get(0).and_then(Value::as_str) == Some("execution_error")
                    })
                    .and_then(|message| message.get(1))
                    .map(summary)
                    .unwrap_or_else(|| "execution error".into());
                return Err(GenerationError::Failed(detail));
            }
            if status
                .and_then(|status| status.get("completed"))
                .and_then(Value::as_bool)
                == Some(true)
            {
                return entry
                    .get("outputs")
                    .cloned()
                    .ok_or(GenerationError::Response("completed prompt has no outputs"));
            }
        }
    }

    async fn fetch(
        &self,
        endpoint: &str,
        kind: GenerationKind,
        descriptor: &Value,
        target: &Path,
        cancel: &CancellationToken,
    ) -> Result<u64, GenerationError> {
        let extension = output_extension(kind);
        let field = |key| descriptor.get(key).and_then(Value::as_str);
        let filename = field("filename")
            .filter(|name| safe_output_name(name, extension))
            .ok_or(GenerationError::Response("unsafe output filename"))?;
        let subfolder = field("subfolder")
            .filter(|folder| safe_subfolder(folder))
            .ok_or(GenerationError::Response("unsafe output subfolder"))?;
        if field("type") != Some("output") {
            return Err(GenerationError::Response("output is not a saved file"));
        }
        let request = Request::new(endpoint, "/view", None, None)?.with_query(&[
            ("filename", filename),
            ("subfolder", subfolder),
            ("type", "output"),
        ])?;
        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let operation = async {
            let response = self.http.send(request).await?;
            if !(200..300).contains(&response.status) {
                return Err(GenerationError::Http(HttpError::Status(response.status)));
            }
            let temporary = tempfile::Builder::new()
                .prefix(".rigspark-output.")
                .suffix(".part")
                .tempfile_in(parent)
                .map_err(|error| GenerationError::Output(error.to_string()))?;
            let mut output = tokio::fs::File::from_std(
                temporary
                    .reopen()
                    .map_err(|error| GenerationError::Output(error.to_string()))?,
            );
            let mut body = response.body.take(MAX_OUTPUT_BYTES + 1);
            let mut buffer = vec![0u8; 65536];
            let mut head = Vec::with_capacity(16);
            let mut total = 0u64;
            loop {
                let count = body
                    .read(&mut buffer)
                    .await
                    .map_err(|_| HttpError::Transport)?;
                if count == 0 {
                    break;
                }
                total += count as u64;
                if total > MAX_OUTPUT_BYTES {
                    return Err(GenerationError::Response("output exceeds 1 GiB"));
                }
                if head.len() < 16 {
                    let needed = (16 - head.len()).min(count);
                    head.extend_from_slice(&buffer[..needed]);
                }
                output
                    .write_all(&buffer[..count])
                    .await
                    .map_err(|error| GenerationError::Output(error.to_string()))?;
            }
            if !has_magic(kind, &head) {
                return Err(GenerationError::Response(
                    "output is not the expected media type",
                ));
            }
            output
                .sync_all()
                .await
                .map_err(|error| GenerationError::Output(error.to_string()))?;
            drop(output);
            temporary.persist_noclobber(target).map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    GenerationError::Exists(target.to_path_buf())
                } else {
                    GenerationError::Output(error.error.to_string())
                }
            })?;
            Ok(total)
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(GenerationError::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(600), operation) =>
                result.map_err(|_| GenerationError::Timeout)?,
        }
    }

    pub async fn generate(
        &self,
        request: &GenerationRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<GenerationOutcome, GenerationError> {
        let model = request.model;
        validate_prompt(request.prompt)?;
        if request.seed > MAX_SEED {
            return Err(GenerationError::Invalid(
                "seed must be in 0..=9007199254740991",
            ));
        }
        crate::state::loopback(request.endpoint)
            .map_err(|_| GenerationError::Invalid("ComfyUI endpoint must be loopback HTTP"))?;
        let extension = output_extension(model.kind);
        if let Some(output) = request.output {
            if !output
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
            {
                return Err(GenerationError::Invalid(match model.kind {
                    GenerationKind::Image => "image output must use the .png extension",
                    GenerationKind::Video => "video output must use the .webp extension",
                }));
            }
            if std::fs::symlink_metadata(output).is_ok() {
                return Err(GenerationError::Exists(output.to_path_buf()));
            }
        }
        let status = self.ready(request.endpoint, cancel).await?;
        let devices = &status.devices;
        let built_in = model
            .runnable()
            .map_err(|error| GenerationError::Catalog(error.to_string()))?;
        if matches!(built_in, Workflow::WanT2v | Workflow::Wan22Ti2v) && apple_mps(devices) {
            self.emit(
                "ComfyUI reports Apple MPS: using the euler sampler (uni_pc diverges on MPS)"
                    .into(),
            );
        }
        self.emit(format!("Verifying weights for {}", model.id));
        let mut weights = self.install(model, request.comfyui_dir, cancel).await?;
        self.resolve_names(request.endpoint, &mut weights, cancel)
            .await?;
        let names = weights
            .iter()
            .map(|weight| (weight.role, weight.name.clone()))
            .collect();
        let graph = workflow(model, &names, request.prompt, request.seed, devices)?;
        ensure_local_only(&graph)?;
        self.make_room(
            request.endpoint,
            &status,
            model.total_bytes(),
            &graph,
            cancel,
        )
        .await?;
        let (status, response) = self
            .post_json(
                request.endpoint,
                "/prompt",
                json!({"prompt": graph}),
                cancel,
            )
            .await?;
        if !(200..300).contains(&status) {
            return Err(GenerationError::Rejected(summary(&response)));
        }
        if response
            .get("node_errors")
            .and_then(Value::as_object)
            .is_some_and(|errors| !errors.is_empty())
        {
            return Err(GenerationError::Rejected(summary(&response)));
        }
        let id = prompt_id(&response)?;
        self.emit(format!(
            "Queued ComfyUI prompt {id}; generating {}",
            model.kind.name()
        ));
        let outputs = match self.wait(request.endpoint, &id, cancel).await {
            Ok(outputs) => outputs,
            Err(error) => {
                if matches!(error, GenerationError::Cancelled | GenerationError::Timeout) {
                    self.cancel_remote(request.endpoint, &id).await;
                }
                return Err(error);
            }
        };
        let descriptor = outputs
            .get(save_node(built_in))
            .and_then(|node| node.get("images"))
            .and_then(Value::as_array)
            .and_then(|images| images.first())
            .ok_or(GenerationError::Response("save node produced no output"))?;
        let target = match request.output {
            Some(path) => path.to_path_buf(),
            None => request.output_dir.join(format!(
                "rigspark-{}-{}.{extension}",
                model.kind.name(),
                &id[..id.len().min(8)]
            )),
        };
        let bytes = self
            .fetch(request.endpoint, model.kind, descriptor, &target, cancel)
            .await?;
        Ok(GenerationOutcome {
            model: model.id.clone(),
            kind: model.kind,
            path: target,
            bytes,
            prompt_id: id,
            seed: request.seed,
            weights,
        })
    }
}

/// Inputs shared by the CLI, terminal UI, and browser GUI.
#[derive(Debug, Clone)]
pub struct NativeOptions {
    pub model: String,
    pub prompt: String,
    pub seed: Option<u64>,
    pub comfyui_dir: PathBuf,
    pub port: u16,
    pub bypass: bool,
    pub output: Option<PathBuf>,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeOutcome {
    pub result: GenerationOutcome,
    pub fit: GenerationFit,
}

/// `RIGSPARK_COMFYUI_DIR`, else `~/ComfyUI` when it looks like a ComfyUI install.
pub fn default_comfyui_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RIGSPARK_COMFYUI_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())?;
    let candidate = PathBuf::from(home).join("ComfyUI");
    candidate.join("models").is_dir().then_some(candidate)
}

/// Validates everything that needs no hardware, network, or ComfyUI access.
pub fn prepare(
    options: &NativeOptions,
) -> Result<(GenerationModel, u64, PathBuf), GenerationError> {
    validate_prompt(&options.prompt)?;
    let seed = match options.seed {
        Some(seed) if seed <= MAX_SEED => seed,
        Some(_) => {
            return Err(GenerationError::Invalid(
                "seed must be in 0..=9007199254740991",
            ));
        }
        None => random_seed(),
    };
    if options.port == 0 {
        return Err(GenerationError::Invalid(
            "ComfyUI port must be in 1..=65535",
        ));
    }
    let catalog = GenerationCatalog::bundled()
        .map_err(|error| GenerationError::Catalog(error.to_string()))?;
    let model = catalog
        .resolve(&options.model)
        .map_err(|error| GenerationError::Catalog(error.to_string()))?
        .clone();
    if options.comfyui_dir.as_os_str().is_empty() {
        return Err(GenerationError::Invalid(
            "a ComfyUI directory is required (--comfyui-dir or RIGSPARK_COMFYUI_DIR)",
        ));
    }
    let dir = std::path::absolute(&options.comfyui_dir)
        .map_err(|_| GenerationError::Invalid("invalid ComfyUI directory"))?;
    if !dir.join("models").is_dir() {
        return Err(GenerationError::Invalid(
            "ComfyUI directory must contain a models/ folder",
        ));
    }
    Ok((model, seed, dir))
}

/// Full local run: validate, check memory fit, then drive the loopback ComfyUI server.
pub async fn run_native(
    options: &NativeOptions,
    events: Option<Events>,
    cancel: &CancellationToken,
) -> Result<NativeOutcome, GenerationError> {
    let (model, seed, comfyui_dir) = prepare(options)?;
    let emit = |message: String| {
        if let Some(events) = &events {
            events(message);
        }
    };
    let (hardware, warnings) = crate::hardware::detect()
        .await
        .map_err(|error| GenerationError::Hardware(error.to_string()))?;
    for warning in warnings {
        emit(warning);
    }
    let memory = fit(&model, &hardware);
    if memory.verdict == FitVerdict::No && !options.bypass {
        return Err(GenerationError::NoFit(format!(
            "{} will not fit this machine ({}); bypass to try anyway (integrity checks still apply)",
            model.id, memory.reason
        )));
    }
    emit(format!(
        "Model {} ({}): fit {} - {}; speed unknown",
        model.id,
        model.kind.name(),
        memory.verdict.name(),
        memory.reason
    ));
    let http = crate::http::NativeTransport::new()?;
    let download = crate::acquire::HfTransport::new().map_err(GenerationError::Install)?;
    let client = ComfyUi {
        http: &http,
        download: &download,
        poll_interval: Duration::from_secs(1),
        deadline: Duration::from_secs(2 * 60 * 60),
        events: events.clone(),
    };
    let endpoint = format!("http://127.0.0.1:{}", options.port);
    let result = client
        .generate(
            &GenerationRequest {
                model: &model,
                prompt: &options.prompt,
                seed,
                endpoint: &endpoint,
                comfyui_dir: &comfyui_dir,
                output: options.output.as_deref(),
                output_dir: &options.output_dir,
            },
            cancel,
        )
        .await?;
    Ok(NativeOutcome {
        result,
        fit: memory,
    })
}

pub type GenerationFuture<'call> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<NativeOutcome, GenerationError>> + Send + 'call>,
>;

/// Injectable generation boundary so the TUI and GUI can be tested without ComfyUI.
pub trait Generator: Send + Sync {
    fn generate(
        &self,
        options: NativeOptions,
        events: Events,
        cancel: CancellationToken,
    ) -> GenerationFuture<'_>;
}

pub struct NativeGenerator;
impl Generator for NativeGenerator {
    fn generate(
        &self,
        options: NativeOptions,
        events: Events,
        cancel: CancellationToken,
    ) -> GenerationFuture<'_> {
        Box::pin(async move { run_native(&options, Some(events), &cancel).await })
    }
}
