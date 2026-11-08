use rigspark_runtime::{
    adapters::{BackendKind, RuntimeAdapter, ServeRequest},
    http::{HttpError, Request, Response, Transport},
    identity::{Listener, ProcessIdentity, ProcessProbe},
    process_control::{ChildProcess, ProcessControl, SpawnSpec},
    state::StateError,
};
use serde_json::json;
use std::{path::PathBuf, sync::Mutex};
use tokio_util::sync::CancellationToken;
#[cfg(windows)]
const RUNTIME: &str = "C:/opt/runtime";
#[cfg(not(windows))]
const RUNTIME: &str = "/opt/runtime";
#[cfg(windows)]
const STUDIO: &str = "C:/trusted/studio";
#[cfg(not(windows))]
const STUDIO: &str = "/trusted/studio";
#[cfg(windows)]
const LMS: &str = "C:/opt/lms";
#[cfg(not(windows))]
const LMS: &str = "/opt/lms";
struct Probe {
    binary: String,
}
#[async_trait::async_trait]
impl ProcessProbe for Probe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Ok(Listener {
            identity: ProcessIdentity {
                pid: 123,
                process: "runtime".into(),
                executable: self.binary.clone(),
                started: "instance".into(),
            },
            address: host.into(),
            port,
        })
    }
    async fn process(&self, _pid: u32) -> Result<ProcessIdentity, StateError> {
        Ok(self.listener(11435, "127.0.0.1").await?.identity)
    }
}
struct Control {
    signals: Mutex<Vec<u32>>,
}
#[async_trait::async_trait]
impl ProcessControl for Control {
    async fn occupied(&self, _endpoint: &str) -> Result<bool, String> {
        Ok(true)
    }
    async fn spawn(&self, _spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>, String> {
        panic!("must not spawn on an occupied port")
    }
    async fn signal(&self, identity: &ProcessIdentity, _force: bool) -> Result<(), String> {
        self.signals.lock().unwrap().push(identity.pid);
        Ok(())
    }
    async fn alive(&self, _pid: u32) -> Result<bool, String> {
        Ok(false)
    }
}
struct Http;
struct ModelHttp {
    path: String,
    mlx: bool,
}
struct LegacyBitNetHttp(serde_json::Value);
#[async_trait::async_trait]
impl Transport for LegacyBitNetHttp {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        let body = match request.url.path() {
            "/health" => json!({"status":"ok"}),
            "/props" => self.0.clone(),
            "/v1/models" => json!({"data":[{"id":"test:latest"}]}),
            _ => panic!("unexpected legacy BitNet request"),
        };
        Ok(Response {
            status: 200,
            body: Box::pin(std::io::Cursor::new(body.to_string().into_bytes())),
        })
    }
}
#[async_trait::async_trait]
impl Transport for ModelHttp {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        let body = match request.url.path() {
            "/health" => json!({"status":"ok"}),
            "/props" => json!({"model_path":self.path,"model_alias":"test:latest"}),
            "/v1/models" => {
                json!({"data":[{"id":if self.mlx{self.path.as_str()}else{"test:latest"}}]})
            }
            "/v1/chat/completions" => {
                assert_eq!(
                    request.body().unwrap()["model"],
                    if self.mlx {
                        "default_model"
                    } else {
                        "test:latest"
                    }
                );
                json!({"choices":[{"message":{"content":"ready"}}]})
            }
            _ => panic!("unexpected model request"),
        };
        Ok(Response {
            status: 200,
            body: Box::pin(std::io::Cursor::new(body.to_string().into_bytes())),
        })
    }
}
#[tokio::test]
async fn llama_startup_uses_exact_artifact_and_alias() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("weights.gguf");
    std::fs::write(&path, b"verified").unwrap();
    let http = ModelHttp {
        path: path.to_string_lossy().into_owned(),
        mlx: false,
    };
    let probe = Probe {
        binary: RUNTIME.into(),
    };
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let adapter = RuntimeAdapter::new(
        BackendKind::LlamaCpp,
        PathBuf::from(RUNTIME),
        &http,
        &probe,
        &control,
    );
    let input = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(path.clone()),
        context: None,
        cache: None,
        slot_save_path: None,
    };
    assert!(
        adapter
            .serve(&input, &CancellationToken::new())
            .await
            .unwrap()
            .owned_by_us
    );
    assert_eq!(
        *control.args.lock().unwrap(),
        vec![
            "-m",
            path.to_str().unwrap(),
            "--host",
            "127.0.0.1",
            "--port",
            "11435",
            "--alias",
            "test:latest"
        ]
    );
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[tokio::test]
async fn mlx_owned_startup_is_isolated_authenticated_and_bound_to_exact_model() {
    use rigspark_runtime::{adapters::BackendAdapter, special_adapters::MlxAdapter};
    struct Version;
    #[async_trait::async_trait]
    impl rigspark_runtime::command::CommandRunner for Version {
        async fn run(
            &self,
            _binary: &std::path::Path,
            args: &[String],
            _cancel: &CancellationToken,
            _timeout: std::time::Duration,
        ) -> Result<String, String> {
            assert_eq!(args[0], "-I");
            Ok("0.31.3\n".into())
        }
    }
    let root = tempfile::tempdir().unwrap();
    for file in [
        "config.json",
        "tokenizer_config.json",
        "weights.safetensors",
    ] {
        std::fs::write(root.path().join(file), b"{}").unwrap();
    }
    let http = ModelHttp {
        path: root.path().to_string_lossy().into_owned(),
        mlx: true,
    };
    let probe = Probe {
        binary: "/trusted/python".into(),
    };
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let adapter = MlxAdapter {
        binary: PathBuf::from("/trusted/python"),
        http: &http,
        probe: &probe,
        control: &control,
        commands: &Version,
        token: None,
    };
    let input = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(root.path().to_owned()),
        context: None,
        cache: None,
        slot_save_path: None,
    };
    let active = adapter
        .serve(&input, &CancellationToken::new())
        .await
        .unwrap();
    assert!(active.owned_by_us);
    assert_eq!(active.auth_token.as_ref().unwrap().len(), 64);
    let args = control.args.lock().unwrap();
    assert_eq!(&args[..2], ["-I", "-c"]);
    assert!(args[2].contains("hmac.compare_digest"));
    assert!(args[2].contains("4194304"));
    assert_eq!(args[5], root.path().to_str().unwrap());
    assert_eq!(
        control.env.lock().unwrap()["LLMUP_MLX_AUTH_TOKEN"],
        *active.auth_token.as_ref().unwrap()
    );
}
#[tokio::test]
async fn attach_only_never_spawns_when_daemon_disappears() {
    use rigspark_runtime::adapters::BackendAdapter;
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let probe = Probe {
        binary: RUNTIME.into(),
    };
    let adapter = RuntimeAdapter::new(
        BackendKind::Ollama,
        PathBuf::from(RUNTIME),
        &Http,
        &probe,
        &control,
    );
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
        slot_save_path: None,
    };
    assert!(
        adapter
            .attach_only(&request, &CancellationToken::new())
            .await
            .is_err()
    );
    assert!(control.args.lock().unwrap().is_empty());
}
struct Spawned {
    cleaned: std::sync::Arc<Mutex<bool>>,
}

