//! Local voice: text-to-speech ("speak") and speech-to-text ("listen").
//!
//! RigSpark starts whisper.cpp's `whisper-server` with a pinned, SHA-256 verified ggml
//! file, or `mlx_audio.server` on Apple Silicon, always bound to 127.0.0.1. Any other
//! loopback server that speaks OpenAI's `/v1/audio/speech` and `/v1/audio/transcriptions`
//! can be attached instead. Requests and replies are bounded; audio is WAV only.
use crate::acquire::{Acquisition, Artifact, HfTransport, SizePolicy};
use rigspark_core::{
    sizing::Hardware,
    voice::{VoiceCatalog, VoiceEngine, VoiceKind, VoiceModel},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub const WHISPER_SERVER_ENV: &str = "RIGSPARK_WHISPER_SERVER";
pub const MLX_AUDIO_SERVER_ENV: &str = "RIGSPARK_MLX_AUDIO_SERVER";
/// Longest text one speak request accepts.
pub const MAX_SPEAK_CHARS: usize = 8000;
/// Largest WAV a speak reply or a transcription upload may be.
pub const MAX_AUDIO_BYTES: usize = 48 * 1024 * 1024;
/// Longest transcript accepted back from an engine.
pub const MAX_TRANSCRIPT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceError(pub String);
impl std::fmt::Display for VoiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for VoiceError {}
fn fail<T>(message: impl Into<String>) -> Result<T, VoiceError> {
    Err(VoiceError(message.into()))
}

/// Where an engine's server binary was found, if anywhere.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineProbe {
    pub engine: &'static str,
    pub label: &'static str,
    pub installed: bool,
    pub supported: bool,
    #[serde(skip)]
    pub path: Option<PathBuf>,
    pub install: &'static str,
}

fn binary_from(variable: Option<std::ffi::OsString>, name: &str) -> Option<PathBuf> {
    match variable {
        Some(value) => {
            let path = PathBuf::from(value);
            (path.is_absolute() && path.is_file())
                .then(|| path.canonicalize().ok())
                .flatten()
        }
        None => crate::process_control::resolve_binary(name).ok(),
    }
}

pub fn probe_engine(engine: VoiceEngine, hardware: &Hardware) -> EngineProbe {
    let (label, variable, binary, install) = match engine {
        VoiceEngine::WhisperCpp => (
            "whisper.cpp",
            WHISPER_SERVER_ENV,
            "whisper-server",
            "brew install whisper-cpp (or build whisper.cpp and set RIGSPARK_WHISPER_SERVER)",
        ),
        VoiceEngine::MlxAudio => (
            "mlx-audio",
            MLX_AUDIO_SERVER_ENV,
            "mlx_audio.server",
            "pip install -U mlx-audio (Apple Silicon only)",
        ),
    };
    let path = binary_from(std::env::var_os(variable), binary);
    EngineProbe {
        engine: engine.name(),
        label,
        installed: path.is_some(),
        supported: engine.supports(hardware),
        path,
        install,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotState {
    Starting,
    Running,
    Failed,
}

/// How requests reach a running voice server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    /// whisper.cpp's `/inference` multipart endpoint.
    WhisperCpp,
    /// OpenAI's `/v1/audio/speech` and `/v1/audio/transcriptions`.
    OpenAi,
}

struct Slot {
    model: String,
    engine: &'static str,
    state: SlotState,
    detail: String,
    progress: Option<(u64, u64)>,
    endpoint: Option<String>,
    /// The `model` value sent to an OpenAI-style server.
    request_model: String,
    protocol: Protocol,
    owned: bool,
    child: Option<tokio::process::Child>,
    cancel: CancellationToken,
    generation: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotView {
    pub kind: &'static str,
    pub model: String,
    pub engine: &'static str,
    pub state: SlotState,
    pub detail: String,
    pub progress: Option<(u64, u64)>,
    pub endpoint: Option<String>,
    pub owned: bool,
}

/// One speak and one listen server at a time, each owned or attached.
pub struct VoiceService {
    cache: PathBuf,
    slots: Mutex<BTreeMap<&'static str, Slot>>,
    client: reqwest::Client,
    generation: std::sync::atomic::AtomicU64,
}

#[derive(Debug, Clone)]
pub struct SpeakRequest {
    pub text: String,
    pub voice: Option<String>,
    pub speed: Option<f64>,
}

fn kind_key(kind: VoiceKind) -> &'static str {
    kind.name()
}

fn token(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// A RIFF/WAVE header; every audio body in and out must start with one.
pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

fn free_port() -> Result<u16, VoiceError> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| VoiceError("no free local port".into()))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|_| VoiceError("no free local port".into()))
}

