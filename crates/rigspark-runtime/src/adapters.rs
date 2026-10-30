use crate::{
    http::{Request, Transport, read_json},
    identity::{ProcessIdentity, ProcessProbe},
    process_control::{
        ProcessControl, SpawnSpec, listener, minimal_env, same_process, stop_owned, wait_owned,
    },
    state::ServerState,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct BackendError(pub String);
impl From<String> for BackendError {
    fn from(value: String) -> Self {
        Self(value)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "llamacpp")]
    LlamaCpp,
    #[serde(rename = "bitnet")]
    BitNet,
}
impl BackendKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::LlamaCpp => "llamacpp",
            Self::BitNet => "bitnet",
        }
    }
    pub fn port(self) -> u16 {
        match self {
            Self::Ollama => 11434,
            Self::LlamaCpp | Self::BitNet => 8080,
        }
    }
}
#[derive(Clone)]
pub struct ServeRequest {
    pub model_id: String,
    pub endpoint: String,
    pub model_path: Option<PathBuf>,
    /// Verified GGUF drafter for llama.cpp speculative decoding.
    pub draft_model_path: Option<PathBuf>,
    pub context: Option<u32>,
    pub cache: Option<crate::cache::CacheProfile>,
}
#[async_trait::async_trait]
pub trait BackendAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn trusts(&self, identity: &ProcessIdentity) -> bool;
    async fn attach_only(
        &self,
        _request: &ServeRequest,
        _cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        Err(BackendError("attach-only operation is unsupported".into()))
    }
    fn can_embed(&self) -> bool {
        matches!(self.name(), "ollama" | "lmstudio")
    }
    fn can_stream(&self) -> bool {
        self.name() == "ollama"
    }
    async fn chat(
        &self,
        _handle: &ServerState,
        _input: &crate::ollama_inference::ChatInput,
        _cancel: &CancellationToken,
    ) -> Result<crate::ollama_inference::ChatResult, BackendError> {
        Err(BackendError(
            "chat is not implemented for this adapter".into(),
        ))
    }
    async fn chat_stream(
        &self,
        handle: &ServerState,
        input: &crate::ollama_inference::ChatInput,
        cancel: &CancellationToken,
        on_delta: &mut (dyn for<'chunk> FnMut(&'chunk str) + Send),
    ) -> Result<crate::ollama_inference::ChatResult, BackendError> {
        let result = self.chat(handle, input, cancel).await?;
        on_delta(&result.content);
        Ok(result)
    }
    async fn embed(
        &self,
        _handle: &ServerState,
        _model: &str,
        _inputs: &[String],
        _cancel: &CancellationToken,
    ) -> Result<crate::ollama_inference::EmbedResult, BackendError> {
        Err(BackendError(
            "embeddings are unsupported by this adapter".into(),
        ))
    }
    async fn serve(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError>;
    async fn ready(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError>;
    async fn stop(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError>;
    async fn ready_handle(
        &self,
        request: &ServeRequest,
        _handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        self.ready(request, cancel).await
    }
}
pub struct RuntimeAdapter<'runtime> {
    pub kind: BackendKind,
    pub binary: PathBuf,
    pub http: &'runtime dyn Transport,
    pub probe: &'runtime dyn ProcessProbe,
    pub control: &'runtime dyn ProcessControl,
    pub(crate) ollama_models: Option<PathBuf>,
}
pub fn model_id(value: &str) -> Result<(), BackendError> {
    if value.is_empty()
        || value.len() > 8192
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:/-".contains(&byte)
        })
        || value.split('/').any(|part| part == "..")
    {
        return Err(BackendError("invalid runtime model id".into()));
    }
    Ok(())
}
impl<'runtime> RuntimeAdapter<'runtime> {
    pub fn new(
        kind: BackendKind,
        binary: PathBuf,
        http: &'runtime dyn Transport,
        probe: &'runtime dyn ProcessProbe,
        control: &'runtime dyn ProcessControl,
    ) -> Self {
        Self {
            kind,
            binary,
            http,
            probe,
            control,
            ollama_models: None,
        }
    }
    pub fn with_ollama_models(mut self, models: PathBuf) -> Result<Self, BackendError> {
        if self.kind != BackendKind::Ollama {
            return Err(BackendError(
                "model store configuration requires Ollama".into(),
            ));
        }
        crate::command::OllamaCommandContext::new("http://127.0.0.1:11434", &models)?;
        self.ollama_models = Some(models);
        Ok(self)
    }
    async fn json(
        &self,
        endpoint: &str,
        path: &str,
        cancel: &CancellationToken,
    ) -> Result<Value, BackendError> {
        let request = Request::new(endpoint, path, None, None)
            .map_err(|error| BackendError(error.to_string()))?;
        tokio::time::timeout(
            Duration::from_secs(5),
            read_json(self.http, request, cancel, 4 * 1024 * 1024),
        )
        .await
        .map_err(|_| BackendError("readiness request timed out".into()))?
        .map_err(|error| BackendError(error.to_string()))
    }
    fn validate(&self, request: &ServeRequest) -> Result<url::Url, BackendError> {
        model_id(&request.model_id)?;
        if request.draft_model_path.is_some() && self.kind != BackendKind::LlamaCpp {
            return Err(BackendError(
                "a drafter applies only to llama.cpp draft decoding".into(),
            ));
        }
        if self.kind == BackendKind::BitNet
            && request
                .context
                .is_some_and(|value| !(1..=4096).contains(&value))
        {
            return Err(BackendError(
                "BitNet context must be between 1 and 4096 tokens".into(),
            ));
        }
        if !self.binary.is_absolute()
            || request
                .context
                .is_some_and(|value| !(1..=10000000).contains(&value))
        {
            return Err(BackendError("invalid runtime configuration".into()));
        }
        crate::state::loopback(&request.endpoint).map_err(|error| BackendError(error.to_string()))
    }
    fn gguf_args(
        &self,
        request: &ServeRequest,
        host: &str,
        port: u16,
    ) -> Result<Vec<String>, BackendError> {
        let path = request
            .model_path
            .as_ref()
            .filter(|path| path.is_absolute() && path.is_file())
            .ok_or_else(|| BackendError("verified absolute GGUF path required".into()))?;
        let mut args = vec![
            "-m".into(),
            path.to_string_lossy().into_owned(),
            "--host".into(),
            host.into(),
            "--port".into(),
            port.to_string(),
            "--alias".into(),
            request.model_id.clone(),
        ];
        let context = request
            .context
            .or_else(|| (self.kind == BackendKind::BitNet).then_some(4096));
        if let Some(context) = context {
            args.extend(["--ctx-size".into(), context.to_string()]);
        }
        if self.kind == BackendKind::BitNet {
            args.extend(["-ngl".into(), "0".into()]);
        }
        let drafting = request
            .cache
            .is_some_and(|profile| profile.speculative == crate::cache::Speculative::Draft);
        match (&request.draft_model_path, drafting) {
            (None, false) => {}
            (Some(draft), true) if self.kind == BackendKind::LlamaCpp => {
                if !(draft.is_absolute() && draft.is_file())
                    || Some(draft) == request.model_path.as_ref()
                {
                    return Err(BackendError(
                        "verified absolute drafter GGUF path required".into(),
                    ));
                }
                args.extend(["--model-draft".into(), draft.to_string_lossy().into_owned()]);
            }
            (None, true) => {
                return Err(BackendError(
                    "speculative draft decoding needs a verified drafter".into(),
                ));
            }
            _ => {
                return Err(BackendError(
                    "a drafter applies only to llama.cpp draft decoding".into(),
                ));
            }
        }
        Ok(args)
    }