struct CommandProbe {
    command: Vec<String>,
}
#[async_trait::async_trait]
impl ProcessProbe for CommandProbe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Probe {
            binary: RUNTIME.into(),
        }
        .listener(port, host)
        .await
    }
    async fn process(&self, pid: u32) -> Result<ProcessIdentity, StateError> {
        Probe {
            binary: RUNTIME.into(),
        }
        .process(pid)
        .await
    }
    async fn command_line(&self, _pid: u32) -> Result<Vec<String>, StateError> {
        Ok(self.command.clone())
    }
}
fn legacy_bitnet_props() -> serde_json::Value {
    json!({
        "default_generation_settings":{"model":"test:latest","n_ctx":2048},
        "total_slots":1,
        "chat_template":"test"
    })
}
fn legacy_bitnet_command(path: &std::path::Path) -> Vec<String> {
    [
        RUNTIME,
        "-m",
        path.to_str().unwrap(),
        "--host",
        "127.0.0.1",
        "--port",
        "11435",
        "--alias",
        "test:latest",
        "--ctx-size",
        "2048",
        "-ngl",
        "0",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[tokio::test]
async fn legacy_bitnet_verifies_owned_launch_when_props_omit_model_path() {
    use rigspark_runtime::adapters::BackendAdapter;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("weights.gguf");
    std::fs::write(&path, b"verified").unwrap();
    let probe = CommandProbe {
        command: legacy_bitnet_command(&path),
    };
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let http = LegacyBitNetHttp(legacy_bitnet_props());
    let adapter = RuntimeAdapter::new(
        BackendKind::BitNet,
        PathBuf::from(RUNTIME),
        &http,
        &probe,
        &control,
    );
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(path),
        context: Some(2048),
        cache: None,
        slot_save_path: None,
    };
    let cancel = CancellationToken::new();
    let handle = adapter.serve(&request, &cancel).await.unwrap();
    assert!(handle.owned_by_us);
    assert_eq!(*control.args.lock().unwrap(), probe.command[1..]);
    adapter
        .ready_handle(&request, &handle, &cancel)
        .await
        .unwrap();
    adapter.stop(&handle, &cancel).await.unwrap();
}

#[tokio::test]
async fn legacy_bitnet_rejects_wrong_launch_metadata_and_modern_identity_mismatches() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("weights.gguf");
    std::fs::write(&path, b"verified").unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(path.clone()),
        context: Some(2048),
        cache: None,
        slot_save_path: None,
    };
    let mut tasks = tokio::task::JoinSet::new();
    for case in 0..10 {
        let path = path.clone();
        let request = request.clone();
        tasks.spawn(async move {
            let mut command = legacy_bitnet_command(&path);
            let mut props = legacy_bitnet_props();
            match case {
                0 => command[2] = "/wrong/weights.gguf".into(),
                1 => command[8] = "wrong:latest".into(),
                2 => command.extend(["--model".into(), "/wrong/weights.gguf".into()]),
                3 => props["default_generation_settings"]["model"] = json!("wrong:latest"),
                4 => props["default_generation_settings"]["n_ctx"] = json!(4096),
                5 => props["model_path"] = json!("/wrong/weights.gguf"),
                6 => props["model_alias"] = serde_json::Value::Null,
                7 => props["chat_template"] = serde_json::Value::Null,
                8 | 9 => (),
                _ => unreachable!(),
            }
            let probe = CommandProbe { command };
            let control = SpawnControl {
                args: Mutex::new(Vec::new()),
                env: Mutex::new(Default::default()),
                cleaned: std::sync::Arc::new(Mutex::new(false)),
            };
            let http = LegacyBitNetHttp(props);
            let unavailable = Probe {
                binary: RUNTIME.into(),
            };
            let probe: &dyn ProcessProbe = if case == 8 { &unavailable } else { &probe };
            let adapter = RuntimeAdapter::new(
                if case == 9 {
                    BackendKind::LlamaCpp
                } else {
                    BackendKind::BitNet
                },
                PathBuf::from(RUNTIME),
                &http,
                probe,
                &control,
            );
            assert!(
                adapter
                    .serve(&request, &CancellationToken::new())
                    .await
                    .is_err(),
                "case {case}"
            );
            assert!(*control.cleaned.lock().unwrap(), "case {case}");
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

#[tokio::test]
async fn bitnet_uses_cpu_server_custom_port_context_openai_chat_and_owned_stop() {
    use rigspark_runtime::adapters::BackendAdapter;
    use rigspark_runtime::{
        lifecycle::{Lifecycle, Registry},
        state::{Config, StateStore},
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("ggml-model-i2_s.gguf");
    std::fs::write(&path, b"verified").unwrap();
    let http = ModelHttp {
        path: path.to_string_lossy().into_owned(),
        mlx: false,
    };
    let probe = Probe {
        binary: RUNTIME.into(),
    };
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let adapter = RuntimeAdapter::new(
        BackendKind::BitNet,
        PathBuf::from(RUNTIME),
        &http,
        &probe,
        &control,
    );
    let mut request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(path.clone()),
        context: Some(2048),
        cache: None,
        slot_save_path: None,
    };
    let store = StateStore::new(Config::from_home(root.path().join("state")).unwrap());
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &probe,
    };
    let cancel = CancellationToken::new();
    let reviewed = lifecycle.review(&request.model_id, &cancel).await.unwrap();
    let handle = lifecycle
        .replace("bitnet", &request, &reviewed, &cancel)
        .await
        .unwrap();
    assert_eq!(store.read().unwrap().active.as_ref(), Some(&handle));
    assert_eq!(handle.backend, "bitnet");
    assert_eq!(handle.context, Some(2048));
    assert!(handle.owned_by_us);
    assert_eq!(handle.model_path.as_deref(), path.to_str());
    assert_eq!(
        *control.args.lock().unwrap(),
        [
            "-m",
            path.to_str().unwrap(),
            "--host",
            "127.0.0.1",
            "--port",
            "11435",
            "--alias",
            "test:latest",
            "--ctx-size",
            "2048",
            "-ngl",
            "0",
        ]
    );
    adapter
        .ready_handle(&request, &handle, &CancellationToken::new())
        .await
        .unwrap();
    lifecycle.health(&handle, &request, &cancel).await.unwrap();
    let input = rigspark_runtime::ollama_inference::ChatInput {
        model: "test:latest".into(),
        messages: vec![rigspark_runtime::ollama_inference::ChatMessage {
            role: "user".into(),
            content: "hello".into(),
            tool_calls: vec![],
            tool_name: None,
        }],
        tools: vec![],
        temperature: None,
        slot: None,
        max_tokens: None,
    };
    assert_eq!(
        adapter
            .chat(&handle, &input, &CancellationToken::new())
            .await
            .unwrap()
            .content,
        "ready"
    );
    assert!(!adapter.can_embed());
    assert!(!adapter.can_stream());
    lifecycle.down(&cancel).await.unwrap();
    assert!(store.read().unwrap().active.is_none());
    request.cache = Some(q8_cache());
    assert!(
        adapter
            .serve(&request, &CancellationToken::new())
            .await
            .unwrap_err()
            .0
            .contains("bitnet")
    );
    request.cache = None;
    request.context = Some(4097);
    assert!(
        adapter
            .serve(&request, &CancellationToken::new())
            .await
            .is_err()
    );
}
#[async_trait::async_trait]
impl ChildProcess for Spawned {
    fn pid(&self) -> u32 {
        123
    }
    fn exited(&mut self) -> Result<bool, String> {
        Ok(false)
    }
    async fn terminate(&mut self) -> Result<(), String> {
        *self.cleaned.lock().unwrap() = true;
        Ok(())
    }
    fn detach(&mut self) {}
}
struct SpawnControl {
    args: Mutex<Vec<String>>,
    env: Mutex<std::collections::BTreeMap<String, String>>,
    cleaned: std::sync::Arc<Mutex<bool>>,
}
#[async_trait::async_trait]
impl ProcessControl for SpawnControl {
    async fn occupied(&self, _endpoint: &str) -> Result<bool, String> {
        Ok(false)
    }
    async fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>, String> {
        *self.args.lock().unwrap() = spec.args.clone();
        *self.env.lock().unwrap() = spec.env.clone();
        Ok(Box::new(Spawned {
            cleaned: self.cleaned.clone(),
        }))
    }
    async fn signal(&self, _process: &ProcessIdentity, _force: bool) -> Result<(), String> {
        Ok(())
    }
    async fn alive(&self, _pid: u32) -> Result<bool, String> {
        Ok(false)
    }
}
#[tokio::test]
async fn ollama_startup_binds_custom_loopback_port_and_cleans_untrusted_readiness() {
    let store = tempfile::tempdir().unwrap();
    for executable in [RUNTIME, "/tmp/untrusted"] {
        let cleaned = std::sync::Arc::new(Mutex::new(false));
        let control = SpawnControl {
            args: Mutex::new(Vec::new()),
            env: Mutex::new(Default::default()),
            cleaned: cleaned.clone(),
        };
        let probe = Probe {
            binary: executable.into(),
        };
        let adapter = RuntimeAdapter::new(
            BackendKind::Ollama,
            PathBuf::from(RUNTIME),
            &Http,
            &probe,
            &control,
        )
        .with_ollama_models(store.path().to_path_buf())
        .unwrap();
        let request = ServeRequest {
            draft_model_path: None,
            model_id: "test:latest".into(),
            endpoint: "http://127.0.0.1:11435".into(),
            model_path: None,
            context: None,
            cache: None,
            slot_save_path: None,
        };
        let result = adapter.serve(&request, &CancellationToken::new()).await;
        assert_eq!(result.is_ok(), executable == RUNTIME);
        if let Ok(handle) = result {
            assert!(handle.owned_by_us);
        }
        assert_eq!(*cleaned.lock().unwrap(), executable != RUNTIME);
        assert_eq!(*control.args.lock().unwrap(), vec!["serve"]);
        assert_eq!(
            control.env.lock().unwrap()["OLLAMA_HOST"],
            "127.0.0.1:11435"
        );
        assert_eq!(
            control.env.lock().unwrap()["OLLAMA_MODELS"],
            store.path().to_str().unwrap()
        );
        let allowed = [
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TMPDIR",
            "TMP",
            "TEMP",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "PATHEXT",
            "OLLAMA_HOST",
            "OLLAMA_MODELS",
        ];
        let leaked: Vec<_> = control
            .env
            .lock()
            .unwrap()
            .keys()
            .filter(|key| !allowed.contains(&key.as_str()))
            .cloned()
            .collect();
        assert!(
            leaked.is_empty(),
            "ambient variables reached the daemon: {leaked:?}"
        );
    }
}
#[async_trait::async_trait]
impl Transport for Http {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        let body = match request.url.path() {
            "/api/version" => json!({"version":"0.11.4"}),
            "/api/tags" => json!({"models":[]}),
            "/v1/models" => json!({"data":[{"id":"test:latest"}]}),
            "/health" => json!({"status":"ok"}),
            "/props" => json!({"model_path":"/verified/weights.gguf","model_alias":"test:latest"}),
            _ => panic!("unexpected request"),
        };
        Ok(Response {
            status: 200,
            body: Box::pin(std::io::Cursor::new(serde_json::to_vec(&body).unwrap())),
        })
    }
}
#[tokio::test]
async fn attach_preserves_foreign_ownership_and_stop_never_signals_it() {
    for kind in [BackendKind::Ollama, BackendKind::LlamaCpp] {
        let control = Control {
            signals: Mutex::new(Vec::new()),
        };
        let probe = Probe {
            binary: RUNTIME.into(),
        };
        let adapter = RuntimeAdapter::new(kind, PathBuf::from(RUNTIME), &Http, &probe, &control);
        let request = ServeRequest {
            draft_model_path: None,
            model_id: "test:latest".into(),
            endpoint: "http://127.0.0.1:11435".into(),
            model_path: Some(PathBuf::from("/verified/weights.gguf")),
            context: None,
            cache: None,
            slot_save_path: None,
        };
        let handle = adapter
            .serve(&request, &CancellationToken::new())
            .await
            .unwrap();
        assert!(!handle.owned_by_us);
        assert_eq!(handle.pid, Some(123));
        adapter
            .stop(&handle, &CancellationToken::new())
            .await
            .unwrap();
        assert!(control.signals.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn untrusted_listener_is_rejected_before_attachment() {
    let control = Control {
        signals: Mutex::new(Vec::new()),
    };
    let probe = Probe {
        binary: "/tmp/unknown".into(),
    };
    let adapter = RuntimeAdapter::new(
        BackendKind::Ollama,
        PathBuf::from(RUNTIME),
        &Http,
        &probe,
        &control,
    );
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
        slot_save_path: None,
    };
    assert!(
        adapter
            .serve(&request, &CancellationToken::new())
            .await
            .is_err()
    );
}

struct Commands;
#[async_trait::async_trait]
impl rigspark_runtime::command::CommandRunner for Commands {
    async fn run(
        &self,
        _binary: &std::path::Path,
        args: &[String],
        _cancel: &CancellationToken,
        _timeout: std::time::Duration,
    ) -> Result<String, String> {
        Ok(if args.first().map(String::as_str) == Some("server") {
            json!({"running":true,"port":11435})
        } else {
            json!([{"identifier":"test:latest","path":"owner/model/weights.gguf"}])
        }
        .to_string())
    }
}
struct StudioHttp;
#[async_trait::async_trait]
impl Transport for StudioHttp {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        let body = match request.url.path() {
            "/lmstudio-greeting" => json!({"lmstudio":true}),
            "/v1/models" => json!({"data":[{"id":"test:latest"}]}),
            _ => panic!("unexpected Studio request"),
        };
        Ok(Response {
            status: 200,
            body: Box::pin(std::io::Cursor::new(body.to_string().into_bytes())),
        })
    }
}
#[tokio::test]
async fn studio_requires_exact_delegated_path_and_never_owns_listener() {
    use rigspark_runtime::{adapters::BackendAdapter, special_adapters::LmStudioAdapter};
    let probe = Probe {
        binary: STUDIO.into(),
    };
    let adapter = LmStudioAdapter {
        binary: PathBuf::from(LMS),
        trusted_executables: vec![PathBuf::from(STUDIO)],
        http: &StudioHttp,
        probe: &probe,
        commands: &Commands,
        token: None,
    };
    let mut request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(PathBuf::from("owner/model/weights.gguf")),
        context: None,
        cache: None,
        slot_save_path: None,
    };
    let state = adapter
        .serve(&request, &CancellationToken::new())
        .await
        .unwrap();
    assert!(!state.owned_by_us);
    assert_eq!(
        state.model_path.as_deref(),
        Some("owner/model/weights.gguf")
    );
    request.model_path = Some(PathBuf::from("other/weights.gguf"));
    assert!(
        adapter
            .serve(&request, &CancellationToken::new())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn mlx_never_attaches_to_an_existing_listener() {
    use rigspark_runtime::{adapters::BackendAdapter, special_adapters::MlxAdapter};
    let probe = Probe {
        binary: "/trusted/python".into(),
    };
    let control = Control {
        signals: Mutex::new(Vec::new()),
    };
    let adapter = MlxAdapter {
        binary: PathBuf::from("/trusted/python"),
        http: &Http,
        probe: &probe,
        control: &control,
        commands: &Commands,
        token: None,
    };
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(PathBuf::from("/verified/model")),
        context: None,
        cache: None,
        slot_save_path: None,
    };
    assert!(
        adapter
            .serve(&request, &CancellationToken::new())
            .await
            .is_err()
    );
}

#[test]
fn mlx_accepts_only_the_framework_host_of_the_resolved_interpreter() {
    use rigspark_runtime::special_adapters::mlx_process_executable;
    let framework =
        "/opt/homebrew/Cellar/python@3.14/3.14.6/Frameworks/Python.framework/Versions/3.14";
    let host = PathBuf::from(format!(
        "{framework}/Resources/Python.app/Contents/MacOS/Python"
    ));
    for interpreter in ["python3.14", "python3"] {
        let binary = PathBuf::from(format!("{framework}/bin/{interpreter}"));
        assert_eq!(mlx_process_executable(&binary), host);
    }
    for plain in [
        "/usr/bin/python3",
        "/opt/venv/bin/python3",
        "/Library/Frameworks/Python.framework/Versions/3.12/bin/pip3",
        "/tmp/Python.framework/bin/python3",
        "/tmp/Versions/3.12/bin/python3",
    ] {
        assert_eq!(
            mlx_process_executable(&PathBuf::from(plain)),
            PathBuf::from(plain),
            "{plain}"
        );
    }
}

#[test]
fn lm_studio_trust_matches_windows_paths_case_insensitively_without_widening() {
    use rigspark_runtime::special_adapters::trusted_executable;
    let trusted = vec![
        PathBuf::from(r"\\?\C:\Users\Me\AppData\Local\Programs\LM Studio\LM Studio.exe"),
        PathBuf::from(r"C:\Users\Me\AppData\Local/Programs/LM Studio/llmster.exe"),
    ];
    for observed in [
        r"c:\users\me\appdata\local\programs\lm studio\lm studio.exe",
        r"C:\Users\Me\AppData\Local\Programs\LM Studio\llmster.exe",
    ] {
        assert!(trusted_executable(&trusted, observed, true), "{observed}");
    }
    for observed in [
        r"C:\Users\Me\AppData\Local\Programs\LM Studio\evil.exe",
        r"D:\Users\Me\AppData\Local\Programs\LM Studio\LM Studio.exe",
        "LM Studio.exe",
    ] {
        assert!(!trusted_executable(&trusted, observed, true), "{observed}");
    }
    let unix = vec![PathBuf::from("/usr/bin/llmster")];
    assert!(trusted_executable(&unix, "/usr/bin/llmster", false));
    assert!(!trusted_executable(&unix, "/usr/bin/LLMSTER", false));
    assert!(!trusted_executable(
        &[PathBuf::from("llmster")],
        "llmster",
        false
    ));
}

#[test]
fn mlx_model_directories_must_be_data_only() {
    use rigspark_runtime::special_adapters::validate_mlx_directory;
    let directory = |config: serde_json::Value| {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.json"), config.to_string()).unwrap();
        std::fs::write(root.path().join("tokenizer_config.json"), "{}").unwrap();
        std::fs::write(root.path().join("model.safetensors"), "weights").unwrap();
        root
    };
    let clean = directory(json!({"model_type":"llama"}));
    validate_mlx_directory(clean.path()).unwrap();
    for config in [
        json!({"model_type":"llama","model_file":"model.py"}),
        json!({"model_type":"llama","model_file":"/tmp/outside.py"}),
        json!({"auto_map":{"AutoModel":"model.CustomModel"}}),
        json!({"trust_remote_code":true}),
    ] {
        let root = directory(config.clone());
        assert!(validate_mlx_directory(root.path()).is_err(), "{config}");
    }
    let code = directory(json!({"model_type":"llama"}));
    std::fs::write(
        code.path().join("model.py"),
        "raise RuntimeError('must not execute')",
    )
    .unwrap();
    assert!(validate_mlx_directory(code.path()).is_err());
    let empty = directory(json!({"model_type":"llama"}));
    std::fs::remove_file(empty.path().join("model.safetensors")).unwrap();
    assert!(validate_mlx_directory(empty.path()).is_err());
}

fn q8_cache() -> rigspark_runtime::cache::CacheProfile {
    rigspark_runtime::cache::CacheProfile {
        kv_k: rigspark_core::sizing::KvCacheType::Q8_0,
        kv_v: rigspark_core::sizing::KvCacheType::Q8_0,
        ..Default::default()
    }
}

#[tokio::test]
async fn owned_launches_apply_the_cache_profile_and_record_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("weights.gguf");
    std::fs::write(&path, b"verified").unwrap();
    let http = ModelHttp {
        path: path.to_string_lossy().into_owned(),
        mlx: false,
    };
    let probe = Probe {
        binary: RUNTIME.into(),
    };
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let llama = RuntimeAdapter::new(
        BackendKind::LlamaCpp,
        PathBuf::from(RUNTIME),
        &http,
        &probe,
        &control,
    );
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(path.clone()),
        context: Some(8192),
        cache: Some(q8_cache()),
        slot_save_path: None,
    };
    let handle = llama
        .serve(&request, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(handle.cache, Some(q8_cache()));
    assert_eq!(
        control.args.lock().unwrap()[8..],
        [
            "--ctx-size",
            "8192",
            "--cache-type-k",
            "q8_0",
            "--cache-type-v",
            "q8_0",
            "--flash-attn",
            "on"
        ]
    );
    let store = tempfile::tempdir().unwrap();
    let ollama = RuntimeAdapter::new(
        BackendKind::Ollama,
        PathBuf::from(RUNTIME),
        &Http,
        &probe,
        &control,
    )
    .with_ollama_models(store.path().to_path_buf())
    .unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: Some(q8_cache()),
        slot_save_path: None,
    };
    let handle = ollama
        .serve(&request, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(handle.cache, Some(q8_cache()));
    assert_eq!(*control.args.lock().unwrap(), vec!["serve"]);
    let env = control.env.lock().unwrap();
    assert_eq!(env["OLLAMA_KV_CACHE_TYPE"], "q8_0");
    assert_eq!(env["OLLAMA_FLASH_ATTENTION"], "1");
}

#[tokio::test]
async fn cache_settings_are_refused_for_a_runtime_rigspark_did_not_start() {
    let probe = Probe {
        binary: RUNTIME.into(),
    };
    let control = Control {
        signals: Mutex::new(Vec::new()),
    };
    for kind in [BackendKind::Ollama, BackendKind::LlamaCpp] {
        let adapter = RuntimeAdapter::new(kind, PathBuf::from(RUNTIME), &Http, &probe, &control);
        let request = ServeRequest {
            draft_model_path: None,
            model_id: "test:latest".into(),
            endpoint: "http://127.0.0.1:11435".into(),
            model_path: None,
            context: None,
            cache: Some(q8_cache()),
            slot_save_path: None,
        };
        let error = adapter
            .serve(&request, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.0.contains("starts"), "{}", error.0);
    }
    assert!(control.signals.lock().unwrap().is_empty());
}

#[tokio::test]
async fn llama_cpp_launches_a_verified_drafter_and_refuses_unverified_ones() {
    use rigspark_runtime::cache::{CacheProfile, Speculative};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("weights.gguf");
    let draft = root.path().join("draft.gguf");
    std::fs::write(&path, b"verified").unwrap();
    std::fs::write(&draft, b"verified drafter").unwrap();
    let http = ModelHttp {
        path: path.to_string_lossy().into_owned(),
        mlx: false,
    };
    let probe = Probe {
        binary: RUNTIME.into(),
    };
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let llama = RuntimeAdapter::new(
        BackendKind::LlamaCpp,
        PathBuf::from(RUNTIME),
        &http,
        &probe,
        &control,
    );
    let drafting = CacheProfile {
        speculative: Speculative::Draft,
        parallel: Some(4),
        ..CacheProfile::default()
    };
    let request = ServeRequest {
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(path.clone()),
        draft_model_path: Some(draft.clone()),
        context: None,
        cache: Some(drafting),
        slot_save_path: None,
    };
    let handle = llama
        .serve(&request, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(handle.cache, Some(drafting));
    let draft_text = draft.to_string_lossy().into_owned();
    assert_eq!(
        control.args.lock().unwrap()[8..],
        [
            "--model-draft",
            draft_text.as_str(),
            "--spec-type",
            "draft-simple",
            "--parallel",
            "4",
            "--kv-unified"
        ]
    );
    let cancel = CancellationToken::new();
    for bad in [
        ServeRequest {
            draft_model_path: None,
            ..request.clone()
        },
        ServeRequest {
            draft_model_path: Some(root.path().join("missing.gguf")),
            ..request.clone()
        },
        ServeRequest {
            draft_model_path: Some(path.clone()),
            ..request.clone()
        },
        ServeRequest {
            cache: None,
            ..request.clone()
        },
    ] {
        assert!(llama.serve(&bad, &cancel).await.is_err());
    }
    let ollama = RuntimeAdapter::new(
        BackendKind::Ollama,
        PathBuf::from(RUNTIME),
        &Http,
        &probe,
        &control,
    );
    assert!(
        ollama
            .serve(
                &ServeRequest {
                    model_path: None,
                    cache: None,
                    ..request.clone()
                },
                &cancel
            )
            .await
            .is_err(),
        "only llama.cpp takes a drafter"
    );
}

/// llama.cpp answering chat with its own timings and slot actions, recording each request.
struct SlotHttp {
    requests: Mutex<Vec<(String, Option<String>, serde_json::Value)>>,
}
#[async_trait::async_trait]
impl Transport for SlotHttp {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        let path = request.url.path().to_owned();
        let query = request.url.query().map(str::to_owned);
        let sent = request.body().cloned().unwrap_or_default();
        self.requests
            .lock()
            .unwrap()
            .push((path.clone(), query.clone(), sent));
        let body = match (path.as_str(), query.as_deref()) {
            ("/v1/chat/completions", _) => json!({
                "choices": [{"message": {"content": "done"}}],
                "timings": {"cache_n": 40, "prompt_n": 12, "prompt_ms": 30.0,
                    "predicted_n": 50, "predicted_ms": 1000.0,
                    "draft_n": 20, "draft_n_accepted": 15}
            }),
            ("/slots/2", Some("action=save")) => json!({"id_slot": 2, "filename": "spark-a.bin",
                "n_saved": 1745, "n_written": 14309796, "timings": {"save_ms": 49.865}}),
            ("/slots/3", Some("action=restore")) => json!({"id_slot": 3, "filename": "spark-a.bin",
                "n_restored": 1745, "n_read": 14309796, "timings": {"restore_ms": 42.937}}),
            ("/slots/2", Some("action=erase")) => json!({"id_slot": 2, "n_erased": 1745}),
            ("/slots/4", Some("action=erase")) => json!({"id_slot": 5, "n_erased": 1}),
            ("/slots/5", Some("action=save")) => json!({"id_slot": 5, "n_saved": 1,
                "n_written": 1, "timings": {"save_ms": -1.0}}),
            ("/slots/6", Some("action=save")) => json!({"id_slot": 6, "n_saved": 1,
                "n_written": 1, "timings": {"save_ms": 0.0}}),
            // Larger than 1 KiB, well inside the 64 KiB reply limit.
            ("/slots/7", Some("action=save")) => json!({"id_slot": 7, "n_saved": 1,
                "filename": "x".repeat(4000), "timings": {"save_ms": 1.0}}),
            other => panic!("unexpected request {other:?}"),
        };
        Ok(Response {
            status: 200,
            body: Box::pin(std::io::Cursor::new(body.to_string().into_bytes())),
        })
    }
}

#[tokio::test]
async fn llama_cpp_keeps_kv_slots_pins_requests_and_reports_its_own_timings() {
    use rigspark_runtime::{
        ollama_inference::{ChatInput, ChatMessage, Timings},
        openai::{KvSlotAction, KvSlotReport, OpenAiInference},
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("weights.gguf");
    std::fs::write(&path, b"verified").unwrap();
    let slots = root.path().join("kv-slots");
    std::fs::create_dir(&slots).unwrap();
    let http = ModelHttp {
        path: path.to_string_lossy().into_owned(),
        mlx: false,
    };
    let probe = Probe {
        binary: RUNTIME.into(),
    };
    let control = SpawnControl {
        args: Mutex::new(Vec::new()),
        env: Mutex::new(Default::default()),
        cleaned: std::sync::Arc::new(Mutex::new(false)),
    };
    let llama = RuntimeAdapter::new(
        BackendKind::LlamaCpp,
        PathBuf::from(RUNTIME),
        &http,
        &probe,
        &control,
    );
    let request = ServeRequest {
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: Some(path.clone()),
        draft_model_path: None,
        context: None,
        cache: None,
        slot_save_path: Some(slots.clone()),
    };
    let handle = llama
        .serve(&request, &CancellationToken::new())
        .await
        .unwrap();
    let args = control.args.lock().unwrap().clone();
    let at = args
        .iter()
        .position(|arg| arg == "--slot-save-path")
        .unwrap();
    assert_eq!(args[at + 1], slots.to_string_lossy());

    // A relative or missing directory, or one for another backend, is refused before launch.
    for (kind, directory) in [
        (BackendKind::LlamaCpp, PathBuf::from("kv-slots")),
        (BackendKind::LlamaCpp, root.path().join("missing")),
        (BackendKind::BitNet, slots.clone()),
    ] {
        let adapter = RuntimeAdapter::new(kind, PathBuf::from(RUNTIME), &http, &probe, &control);
        let error = adapter
            .serve(
                &ServeRequest {
                    slot_save_path: Some(directory.clone()),
                    ..request.clone()
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.0.contains("KV slot"), "{directory:?}: {}", error.0);
    }

    let replies = SlotHttp {
        requests: Mutex::new(Vec::new()),
    };
    let inference = OpenAiInference {
        http: &replies,
        probe: &probe,
        adapter: &llama,
        token: None,
    };
    let cancel = CancellationToken::new();
    let input = ChatInput {
        model: "test:latest".into(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
            tool_calls: Vec::new(),
            tool_name: None,
        }],
        tools: Vec::new(),
        temperature: None,
        slot: Some(2),
        max_tokens: None,
    };
    let result = inference.chat(&handle, &input, &cancel).await.unwrap();
    assert_eq!(result.content, "done");
    let timings = result.timings.unwrap();
    assert_eq!(
        timings,
        Timings {
            prompt_tokens: 12,
            prompt_ms: 30.0,
            generated_tokens: 50,
            generated_ms: 1000.0,
            cached_tokens: Some(40),
            draft_tokens: Some(20),
            draft_accepted: Some(15),
        }
    );
    assert_eq!(timings.generated_tokens_per_second(), Some(50.0));
    assert_eq!(timings.prompt_tokens_per_second(), Some(400.0));
    let sent = replies.requests.lock().unwrap()[0].2.clone();
    assert_eq!(sent["id_slot"], 2);
    assert_eq!(sent["cache_prompt"], true);
    assert!(sent.get("max_tokens").is_none() && sent.get("temperature").is_none());
    let capped = ChatInput {
        max_tokens: Some(64),
        temperature: Some(0.2),
        ..input.clone()
    };
    inference.chat(&handle, &capped, &cancel).await.unwrap();
    let sent = replies.requests.lock().unwrap().last().unwrap().2.clone();
    assert_eq!(sent["max_tokens"], 64);
    assert_eq!(sent["temperature"], 0.2);
    for bad in [
        ChatInput {
            max_tokens: Some(0),
            ..input.clone()
        },
        ChatInput {
            temperature: Some(f64::NAN),
            ..input.clone()
        },
    ] {
        assert!(inference.chat(&handle, &bad, &cancel).await.is_err());
    }
    for slot in [Some(16), None] {
        let unpinned = ChatInput {
            slot,
            ..input.clone()
        };
        let outcome = inference.chat(&handle, &unpinned, &cancel).await;
        match slot {
            Some(_) => assert!(outcome.is_err()),
            None => assert!(
                replies
                    .requests
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .2
                    .get("id_slot")
                    .is_none()
            ),
        }
    }

    let save = KvSlotAction::Save {
        name: "spark-a".into(),
    };
    assert_eq!(
        inference.kv_slot(&handle, 2, &save, &cancel).await.unwrap(),
        KvSlotReport {
            slot: 2,
            tokens: 1745,
            bytes: Some(14309796),
            ms: Some(49.865)
        }
    );
    let restore = KvSlotAction::Restore {
        name: "spark-a".into(),
    };
    let restored = inference
        .kv_slot(&handle, 3, &restore, &cancel)
        .await
        .unwrap();
    assert_eq!((restored.slot, restored.tokens), (3, 1745));
    let erased = inference
        .kv_slot(&handle, 2, &KvSlotAction::Erase, &cancel)
        .await
        .unwrap();
    assert_eq!((erased.tokens, erased.bytes, erased.ms), (1745, None, None));
    let recorded = replies.requests.lock().unwrap().clone();
    let slot_calls: Vec<_> = recorded
        .iter()
        .filter(|(path, _, _)| path.starts_with("/slots/"))
        .map(|(path, query, body)| (path.as_str(), query.as_deref(), body.clone()))
        .collect();
    assert_eq!(
        slot_calls,
        [
            (
                "/slots/2",
                Some("action=save"),
                json!({"filename": "spark-a.bin"})
            ),
            (
                "/slots/3",
                Some("action=restore"),
                json!({"filename": "spark-a.bin"})
            ),
            ("/slots/2", Some("action=erase"), json!({})),
        ]
    );
    // A reply for another slot, an unsafe name or an out-of-range slot is refused.
    assert!(
        inference
            .kv_slot(&handle, 4, &KvSlotAction::Erase, &cancel)
            .await
            .is_err()
    );
    // A negative duration is refused; zero is fine; a reply over 1 KiB still reads.
    let one = |name: &str| KvSlotAction::Save { name: name.into() };
    assert!(
        inference
            .kv_slot(&handle, 5, &one("a"), &cancel)
            .await
            .is_err()
    );
    assert_eq!(
        inference
            .kv_slot(&handle, 6, &one("a"), &cancel)
            .await
            .unwrap()
            .ms,
        Some(0.0)
    );
    assert_eq!(
        inference
            .kv_slot(&handle, 7, &one(&"a".repeat(64)), &cancel)
            .await
            .unwrap()
            .tokens,
        1
    );
    for name in ["", "../x", "a.bin", &"a".repeat(65)] {
        let action = KvSlotAction::Save { name: name.into() };
        assert!(
            inference
                .kv_slot(&handle, 2, &action, &cancel)
                .await
                .is_err(),
            "{name:?}"
        );
    }
    assert!(
        inference
            .kv_slot(&handle, 16, &KvSlotAction::Erase, &cancel)
            .await
            .is_err()
    );
    assert_eq!(
        replies
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _, _)| path.starts_with("/slots/"))
            .count(),
        7,
        "refused names and slots never reach the server"
    );
}