fn multipart(boundary: &str, fields: &[(&str, &str)], audio: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(audio.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn bounded(response: reqwest::Response, maximum: usize) -> Result<Vec<u8>, VoiceError> {
    if !response.status().is_success() {
        return fail(format!(
            "voice server answered {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return fail("voice server reply exceeds the size limit");
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| VoiceError("voice server reply was interrupted".into()))?
    {
        if chunk.len() > maximum - bytes.len() {
            return fail("voice server reply exceeds the size limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

impl VoiceService {
    pub fn new(home: &Path) -> Result<Self, VoiceError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| VoiceError("voice client setup failed".into()))?;
        Ok(Self {
            cache: home.join("cache"),
            slots: Mutex::new(BTreeMap::new()),
            client,
            generation: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<&'static str, Slot>>, VoiceError> {
        self.slots
            .lock()
            .map_err(|_| VoiceError("voice state unavailable".into()))
    }

    pub fn status(&self) -> Vec<SlotView> {
        let Ok(slots) = self.lock() else {
            return Vec::new();
        };
        slots
            .iter()
            .map(|(kind, slot)| SlotView {
                kind,
                model: slot.model.clone(),
                engine: slot.engine,
                state: slot.state,
                detail: slot.detail.clone(),
                progress: slot.progress,
                endpoint: slot.endpoint.clone(),
                owned: slot.owned,
            })
            .collect()
    }

    fn update(&self, kind: &'static str, generation: u64, change: impl FnOnce(&mut Slot)) {
        if let Ok(mut slots) = self.lock()
            && let Some(slot) = slots
                .get_mut(kind)
                .filter(|slot| slot.generation == generation)
        {
            change(slot);
        }
    }

    /// Replaces the slot for `kind`, stopping whatever ran there before.
    fn claim(&self, kind: VoiceKind, slot: Slot) -> Result<(), VoiceError> {
        let previous = self.lock()?.insert(kind_key(kind), slot);
        if let Some(mut previous) = previous {
            previous.cancel.cancel();
            if let Some(child) = previous.child.as_mut() {
                let _ = child.start_kill();
            }
        }
        Ok(())
    }

    pub fn stop(&self, kind: VoiceKind) -> bool {
        let Ok(mut slots) = self.lock() else {
            return false;
        };
        match slots.remove(kind_key(kind)) {
            Some(mut slot) => {
                slot.cancel.cancel();
                if let Some(child) = slot.child.as_mut() {
                    let _ = child.start_kill();
                }
                true
            }
            None => false,
        }
    }

    pub fn shutdown(&self) {
        for kind in [VoiceKind::Speak, VoiceKind::Listen] {
            self.stop(kind);
        }
    }

    /// Uses a voice server someone else runs on loopback, speaking OpenAI's audio API.
    pub async fn attach(
        &self,
        kind: VoiceKind,
        endpoint: &str,
        model: &str,
    ) -> Result<(), VoiceError> {
        let url = crate::state::loopback(endpoint).map_err(|_| {
            VoiceError("endpoint must be http://127.0.0.1:<port> or localhost".into())
        })?;
        if url.query().is_some() || url.fragment().is_some() || endpoint.len() > 256 {
            return fail("endpoint must be a plain loopback base URL");
        }
        if model.len() > 200
            || !model.split('/').all(|part| token(part, 100))
            || model.split('/').count() > 2
        {
            return fail("model must be a repository id or a short name");
        }
        let base = endpoint.trim_end_matches('/').to_owned();
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.client.get(format!("{base}/v1/models")).send(),
        )
        .await
        .map_err(|_| VoiceError("voice server did not answer".into()))?
        .map_err(|_| VoiceError("voice server is not reachable".into()))?;
        if !response.status().is_success() {
            return fail("voice server does not offer /v1/models");
        }
        let generation = self.next_generation();
        self.claim(
            kind,
            Slot {
                model: model.into(),
                engine: "attached",
                state: SlotState::Running,
                detail: "Attached server".into(),
                progress: None,
                endpoint: Some(base),
                request_model: model.into(),
                protocol: Protocol::OpenAi,
                owned: false,
                child: None,
                cancel: CancellationToken::new(),
                generation,
            },
        )
    }

    fn next_generation(&self) -> u64 {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1
    }

    /// Starts `model`'s engine in the background; `status` reports its progress.
    pub fn start(
        self: &Arc<Self>,
        model: &VoiceModel,
        hardware: &Hardware,
        parent: &CancellationToken,
    ) -> Result<(), VoiceError> {
        let probe = probe_engine(model.engine, hardware);
        if !probe.supported {
            return fail(format!("{} does not run on this machine", probe.label));
        }
        let Some(binary) = probe.path.clone() else {
            return fail(format!(
                "{} is not installed: {}",
                probe.label, probe.install
            ));
        };
        self.begin(model, binary, parent)
    }

    /// Claims `model`'s slot and launches `binary` for it in the background.
    fn begin(
        self: &Arc<Self>,
        model: &VoiceModel,
        binary: PathBuf,
        parent: &CancellationToken,
    ) -> Result<(), VoiceError> {
        let kind = kind_key(model.kind);
        let generation = self.next_generation();
        let cancel = parent.child_token();
        self.claim(
            model.kind,
            Slot {
                model: model.id.clone(),
                engine: model.engine.name(),
                state: SlotState::Starting,
                detail: "Starting".into(),
                progress: None,
                endpoint: None,
                request_model: model
                    .engine_model
                    .clone()
                    .unwrap_or_else(|| model.id.clone()),
                protocol: match model.engine {
                    VoiceEngine::WhisperCpp => Protocol::WhisperCpp,
                    VoiceEngine::MlxAudio => Protocol::OpenAi,
                },
                owned: true,
                child: None,
                cancel: cancel.clone(),
                generation,
            },
        )?;
        let service = self.clone();
        let model = model.clone();
        tokio::spawn(async move {
            let outcome = service
                .launch(&model, &binary, kind, generation, &cancel)
                .await;
            service.update(kind, generation, |slot| match outcome {
                Ok(()) => {}
                Err(error) => {
                    slot.state = SlotState::Failed;
                    slot.detail = error.0;
                    if let Some(child) = slot.child.as_mut() {
                        let _ = child.start_kill();
                    }
                    slot.child = None;
                }
            });
        });
        Ok(())
    }

    async fn launch(
        self: &Arc<Self>,
        model: &VoiceModel,
        binary: &Path,
        kind: &'static str,
        generation: u64,
        cancel: &CancellationToken,
    ) -> Result<(), VoiceError> {
        let port = free_port()?;
        let base = format!("http://127.0.0.1:{port}");
        let mut command = tokio::process::Command::new(binary);
        match model.engine {
            VoiceEngine::WhisperCpp => {
                let file = model
                    .file
                    .as_ref()
                    .ok_or_else(|| VoiceError("missing pinned file".into()))?;
                self.update(kind, generation, |slot| {
                    slot.detail = format!("Downloading {}", file.file);
                });
                let artifact = Artifact {
                    backend: "whispercpp".into(),
                    repo: file.repo.clone(),
                    revision: file.revision.clone(),
                    file: file.file.clone(),
                    sha256: file.sha256.clone(),
                    bytes: file.bytes,
                };
                let progress = {
                    let service = self.clone();
                    move |done: u64, total: u64, _: &str| {
                        service
                            .update(kind, generation, |slot| slot.progress = Some((done, total)));
                    }
                };
                let acquisition = Acquisition::new(&self.cache)
                    .map_err(VoiceError)?
                    .with_progress(progress);
                let transport = HfTransport::new().map_err(VoiceError)?;
                let size = if file.exact_size {
                    SizePolicy::Exact
                } else {
                    SizePolicy::Ceiling
                };
                let acquired = acquisition
                    .acquire_sized(&artifact, size, &transport, cancel)
                    .await
                    .map_err(|error| VoiceError(format!("download failed: {error}")))?;
                command.arg("-m").arg(&acquired.path).args([
                    "--host",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                ]);
            }
            VoiceEngine::MlxAudio => {
                command.args(["--host", "127.0.0.1", "--port", &port.to_string()]);
            }
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .map_err(|_| VoiceError("voice server failed to start".into()))?;
        self.update(kind, generation, |slot| {
            slot.child = Some(child);
            slot.progress = None;
            slot.detail = "Loading the model".into();
        });
        let probe = match model.engine {
            VoiceEngine::WhisperCpp => format!("{base}/"),
            VoiceEngine::MlxAudio => format!("{base}/v1/models"),
        };
        self.wait_ready(&probe, kind, generation, cancel).await?;
        if model.engine == VoiceEngine::MlxAudio {
            self.update(kind, generation, |slot| {
                slot.detail = "Fetching and loading weights (mlx-audio)".into();
            });
            let repository = model.engine_model.clone().unwrap_or_default();
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => return fail("cancelled"),
                response = self
                    .client
                    .post(format!("{base}/v1/models"))
                    .query(&[("model_name", repository.as_str())])
                    .timeout(Duration::from_secs(1800))
                    .send() => response,
            };
            match response {
                Ok(response) if response.status().is_success() => {}
                _ => return fail("mlx-audio could not load the model"),
            }
        }
        self.update(kind, generation, |slot| {
            slot.state = SlotState::Running;
            slot.detail = "Ready".into();
            slot.endpoint = Some(base.clone());
        });
        Ok(())
    }

    async fn wait_ready(
        &self,
        url: &str,
        kind: &'static str,
        generation: u64,
        cancel: &CancellationToken,
    ) -> Result<(), VoiceError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            if cancel.is_cancelled() {
                return fail("cancelled");
            }
            let exited = {
                let mut slots = self.lock()?;
                match slots
                    .get_mut(kind)
                    .filter(|slot| slot.generation == generation)
                {
                    Some(slot) => slot
                        .child
                        .as_mut()
                        .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_)))),
                    None => return fail("replaced"),
                }
            };
            if exited {
                return fail("voice server exited while starting");
            }
            if let Ok(Ok(response)) =
                tokio::time::timeout(Duration::from_secs(2), self.client.get(url).send()).await
                && response.status().is_success()
            {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                return fail("voice server did not become ready");
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    fn running(
        &self,
        kind: VoiceKind,
    ) -> Result<(String, String, Protocol, &'static str), VoiceError> {
        let slots = self.lock()?;
        let slot = slots
            .get(kind_key(kind))
            .filter(|slot| slot.state == SlotState::Running)
            .ok_or_else(|| {
                VoiceError(format!(
                    "no {} model is running; start one from Models › Voice",
                    kind.name()
                ))
            })?;
        Ok((
            slot.endpoint.clone().unwrap_or_default(),
            slot.request_model.clone(),
            slot.protocol,
            slot.engine,
        ))
    }

    /// Text to WAV with the running speak model.
    pub async fn speak(
        &self,
        request: &SpeakRequest,
        catalog: &VoiceCatalog,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, VoiceError> {
        let text = request.text.trim();
        if text.is_empty() || text.chars().count() > MAX_SPEAK_CHARS {
            return fail(format!("text must be 1 to {MAX_SPEAK_CHARS} characters"));
        }
        if request
            .speed
            .is_some_and(|speed| !(0.5..=2.0).contains(&speed) || !speed.is_finite())
        {
            return fail("speed must be between 0.5 and 2.0");
        }
        if request
            .voice
            .as_deref()
            .is_some_and(|voice| !token(voice, 64))
        {
            return fail("invalid voice");
        }
        let (endpoint, model, _, engine) = self.running(VoiceKind::Speak)?;
        let mut body = json!({
            "model": model,
            "input": text,
            "response_format": "wav",
            "speed": request.speed.unwrap_or(1.0),
        });
        if let Some(voice) = &request.voice {
            body["voice"] = json!(voice);
            // mlx-audio picks Kokoro's language from lang_code, which its voice ids encode.
            if engine == "mlx-audio"
                && let Some(entry) = catalog
                    .models
                    .iter()
                    .find(|entry| entry.engine_model.as_deref() == Some(model.as_str()))
                    .and_then(|entry| entry.voice(voice))
            {
                body["lang_code"] = json!(entry.lang);
            }
        }
        let send = self
            .client
            .post(format!("{endpoint}/v1/audio/speech"))
            .json(&body)
            .timeout(Duration::from_secs(600))
            .send();
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return fail("cancelled"),
            response = send => response.map_err(|_| VoiceError("speech request failed".into()))?,
        };
        let audio = bounded(response, MAX_AUDIO_BYTES).await?;
        if !is_wav(&audio) {
            return fail("voice server did not return WAV audio");
        }
        Ok(audio)
    }

    /// WAV to text with the running listen model.
    pub async fn transcribe(
        &self,
        audio: &[u8],
        language: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<String, VoiceError> {
        if !is_wav(audio) || audio.len() > MAX_AUDIO_BYTES {
            return fail("audio must be a WAV file under 48 MiB");
        }
        if language.is_some_and(|language| !token(language, 16)) {
            return fail("invalid language");
        }
        let (endpoint, model, protocol, _) = self.running(VoiceKind::Listen)?;
        let boundary = format!("rigspark{}", uuid::Uuid::new_v4().simple());
        let mut fields: Vec<(&str, &str)> = Vec::new();
        let url = match protocol {
            Protocol::WhisperCpp => {
                fields.extend([("response_format", "json"), ("temperature", "0.0")]);
                format!("{endpoint}/inference")
            }
            Protocol::OpenAi => {
                fields.extend([("model", model.as_str()), ("response_format", "json")]);
                format!("{endpoint}/v1/audio/transcriptions")
            }
        };
        if let Some(language) = language {
            fields.push(("language", language));
        }
        let body = multipart(&boundary, &fields, audio);
        let send = self
            .client
            .post(url)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .timeout(Duration::from_secs(600))
            .send();
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return fail("cancelled"),
            response = send => response.map_err(|_| VoiceError("transcription request failed".into()))?,
        };
        let raw = bounded(response, MAX_TRANSCRIPT_BYTES).await?;
        let value: Value = serde_json::from_slice(&raw)
            .map_err(|_| VoiceError("voice server returned an invalid transcript".into()))?;
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| VoiceError("voice server returned no text".into()))?;
        Ok(rigspark_core::reports::strip_control(text.trim()))
    }
}

impl Drop for VoiceService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn wav(samples: usize) -> Vec<u8> {
        let data = samples * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&32000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data as u32).to_le_bytes());
        bytes.resize(44 + data, 0);
        bytes
    }

    /// A loopback server that answers every request with `reply` and records requests.
    async fn server(
        reply: Vec<u8>,
        content_type: &'static str,
    ) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let reply = reply.clone();
                let record = record.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = vec![0u8; 65536];
                    loop {
                        let read = socket.read(&mut buffer).await.unwrap_or(0);
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        let text = String::from_utf8_lossy(&request);
                        if let Some(end) = text.find("\r\n\r\n") {
                            let length = text[..end]
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if request.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    record.lock().unwrap().push(request);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        reply.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&reply).await;
                });
            }
        });
        (format!("http://{address}"), seen)
    }

    fn catalog() -> VoiceCatalog {
        VoiceCatalog::bundled().unwrap()
    }
    fn request(text: &str) -> SpeakRequest {
        SpeakRequest {
            text: text.into(),
            voice: Some("af_heart".into()),
            speed: Some(1.0),
        }
    }

    #[test]
    fn wav_headers_and_multipart_bodies_are_exact() {
        assert!(is_wav(&wav(4)));
        assert!(!is_wav(b"RIFF\0\0\0\0WAVX"));
        assert!(!is_wav(b"RIFF"));
        let body = multipart("b", &[("model", "m")], b"AUDIO");
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "--b\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nm\r\n--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\nAUDIO\r\n--b--\r\n"
        );
    }

    #[tokio::test]
    async fn nothing_runs_until_a_model_is_started_or_attached() {
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        assert!(service.status().is_empty());
        let error = service
            .speak(&request("hi"), &catalog(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.0.contains("no speak model is running"), "{error}");
        assert!(
            service
                .transcribe(&wav(4), None, &CancellationToken::new())
                .await
                .unwrap_err()
                .0
                .contains("no listen model")
        );
        assert!(!service.stop(VoiceKind::Speak));
    }

    #[tokio::test]
    async fn attached_servers_must_be_plain_loopback_openai_audio() {
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        for endpoint in [
            "https://127.0.0.1:1",
            "http://example.com:8000",
            "http://user@127.0.0.1:1",
            "http://127.0.0.1:1/?x=1",
        ] {
            assert!(
                service
                    .attach(VoiceKind::Speak, endpoint, "kokoro")
                    .await
                    .is_err(),
                "{endpoint}"
            );
        }
        let (base, _) = server(b"{\"data\":[]}".to_vec(), "application/json").await;
        for model in ["", "a/b/c", "bad model", &"m".repeat(101)] {
            assert!(
                service
                    .attach(VoiceKind::Speak, &base, model)
                    .await
                    .is_err(),
                "{model:?}"
            );
        }
        service
            .attach(
                VoiceKind::Speak,
                &format!("{base}/"),
                "mlx-community/Kokoro-82M-bf16",
            )
            .await
            .unwrap();
        let status = service.status();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].state, SlotState::Running);
        assert_eq!(status[0].endpoint.as_deref(), Some(base.as_str()));
        assert!(!status[0].owned);
        assert!(service.stop(VoiceKind::Speak));
        assert!(service.status().is_empty());
    }

    #[tokio::test]
    async fn speech_requests_are_bounded_and_replies_must_be_wav() {
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        let (base, seen) = server(wav(8), "audio/wav").await;
        service
            .attach(VoiceKind::Speak, &base, "mlx-community/Kokoro-82M-bf16")
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let audio = service
            .speak(&request("Hello there"), &catalog(), &cancel)
            .await
            .unwrap();
        assert_eq!(audio, wav(8));
        let sent =
            String::from_utf8_lossy(&seen.lock().unwrap().last().unwrap().clone()).into_owned();
        assert!(sent.starts_with("POST /v1/audio/speech"), "{sent}");
        assert!(sent.contains("\"input\":\"Hello there\""), "{sent}");
        assert!(sent.contains("\"voice\":\"af_heart\""), "{sent}");
        assert!(sent.contains("\"response_format\":\"wav\""), "{sent}");
        // Attached servers get no mlx-audio language hint.
        assert!(!sent.contains("lang_code"), "{sent}");
        for (text, voice, speed) in [
            ("", None, None),
            ("   ", None, None),
            (&"x".repeat(MAX_SPEAK_CHARS + 1)[..], None, None),
            ("hi", Some("bad voice"), None),
            ("hi", None, Some(0.4)),
            ("hi", None, Some(2.1)),
            ("hi", None, Some(f64::NAN)),
        ] {
            let request = SpeakRequest {
                text: text.into(),
                voice: voice.map(Into::into),
                speed,
            };
            assert!(
                service.speak(&request, &catalog(), &cancel).await.is_err(),
                "{request:?}"
            );
        }
        assert!(
            service
                .speak(&request(&"x".repeat(MAX_SPEAK_CHARS)), &catalog(), &cancel)
                .await
                .is_ok()
        );
        let (base, _) = server(b"not audio".to_vec(), "audio/wav").await;
        service
            .attach(VoiceKind::Speak, &base, "kokoro")
            .await
            .unwrap();
        assert_eq!(
            service
                .speak(&request("hi"), &catalog(), &cancel)
                .await
                .unwrap_err()
                .0,
            "voice server did not return WAV audio"
        );
    }

    #[tokio::test]
    async fn transcription_posts_multipart_wav_and_reads_text() {
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        let (base, seen) = server(
            b"{\"text\":\"  hello \\u001b[31mworld \"}".to_vec(),
            "application/json",
        )
        .await;
        service
            .attach(VoiceKind::Listen, &base, "whisper-large-v3-turbo")
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let text = service
            .transcribe(&wav(16), Some("en"), &cancel)
            .await
            .unwrap();
        assert_eq!(text, "hello world");
        let sent = seen.lock().unwrap().last().unwrap().clone();
        let head = String::from_utf8_lossy(&sent);
        assert!(head.starts_with("POST /v1/audio/transcriptions"), "{head}");
        assert!(
            head.contains("multipart/form-data; boundary=rigspark"),
            "{head}"
        );
        assert!(
            head.contains("name=\"model\"\r\n\r\nwhisper-large-v3-turbo"),
            "{head}"
        );
        assert!(head.contains("name=\"language\"\r\n\r\nen"), "{head}");
        assert!(sent.windows(wav(16).len()).any(|window| window == wav(16)));
        assert!(
            service
                .transcribe(b"not audio", None, &cancel)
                .await
                .is_err()
        );
        assert!(
            service
                .transcribe(&wav(4), Some("e n"), &cancel)
                .await
                .is_err()
        );
        let (base, _) = server(b"{\"segments\":[]}".to_vec(), "application/json").await;
        service
            .attach(VoiceKind::Listen, &base, "whisper")
            .await
            .unwrap();
        assert_eq!(
            service
                .transcribe(&wav(4), None, &cancel)
                .await
                .unwrap_err()
                .0,
            "voice server returned no text"
        );
    }

    #[tokio::test]
    async fn starting_needs_an_installed_supported_engine() {
        let home = tempfile::tempdir().unwrap();
        let service = Arc::new(VoiceService::new(home.path()).unwrap());
        let linux = Hardware {
            arch: rigspark_core::sizing::CpuArch::X64,
            platform: rigspark_core::sizing::Platform::Linux,
            total_ram_bytes: 16e9,
            free_ram_bytes: 16e9,
            free_disk_bytes: 1e12,
            gpu: Vec::new(),
            unified_memory: None,
        };
        let kokoro = catalog().resolve("speak").unwrap().clone();
        let error = service
            .start(&kokoro, &linux, &CancellationToken::new())
            .unwrap_err();
        assert_eq!(error.0, "mlx-audio does not run on this machine");
        assert!(service.status().is_empty());
        let probe = probe_engine(VoiceEngine::MlxAudio, &linux);
        assert!(!probe.supported);
        assert!(probe.install.contains("mlx-audio"));
    }

    #[test]
    fn an_explicit_engine_path_must_be_an_absolute_file() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("whisper-server");
        std::fs::write(&binary, "").unwrap();
        assert_eq!(
            binary_from(Some(binary.clone().into()), "unused"),
            Some(binary.canonicalize().unwrap())
        );
        assert_eq!(binary_from(Some("whisper-server".into()), "unused"), None);
        assert_eq!(binary_from(Some(directory.path().into()), "unused"), None);
    }

    fn slot(kind: VoiceKind, engine: &'static str, endpoint: &str, generation: u64) -> Slot {
        Slot {
            model: kind.name().into(),
            engine,
            state: SlotState::Running,
            detail: "Ready".into(),
            progress: None,
            endpoint: Some(endpoint.into()),
            request_model: "mlx-community/Kokoro-82M-bf16".into(),
            protocol: Protocol::OpenAi,
            owned: false,
            child: None,
            cancel: CancellationToken::new(),
            generation,
        }
    }

    #[test]
    fn limits_errors_ports_and_generations_are_exact() {
        assert_eq!(MAX_AUDIO_BYTES, 50_331_648);
        assert_eq!(MAX_TRANSCRIPT_BYTES, 262_144);
        assert_eq!(VoiceError("boom".into()).to_string(), "boom");
        let port = free_port().unwrap();
        assert!(port > 1);
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        assert_eq!(service.next_generation(), 1);
        assert_eq!(service.next_generation(), 2);
        assert_eq!(service.next_generation(), 3);
    }

    #[test]
    fn updates_reach_only_the_current_generation() {
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        service
            .claim(
                VoiceKind::Listen,
                slot(VoiceKind::Listen, "attached", "http://127.0.0.1:9", 7),
            )
            .unwrap();
        service.update("listen", 6, |slot| slot.detail = "stale".into());
        assert_eq!(service.status()[0].detail, "Ready");
        service.update("listen", 7, |slot| slot.detail = "fresh".into());
        let status = service.status();
        assert_eq!(status[0].detail, "fresh");
        assert_eq!(status[0].kind, "listen");
        service
            .claim(
                VoiceKind::Speak,
                slot(VoiceKind::Speak, "attached", "http://127.0.0.1:9", 8),
            )
            .unwrap();
        assert_eq!(
            service
                .status()
                .iter()
                .map(|view| view.kind)
                .collect::<Vec<_>>(),
            ["listen", "speak"]
        );
        service.shutdown();
        assert!(service.status().is_empty());
    }

    fn response(chunks: Vec<&'static [u8]>, length: bool) -> reqwest::Response {
        let total: usize = chunks.iter().map(|chunk| chunk.len()).sum();
        let body = reqwest::Body::wrap_stream(futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok::<_, std::io::Error>(bytes::Bytes::from_static(chunk))),
        ));
        let mut builder = http::Response::builder().status(200);
        if length {
            builder = builder.header("content-length", total.to_string());
        }
        reqwest::Response::from(builder.body(body).unwrap())
    }

    #[tokio::test]
    async fn replies_are_bounded_by_length_and_by_streamed_bytes() {
        assert_eq!(
            bounded(response(vec![b"abcd"], true), 4).await.unwrap(),
            b"abcd"
        );
        assert!(bounded(response(vec![b"abcde"], true), 4).await.is_err());
        assert_eq!(
            bounded(response(vec![b"ab", b"cd"], false), 4)
                .await
                .unwrap(),
            b"abcd"
        );
        assert!(
            bounded(response(vec![b"ab", b"cde"], false), 4)
                .await
                .is_err()
        );
        assert!(
            bounded(response(vec![b"abc", b"de"], false), 4)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn attach_refuses_fragments_queries_long_endpoints_and_long_models() {
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        let (base, _) = server(b"{\"data\":[]}".to_vec(), "application/json").await;
        for endpoint in [format!("{base}/?x=1"), format!("{base}/#top")] {
            assert_eq!(
                service
                    .attach(VoiceKind::Speak, &endpoint, "kokoro")
                    .await
                    .unwrap_err()
                    .0,
                "endpoint must be a plain loopback base URL",
                "{endpoint}"
            );
        }
        let longest = format!("{base}{}", "/".repeat(256 - base.len()));
        service
            .attach(VoiceKind::Speak, &longest, "kokoro")
            .await
            .unwrap();
        assert!(
            service
                .attach(VoiceKind::Speak, &format!("{longest}/"), "kokoro")
                .await
                .is_err()
        );
        let widest = format!("{}/{}", "a".repeat(100), "b".repeat(99));
        service
            .attach(VoiceKind::Speak, &base, &widest)
            .await
            .unwrap();
        assert!(
            service
                .attach(VoiceKind::Speak, &base, &format!("{widest}b"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn mlx_audio_requests_carry_the_voice_language() {
        let home = tempfile::tempdir().unwrap();
        let service = VoiceService::new(home.path()).unwrap();
        let (base, seen) = server(wav(8), "audio/wav").await;
        service
            .claim(
                VoiceKind::Speak,
                slot(VoiceKind::Speak, "mlx-audio", &base, 1),
            )
            .unwrap();
        service
            .speak(&request("Hello"), &catalog(), &CancellationToken::new())
            .await
            .unwrap();
        let sent =
            String::from_utf8_lossy(&seen.lock().unwrap().last().unwrap().clone()).into_owned();
        assert!(sent.contains("\"lang_code\":\"a\""), "{sent}");
    }

    /// A fake `mlx_audio.server`: waits `delay` seconds, then answers GET with 200
    /// and POST with `post_status`; or exits at once when `delay` is negative.
    #[cfg(unix)]
    fn fake_engine(directory: &Path, delay: i32, post_status: u16) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = directory.join(format!("engine-{delay}-{post_status}"));
        let body = if delay < 0 {
            "#!/bin/sh\nexit 0\n".to_owned()
        } else {
            format!(
                r#"#!/bin/sh
exec python3 -c '
import sys, time, http.server
port = int(sys.argv[sys.argv.index("--port") + 1])
time.sleep({delay})
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, code):
        self.send_response(code); self.send_header("Content-Length", "2"); self.end_headers(); self.wfile.write(b"{{}}")
    def do_GET(self): self.reply(200)
    def do_POST(self): self.reply({post_status})
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
' x "$@"
"#
            )
        };
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    async fn settle(service: &VoiceService) -> SlotView {
        for _ in 0..150 {
            let view = service.status().remove(0);
            if view.state != SlotState::Starting {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the engine never settled");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launched_engines_become_ready_load_the_model_or_fail_clearly() {
        let home = tempfile::tempdir().unwrap();
        let kokoro = catalog().resolve("speak").unwrap().clone();
        let cancel = CancellationToken::new();
        for (delay, post, state, detail) in [
            (1, 200, SlotState::Running, "Ready"),
            (
                0,
                500,
                SlotState::Failed,
                "mlx-audio could not load the model",
            ),
            (
                -1,
                200,
                SlotState::Failed,
                "voice server exited while starting",
            ),
        ] {
            let service = Arc::new(VoiceService::new(home.path()).unwrap());
            let binary = fake_engine(home.path(), delay, post);
            service.begin(&kokoro, binary, &cancel).unwrap();
            let view = settle(&service).await;
            assert_eq!(
                (view.state, view.detail.as_str()),
                (state, detail),
                "{delay}"
            );
            assert!(view.owned);
            assert_eq!(view.engine, "mlx-audio");
            assert_eq!(view.endpoint.is_some(), state == SlotState::Running);
            if state == SlotState::Running {
                let endpoint = view.endpoint.unwrap();
                let port: u16 = endpoint.rsplit(':').next().unwrap().parse().unwrap();
                assert!(port > 1);
            }
            service.shutdown();
        }
    }
}