    async fn legacy_bitnet_identity(
        &self,
        request: &ServeRequest,
        props: Value,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        #[derive(Deserialize)]
        struct Generation {
            model: String,
            n_ctx: u32,
        }
        #[derive(Deserialize)]
        struct Props {
            default_generation_settings: Generation,
            total_slots: u32,
            chat_template: String,
        }
        let props: Props = serde_json::from_value(props)
            .map_err(|_| BackendError("invalid legacy BitNet properties".into()))?;
        if props.default_generation_settings.model != request.model_id
            || props.default_generation_settings.n_ctx != request.context.unwrap_or(4096)
            || props.total_slots != 1
            || props.chat_template.is_empty()
        {
            return Err(BackendError(
                "legacy BitNet model or context differs".into(),
            ));
        }
        let before = listener(self.probe, &request.endpoint, cancel).await?;
        if !self.trusts(&before.identity) {
            return Err(BackendError("untrusted BitNet executable".into()));
        }
        // Older BitNet exposes only an alias over HTTP; prove the exact model launch via the OS.
        let command = self
            .probe
            .command_line(before.identity.pid)
            .await
            .map_err(|error| BackendError(error.to_string()))?;
        let mut expected = vec![before.identity.executable.clone()];
        expected.extend(self.gguf_args(request, &before.address, before.port)?);
        let after = listener(self.probe, &request.endpoint, cancel).await?;
        if command != expected || !same_process(&before.identity, &after.identity) {
            return Err(BackendError("legacy BitNet launch identity differs".into()));
        }
        Ok(())
    }