#[test]
fn kv_slot_files_live_privately_beside_the_runtime_state() {
    let root = tempfile::tempdir().unwrap();
    let config = rigspark_runtime::state::Config {
        home: root.path().to_path_buf(),
        state: root.path().join("slot").join("state.json"),
        lock: root.path().join("slot").join("lock"),
        staging: root.path().join("slot").join(".staging"),
    };
    let directory = rigspark_runtime::application::kv_slot_directory(&config).unwrap();
    assert_eq!(directory, root.path().join("slot").join("kv-slots"));
    assert!(directory.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&directory).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    // Idempotent, and a relative state path is refused.
    assert_eq!(
        rigspark_runtime::application::kv_slot_directory(&config).unwrap(),
        directory
    );
    let relative = rigspark_runtime::state::Config {
        state: "state.json".into(),
        ..config
    };
    assert!(rigspark_runtime::application::kv_slot_directory(&relative).is_err());
}

#[tokio::test]
async fn slot_actions_and_pinned_chats_refuse_a_listener_that_is_not_the_server_started() {
    use rigspark_runtime::{
        ollama_inference::{ChatInput, ChatMessage},
        openai::{KvSlotAction, OpenAiInference},
        state::ServerState,
    };
    let probe = Probe {
        binary: "/usr/bin/impostor".into(),
    };
    let control = Control {
        signals: Mutex::new(Vec::new()),
    };
    let http = SlotHttp {
        requests: Mutex::new(Vec::new()),
    };
    let llama = RuntimeAdapter::new(
        BackendKind::LlamaCpp,
        PathBuf::from(RUNTIME),
        &http,
        &probe,
        &control,
    );
    let handle = ServerState {
        backend: "llamacpp".into(),
        model_id: "test:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        port: 11435,
        owned_by_us: true,
        pid: Some(123),
        runtime_model_id: None,
        context: None,
        integrity: None,
        local_manifest_digest: None,
        model_path: None,
        process_executable: Some(RUNTIME.into()),
        process_started_at: Some("instance".into()),
        auth_token: None,
        cache: None,
    };
    let inference = OpenAiInference {
        http: &http,
        probe: &probe,
        adapter: &llama,
        token: None,
    };
    let cancel = CancellationToken::new();
    assert!(
        inference
            .kv_slot(&handle, 2, &KvSlotAction::Erase, &cancel)
            .await
            .is_err()
    );
    let input = ChatInput {
        model: "test:latest".into(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
            tool_calls: Vec::new(),
            tool_name: None,
        }],
        tools: Vec::new(),
        temperature: None,
        slot: Some(2),
        max_tokens: None,
    };
    assert!(inference.chat(&handle, &input, &cancel).await.is_err());
    assert!(
        http.requests.lock().unwrap().is_empty(),
        "nothing reached the impostor"
    );
}