    async fn ready_once(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        self.validate(request)?;
        match self.kind {
            BackendKind::Ollama => {
                let version = self.json(&request.endpoint, "/api/version", cancel).await?;
                if !version
                    .get("version")
                    .and_then(Value::as_str)
                    .is_some_and(|version| !version.is_empty() && version.len() <= 100)
                {
                    return Err(BackendError("listener is not Ollama".into()));
                }
                crate::ollama_installed::parse_inventory(
                    self.json(&request.endpoint, "/api/tags", cancel).await?,
                )
                .map_err(|error| BackendError(error.to_string()))?;
            }
            BackendKind::LlamaCpp | BackendKind::BitNet => {
                let health = self.json(&request.endpoint, "/health", cancel).await?;
                if health.get("status").and_then(Value::as_str) != Some("ok") {
                    return Err(BackendError("llama.cpp model is not ready".into()));
                }
                let props = self.json(&request.endpoint, "/props", cancel).await?;
                if self.kind == BackendKind::BitNet
                    && props.get("model_path").is_none()
                    && props.get("model_alias").is_none()
                {
                    return self.legacy_bitnet_identity(request, props, cancel).await;
                }
                let path = props.get("model_path").and_then(Value::as_str);
                let alias = props.get("model_alias").and_then(Value::as_str);
                if request
                    .model_path
                    .as_ref()
                    .is_some_and(|expected| path != expected.to_str())
                    || alias != Some(request.model_id.as_str())
                {
                    return Err(BackendError(
                        "llama.cpp loaded artifact or alias differs".into(),
                    ));
                }
            }
        }
        Ok(())
    }
    pub async fn ready(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        let operation = async {
            let mut last = BackendError("runtime is not ready".into());
            for attempt in 0..20 {
                match self.ready_once(request, cancel).await {
                    Ok(()) => return Ok(()),
                    Err(error) => last = error,
                }
                tokio::time::sleep(Duration::from_millis((100u64 << attempt.min(4)).min(2000)))
                    .await;
            }
            Err(last)
        };
        tokio::select! {biased;_=cancel.cancelled()=>Err(BackendError("cancelled".into())),result=tokio::time::timeout(Duration::from_secs(30),operation)=>result.map_err(|_|BackendError("runtime readiness timed out".into()))?}
    }
    pub async fn serve(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        let mut request = request.clone();
        if self.kind == BackendKind::BitNet {
            request.context.get_or_insert(4096);
        }
        let request = &request;
        let url = self.validate(request)?;
        if cancel.is_cancelled() {
            return Err(BackendError("cancelled".into()));
        }
        let occupied = tokio::select! {biased;_=cancel.cancelled()=>return Err(BackendError("cancelled".into())),result=self.control.occupied(&request.endpoint)=>result?};
        let executable = self
            .binary
            .to_str()
            .ok_or_else(|| BackendError("invalid executable path".into()))?;
        let (observed, owned) = if occupied {
            if self.kind == BackendKind::BitNet {
                return Err(BackendError(
                    "BitNet requires an unused port; refusing to attach to an existing listener"
                        .into(),
                ));
            }
            if crate::cache::effective(request.cache.as_ref()).is_some() {
                return Err(BackendError(format!(
                    "cache settings apply only to a {} runtime rigspark starts; stop the running server or omit the cache options",
                    self.kind.name()
                )));
            }
            let before = listener(self.probe, &request.endpoint, cancel).await?;
            if before.identity.executable != executable {
                return Err(BackendError(
                    "refusing untrusted listener executable".into(),
                ));
            }
            self.ready_once(request, cancel).await?;
            let after = listener(self.probe, &request.endpoint, cancel).await?;
            if !same_process(&before.identity, &after.identity) {
                return Err(BackendError("listener changed during attach".into()));
            }
            (after, false)
        } else {
            let host = url
                .host_str()
                .ok_or_else(|| BackendError("missing runtime host".into()))?
                .trim_matches(['[', ']']);
            let port = url
                .port_or_known_default()
                .ok_or_else(|| BackendError("missing port".into()))?;
            let mut env = minimal_env();
            let delta = crate::cache::launch_delta(request.cache.as_ref(), self.kind.name(), true)
                .map_err(|error| BackendError(error.to_string()))?;
            let mut args = match self.kind {
                BackendKind::Ollama => {
                    if let Some(models) = &self.ollama_models {
                        let context =
                            crate::command::OllamaCommandContext::new(&request.endpoint, models)?;
                        env.extend(context.environment().clone());
                    }
                    env.insert(
                        "OLLAMA_HOST".into(),
                        format!(
                            "{}:{port}",
                            if host.contains(':') {
                                format!("[{host}]")
                            } else {
                                host.into()
                            }
                        ),
                    );
                    vec!["serve".into()]
                }
                BackendKind::LlamaCpp | BackendKind::BitNet => {
                    self.gguf_args(request, host, port)?
                }
            };
            args.extend(delta.args);
            env.extend(delta.env);
            if cancel.is_cancelled() {
                return Err(BackendError("cancelled".into()));
            }
            let child = self
                .control
                .spawn(&SpawnSpec {
                    binary: self.binary.clone(),
                    args,
                    env,
                })
                .await?;
            let observed = wait_owned(
                child,
                &request.endpoint,
                executable,
                self.probe,
                cancel,
                || async { self.ready(request, cancel).await.map_err(|error| error.0) },
            )
            .await?;
            (observed, true)
        };
        let state = ServerState {
            backend: self.kind.name().into(),
            model_id: request.model_id.clone(),
            endpoint: request.endpoint.clone(),
            port: observed.port,
            owned_by_us: owned,
            pid: Some(observed.identity.pid),
            runtime_model_id: None,
            context: request.context,
            integrity: None,
            local_manifest_digest: None,
            model_path: if self.kind == BackendKind::BitNet {
                request
                    .model_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            } else {
                None
            },
            process_executable: Some(observed.identity.executable),
            process_started_at: Some(observed.identity.started),
            auth_token: None,
            cache: if owned {
                crate::cache::effective(request.cache.as_ref())
            } else {
                None
            },
        };
        state
            .validate()
            .map_err(|error| BackendError(error.to_string()))?;
        Ok(state)
    }
    pub async fn stop(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        handle
            .validate()
            .map_err(|error| BackendError(error.to_string()))?;
        if handle.backend != self.kind.name() {
            return Err(BackendError("backend handle mismatch".into()));
        }
        if !handle.owned_by_us {
            return Ok(());
        }
        let expected = ProcessIdentity {
            pid: handle
                .pid
                .ok_or_else(|| BackendError("missing PID".into()))?,
            process: String::new(),
            executable: handle
                .process_executable
                .clone()
                .ok_or_else(|| BackendError("missing executable identity".into()))?,
            started: handle
                .process_started_at
                .clone()
                .ok_or_else(|| BackendError("missing start identity".into()))?,
        };
        if self.binary.to_str() != Some(&expected.executable) {
            return Err(BackendError("untrusted stop executable".into()));
        }
        stop_owned(
            &handle.endpoint,
            &expected,
            self.probe,
            self.control,
            cancel,
        )
        .await
        .map_err(BackendError)
    }
}
#[async_trait::async_trait]
impl BackendAdapter for RuntimeAdapter<'_> {
    async fn attach_only(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        if self.kind == BackendKind::BitNet {
            return Err(BackendError(
                "BitNet requires an owned server; use up".into(),
            ));
        }
        self.validate(request)?;
        if cancel.is_cancelled() {
            return Err(BackendError("cancelled".into()));
        }
        if !self.control.occupied(&request.endpoint).await? {
            return Err(BackendError(
                "existing runtime required; refusing to spawn".into(),
            ));
        }
        let before = listener(self.probe, &request.endpoint, cancel).await?;
        if !self.trusts(&before.identity) {
            return Err(BackendError("untrusted attached executable".into()));
        }
        self.ready_once(request, cancel).await?;
        let after = listener(self.probe, &request.endpoint, cancel).await?;
        if !same_process(&before.identity, &after.identity) {
            return Err(BackendError("listener changed during attachment".into()));
        }
        let state=crate::state::RuntimeState::parse(&serde_json::json!({"schemaVersion":2,"active":{"backend":self.name(),"modelId":request.model_id,"endpoint":request.endpoint,"port":after.port,"ownedByUs":false,"pid":after.identity.pid,"processExecutable":after.identity.executable,"processStartedAt":after.identity.started}}).to_string()).map_err(|error|BackendError(error.to_string()))?;
        state
            .active
            .ok_or_else(|| BackendError("missing attached state".into()))
    }
    async fn ready_handle(
        &self,
        request: &ServeRequest,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        handle
            .validate()
            .map_err(|error| BackendError(error.to_string()))?;
        if handle.backend != self.name() || handle.endpoint != request.endpoint {
            return Err(BackendError("readiness handle mismatch".into()));
        }
        if self.kind == BackendKind::BitNet
            && (handle.model_id != request.model_id
                || handle.model_path.as_deref()
                    != request.model_path.as_ref().and_then(|path| path.to_str())
                || handle.context != Some(request.context.unwrap_or(4096)))
        {
            return Err(BackendError(
                "BitNet readiness model or context mismatch".into(),
            ));
        }
        let before = listener(self.probe, &handle.endpoint, cancel).await?;
        crate::identity::capture(handle, &before, self.trusts(&before.identity))
            .map_err(|error| BackendError(error.to_string()))?;
        self.ready(request, cancel).await?;
        let models = self.json(&handle.endpoint, "/v1/models", cancel).await?;
        #[derive(Deserialize)]
        struct Model {
            id: String,
        }
        #[derive(Deserialize)]
        struct Models {
            data: Vec<Model>,
        }
        let models: Models = serde_json::from_value(models)
            .map_err(|_| BackendError("invalid OpenAI models response".into()))?;
        if models.data.len() > 10000
            || models
                .data
                .iter()
                .any(|model| model.id.is_empty() || model.id.len() > 8192)
        {
            return Err(BackendError("invalid OpenAI models".into()));
        }
        let after = listener(self.probe, &handle.endpoint, cancel).await?;
        if !same_process(&before.identity, &after.identity) {
            return Err(BackendError("readiness listener changed".into()));
        }
        Ok(())
    }
    fn name(&self) -> &'static str {
        self.kind.name()
    }
    fn trusts(&self, identity: &ProcessIdentity) -> bool {
        self.binary.to_str() == Some(identity.executable.as_str())
    }
    async fn chat(
        &self,
        handle: &ServerState,
        input: &crate::ollama_inference::ChatInput,
        cancel: &CancellationToken,
    ) -> Result<crate::ollama_inference::ChatResult, BackendError> {
        if matches!(self.kind, BackendKind::LlamaCpp | BackendKind::BitNet) {
            return crate::openai::OpenAiInference {
                http: self.http,
                probe: self.probe,
                adapter: self,
                token: None,
            }
            .chat(handle, input, cancel)
            .await;
        }
        let expected = expected_process(handle)?;
        let mut input = input.clone();
        input.model = handle.runtime_model_id.clone().unwrap_or(input.model);
        crate::ollama_inference::OllamaInference::new(
            self.http,
            self.probe,
            self.binary
                .to_str()
                .ok_or_else(|| BackendError("invalid executable".into()))?,
        )
        .chat(&handle.endpoint, &expected, &input, cancel)
        .await
        .map_err(|error| BackendError(error.to_string()))
    }
    async fn chat_stream(
        &self,
        handle: &ServerState,
        input: &crate::ollama_inference::ChatInput,
        cancel: &CancellationToken,
        on_delta: &mut (dyn for<'chunk> FnMut(&'chunk str) + Send),
    ) -> Result<crate::ollama_inference::ChatResult, BackendError> {
        if matches!(self.kind, BackendKind::LlamaCpp | BackendKind::BitNet) {
            let result = self.chat(handle, input, cancel).await?;
            on_delta(&result.content);
            return Ok(result);
        }
        let expected = expected_process(handle)?;
        let mut input = input.clone();
        input.model = handle.runtime_model_id.clone().unwrap_or(input.model);
        crate::ollama_inference::OllamaInference::new(
            self.http,
            self.probe,
            self.binary
                .to_str()
                .ok_or_else(|| BackendError("invalid executable".into()))?,
        )
        .chat_stream(&handle.endpoint, &expected, &input, cancel, on_delta)
        .await
        .map_err(|error| BackendError(error.to_string()))
    }
    async fn embed(
        &self,
        handle: &ServerState,
        model: &str,
        inputs: &[String],
        cancel: &CancellationToken,
    ) -> Result<crate::ollama_inference::EmbedResult, BackendError> {
        if self.kind != BackendKind::Ollama {
            return Err(BackendError(format!(
                "{} does not serve embeddings",
                self.name()
            )));
        }
        let expected = expected_process(handle)?;
        crate::ollama_inference::OllamaInference::new(
            self.http,
            self.probe,
            self.binary
                .to_str()
                .ok_or_else(|| BackendError("invalid executable".into()))?,
        )
        .embed(&handle.endpoint, &expected, model, inputs, cancel)
        .await
        .map_err(|error| BackendError(error.to_string()))
    }
    async fn serve(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        RuntimeAdapter::serve(self, request, cancel).await
    }
    async fn ready(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        RuntimeAdapter::ready(self, request, cancel).await
    }
    async fn stop(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        RuntimeAdapter::stop(self, handle, cancel).await
    }
}
fn expected_process(handle: &ServerState) -> Result<ProcessIdentity, BackendError> {
    handle
        .validate()
        .map_err(|error| BackendError(error.to_string()))?;
    if handle.backend != "ollama" {
        return Err(BackendError("Ollama handle required".into()));
    }
    Ok(ProcessIdentity {
        pid: handle
            .pid
            .ok_or_else(|| BackendError("expected PID required".into()))?,
        process: String::new(),
        executable: handle
            .process_executable
            .clone()
            .ok_or_else(|| BackendError("expected executable required".into()))?,
        started: handle
            .process_started_at
            .clone()
            .ok_or_else(|| BackendError("expected start identity required".into()))?,
    })
}