#[tokio::test]
async fn only_llama_cpp_servers_get_kv_slots() {
    use rigspark_runtime::{
        application::{kv_slot_directory_for, kv_slot_with_config},
        openai::KvSlotAction,
        state::{Config, RuntimeState, ServerState},
    };
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        home: root.path().to_path_buf(),
        state: root.path().join("state.json"),
        lock: root.path().join("lock"),
        staging: root.path().join(".staging"),
    };
    assert_eq!(
        kv_slot_directory_for("llamacpp", &config).unwrap(),
        Some(root.path().join("kv-slots"))
    );
    for backend in ["ollama", "mlx", "lmstudio", "bitnet"] {
        assert_eq!(
            kv_slot_directory_for(backend, &config).unwrap(),
            None,
            "{backend}"
        );
    }
    // An Ollama server on record is refused before any llama.cpp binary is looked up.
    let state = RuntimeState {
        schema_version: 2,
        active: Some(ServerState {
            backend: "ollama".into(),
            model_id: "qwen3:8b".into(),
            endpoint: "http://127.0.0.1:11434".into(),
            port: 11434,
            owned_by_us: false,
            pid: None,
            runtime_model_id: None,
            context: None,
            integrity: None,
            local_manifest_digest: None,
            model_path: None,
            process_executable: None,
            process_started_at: None,
            auth_token: None,
            cache: None,
        }),
    };
    std::fs::write(&config.state, serde_json::to_vec(&state).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config.state, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let error = kv_slot_with_config(config, 0, &KvSlotAction::Erase, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.0, "KV slots apply only to llama.cpp");
}
