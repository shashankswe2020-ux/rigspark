#[cfg(unix)]
use rigspark_runtime::opencode::{
    LaunchSpec, NativeOpenCodeRunner, OpenCodeHarness, OpenCodeRunner,
};
use rigspark_runtime::{
    adapters::{BackendAdapter, BackendError, ServeRequest},
    harness::{
        ChatHarness, HarnessError, HarnessMessage, HarnessRequest, LocalHarness, Provider,
        RemoteHarness, RemoteRequest, RemoteResponse, RemoteTransport, Secret,
    },
    identity::{Listener, ProcessIdentity, ProcessProbe},
    lifecycle::Registry,
    ollama_inference::{ChatInput, ChatResult},
    opencode::{launch_spec, parse_event},
    state::{Config, RuntimeState, ServerState, StateError, StateStore},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tokio_util::sync::CancellationToken;

fn request(model: &str) -> HarnessRequest {
    HarnessRequest {
        model: model.into(),
        messages: vec![
            HarnessMessage {
                role: "system".into(),
                content: "be brief".into(),
            },
            HarnessMessage {
                role: "user".into(),
                content: "hello".into(),
            },
        ],
        temperature: None,
    }
}

struct Recorded {
    status: u16,
    body: String,
    sent: Mutex<Vec<(BTreeMap<String, String>, Value)>>,
}
impl Recorded {
    fn new(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.into(),
            sent: Mutex::default(),
        }
    }
}
#[async_trait::async_trait]
impl RemoteTransport for Recorded {
    async fn send(&self, request: RemoteRequest) -> Result<RemoteResponse, HarnessError> {
        self.sent
            .lock()
            .unwrap()
            .push((request.headers, request.body));
        Ok(RemoteResponse {
            status: self.status,
            body: Box::pin(std::io::Cursor::new(self.body.clone().into_bytes())),
        })
    }
}

async fn chat(harness: &RemoteHarness<'_>) -> Result<(String, Vec<String>), HarnessError> {
    let mut chunks = Vec::new();
    let reply = harness
        .chat(&request(""), &CancellationToken::new(), &mut |text| {
            chunks.push(text.to_owned());
            Ok(())
        })
        .await?;
    Ok((reply, chunks))
}

const OPENAI_DELTA: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\\u001b[31m there\"}}]}\n\ndata: [DONE]\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"after\"}}]}\n\n";

#[tokio::test]
async fn compatible_endpoints_send_a_bearer_token_only_when_configured() {
    for key in [None, Some("compat-key")] {
        let transport = Recorded::new(200, OPENAI_DELTA);
        let harness = RemoteHarness::new(
            Provider::Compatible,
            "http://127.0.0.1:3000/v1/chat/completions",
            key.map(|key| Secret::new(key).unwrap()),
            &transport,
        )
        .unwrap();
        chat(&harness).await.unwrap();
        let sent = transport.sent.lock().unwrap();
        let (headers, body) = &sent[0];
        assert_eq!(
            headers.get("authorization").cloned(),
            key.map(|key| format!("Bearer {key}"))
        );
        assert_eq!(body["model"], "local-model");
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn streamed_deltas_are_sanitized_and_everything_after_done_is_ignored() {
    let transport = Recorded::new(200, OPENAI_DELTA);
    let harness = RemoteHarness::new(
        Provider::OpenAi,
        "https://api.openai.com/v1/chat/completions",
        Some(Secret::new("openai-key").unwrap()),
        &transport,
    )
    .unwrap();
    let (reply, chunks) = chat(&harness).await.unwrap();
    assert_eq!(reply, "hi there");
    assert_eq!(chunks.concat(), reply);
}

#[tokio::test]
async fn claude_uses_api_key_headers_drops_system_turns_and_parses_text_deltas() {
    let body = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"bonjour\"}}\n\nevent: ping\ndata: not-json\n\n";
    let transport = Recorded::new(200, body);
    let harness = RemoteHarness::new(
        Provider::Claude,
        "https://api.anthropic.com/v1/messages",
        Some(Secret::new("anthropic-key").unwrap()),
        &transport,
    )
    .unwrap();
    assert_eq!(chat(&harness).await.unwrap().0, "bonjour");
    let sent = transport.sent.lock().unwrap();
    let (headers, body) = &sent[0];
    assert_eq!(
        headers.get("x-api-key").map(String::as_str),
        Some("anthropic-key")
    );
    assert!(headers.contains_key("anthropic-version") && !headers.contains_key("authorization"));
    assert_eq!(body["messages"], json!([{"role":"user","content":"hello"}]));
    assert_eq!(body["max_tokens"], 1024);
}

#[tokio::test]
async fn malformed_payloads_and_error_statuses_fail_closed() {
    for payload in [
        "data: not-json\n\n",
        "data: {\"choices\":\"wrong\"}\n\n",
        "data: {\"no\":\"choices\"}\n\n",
    ] {
        let transport = Recorded::new(200, payload);
        let harness = RemoteHarness::new(
            Provider::Compatible,
            "http://127.0.0.1:3000/v1",
            None,
            &transport,
        )
        .unwrap();
        assert!(
            matches!(chat(&harness).await, Err(HarnessError::Response)),
            "{payload}"
        );
    }
    let transport = Recorded::new(401, "{\"error\":\"secret detail\"}");
    let harness = RemoteHarness::new(
        Provider::Compatible,
        "http://127.0.0.1:3000/v1",
        None,
        &transport,
    )
    .unwrap();
    assert!(matches!(
        chat(&harness).await,
        Err(HarnessError::Status(401))
    ));
    for (provider, url) in [
        (
            Provider::OpenAi,
            "https://api.openai.com/v1/chat/completions",
        ),
        (Provider::Claude, "https://api.anthropic.com/v1/messages"),
    ] {
        assert!(RemoteHarness::new(provider, url, None, &transport).is_err());
    }
}

struct Local {
    streaming: bool,
    trusted: bool,
    models: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl ProcessProbe for Local {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Ok(Listener {
            identity: ProcessIdentity {
                pid: 48827,
                process: "LM Studio".into(),
                executable: "/Applications/LM Studio.app/Contents/MacOS/LM Studio".into(),
                started: "2026-08-26 20:02:41".into(),
            },
            address: host.into(),
            port,
        })
    }
    async fn process(&self, _: u32) -> Result<ProcessIdentity, StateError> {
        Ok(self.listener(1234, "127.0.0.1").await?.identity)
    }
}
#[async_trait::async_trait]
impl BackendAdapter for Local {
    fn name(&self) -> &'static str {
        "lmstudio"
    }
    fn trusts(&self, _: &ProcessIdentity) -> bool {
        self.trusted
    }
    async fn serve(
        &self,
        _: &ServeRequest,
        _: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        unreachable!("the harness never starts runtimes")
    }
    async fn ready(&self, _: &ServeRequest, _: &CancellationToken) -> Result<(), BackendError> {
        unreachable!("the harness never starts runtimes")
    }
    async fn stop(&self, _: &ServerState, _: &CancellationToken) -> Result<(), BackendError> {
        unreachable!("the harness never stops runtimes")
    }
    async fn chat(
        &self,
        handle: &ServerState,
        input: &ChatInput,
        _: &CancellationToken,
    ) -> Result<ChatResult, BackendError> {
        assert_eq!(handle.pid, Some(48827));
        self.models.lock().unwrap().push(input.model.clone());
        Ok(ChatResult {
            content: "whole reply".into(),
            tool_calls: vec![],
            timings: None,
        })
    }
    fn can_stream(&self) -> bool {
        self.streaming
    }
}

fn store(home: &std::path::Path, active: Option<Value>) -> StateStore {
    let store = StateStore::new(Config::from_home(home).unwrap());
    let state: RuntimeState =
        serde_json::from_value(json!({"schemaVersion":2,"active":active})).unwrap();
    let guard = store.lock(Duration::from_secs(1)).unwrap();
    store.write(&guard, &state).unwrap();
    guard.release().unwrap();
    store
}

fn attached() -> Value {
    json!({"backend":"lmstudio","modelId":"qwen2.5:0.5b","runtimeModelId":"qwen2.5-0.5b-instruct","endpoint":"http://127.0.0.1:1234","port":1234,"ownedByUs":false,"modelPath":"Qwen/Qwen2.5-0.5B-Instruct-GGUF/q4.gguf","pid":48827,"processExecutable":"/Applications/LM Studio.app/Contents/MacOS/LM Studio","processStartedAt":"2026-08-26 20:02:41"})
}

#[tokio::test]
async fn local_harness_is_unavailable_without_an_active_server() {
    let home = tempfile::tempdir().unwrap();
    let state = store(home.path(), None);
    let adapter = Local {
        streaming: false,
        trusted: true,
        models: Mutex::default(),
    };
    let registry = Registry::new(vec![&adapter]);
    let harness = LocalHarness {
        state: &state,
        registry: &registry,
        probe: &adapter,
    };
    assert!(!harness.available().await);
    let result = ChatHarness::chat(
        &harness,
        &request("x"),
        &CancellationToken::new(),
        &mut |_| Ok(()),
    )
    .await;
    assert!(matches!(result, Err(HarnessError::Unavailable)));
    assert!(adapter.models.lock().unwrap().is_empty());
}

#[tokio::test]
async fn attached_local_runtimes_use_live_identity_and_the_runtime_model_by_default() {
    let home = tempfile::tempdir().unwrap();
    let state = store(home.path(), Some(attached()));
    let adapter = Local {
        streaming: false,
        trusted: true,
        models: Mutex::default(),
    };
    let registry = Registry::new(vec![&adapter]);
    let harness = LocalHarness {
        state: &state,
        registry: &registry,
        probe: &adapter,
    };
    assert!(harness.available().await);
    let mut chunks = Vec::new();
    let reply = ChatHarness::chat(
        &harness,
        &request(""),
        &CancellationToken::new(),
        &mut |text| {
            chunks.push(text.to_owned());
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (reply.as_str(), chunks),
        ("whole reply", vec!["whole reply".to_owned()])
    );
    ChatHarness::chat(
        &harness,
        &request("explicit"),
        &CancellationToken::new(),
        &mut |_| Ok(()),
    )
    .await
    .unwrap();
    assert_eq!(
        *adapter.models.lock().unwrap(),
        ["qwen2.5-0.5b-instruct", "explicit"]
    );
}

#[tokio::test]
async fn untrusted_local_listeners_are_refused_before_inference() {
    let home = tempfile::tempdir().unwrap();
    let state = store(home.path(), Some(attached()));
    let adapter = Local {
        streaming: true,
        trusted: false,
        models: Mutex::default(),
    };
    let registry = Registry::new(vec![&adapter]);
    let harness = LocalHarness {
        state: &state,
        registry: &registry,
        probe: &adapter,
    };
    let result = ChatHarness::chat(
        &harness,
        &request("x"),
        &CancellationToken::new(),
        &mut |_| Ok(()),
    )
    .await;
    assert!(matches!(result, Err(HarnessError::Drift)));
    assert!(adapter.models.lock().unwrap().is_empty());
}

#[test]
fn opencode_unrestricted_mode_and_explicit_providers_are_honoured() {
    let restricted = launch_spec(&request("llama3.2"), false).unwrap();
    let config: Value = serde_json::from_str(&restricted.config).unwrap();
    assert_eq!(
        config["provider"]["ollama"]["options"]["baseURL"],
        "http://127.0.0.1:11434/v1"
    );
    let open = launch_spec(&request("anthropic/claude-sonnet"), true).unwrap();
    let config: Value = serde_json::from_str(&open.config).unwrap();
    assert_eq!(
        (config["permission"].as_str(), config["share"].as_str()),
        (Some("allow"), Some("auto"))
    );
    assert_eq!(config["agent"]["rigspark-chat"]["permission"], "allow");
    assert!(config.get("provider").is_none());
    assert_eq!(open.args[3], "anthropic/claude-sonnet");
    for invalid in ["", "   ", "/model", "provider/"] {
        assert!(
            launch_spec(&request(invalid), false).is_err(),
            "{invalid:?}"
        );
    }
}

#[test]
fn opencode_reasoning_and_tool_activity_render_as_bounded_inline_markdown() {
    let reasoning = parse_event(&json!({"type":"reasoning","timestamp":1,"sessionID":"s","part":{"type":"reasoning","text":"thinking\u{1b}[2J hard"}}).to_string()).unwrap();
    assert!(
        reasoning.starts_with("\n\n> ")
            && reasoning.contains("thinking")
            && !reasoning.contains('\u{1b}')
    );
    let tool = parse_event(&json!({"type":"tool_use","timestamp":1,"sessionID":"s","part":{"type":"tool","tool":"read","state":{"input":{"filePath":"src/main.rs"},"output":"x".repeat(1000)}}}).to_string()).unwrap();
    assert!(
        tool.contains("`read`") && tool.contains("src/main.rs") && tool.contains("```"),
        "{tool}"
    );
    assert!(tool.matches('x').count() <= 400);
    for silent in ["step_start", "step_finish"] {
        assert_eq!(
            parse_event(
                &json!({"type":silent,"timestamp":1,"sessionID":"s","part":{"type":"step"}})
                    .to_string()
            )
            .unwrap(),
            ""
        );
    }
    for invalid in [
        json!({"type":"text","sessionID":"s","part":{"type":"text","text":"no timestamp"}}),
        json!({"type":"text","timestamp":1,"part":{"type":"text","text":"no session"}}),
        json!({"type":"unknown","timestamp":1,"sessionID":"s","part":{"type":"x"}}),
    ] {
        assert!(parse_event(&invalid.to_string()).is_err(), "{invalid}");
    }
}

#[cfg(unix)]
fn script(directory: &std::path::Path, body: &str) -> NativeOpenCodeRunner {
    use std::os::unix::fs::PermissionsExt;
    let binary = directory.join("opencode");
    std::fs::write(&binary, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    NativeOpenCodeRunner {
        binary,
        env: BTreeMap::new(),
        commit_home: None,
    }
}

#[cfg(unix)]
async fn run(
    runner: &NativeOpenCodeRunner,
    cancel: &CancellationToken,
) -> (Result<(), HarnessError>, String) {
    let spec: LaunchSpec = launch_spec(&request("llama3.2"), false).unwrap();
    let mut text = String::new();
    let result = runner
        .run(&spec, cancel, &mut |chunk| {
            text.push_str(chunk);
            Ok(())
        })
        .await;
    (result, text)
}

#[cfg(unix)]
#[tokio::test]
async fn native_opencode_runner_streams_rejects_failures_and_bounds_output() {
    let directory = tempfile::tempdir().unwrap();
    let missing = NativeOpenCodeRunner {
        binary: directory.path().join("absent"),
        env: BTreeMap::new(),
        commit_home: None,
    };
    assert!(
        !OpenCodeHarness {
            runner: &missing,
            unrestricted: false
        }
        .available()
        .await
    );
    let text =
        r#"{"type":"text","timestamp":1,"sessionID":"s","part":{"type":"text","text":"reply"}}"#;
    let ok = script(directory.path(), &format!("printf '%s\\n' '{text}'"));
    assert!(
        OpenCodeHarness {
            runner: &ok,
            unrestricted: false
        }
        .available()
        .await
    );
    let (result, output) = run(&ok, &CancellationToken::new()).await;
    assert!(result.is_ok());
    assert_eq!(output, "reply");
    let failing = script(
        directory.path(),
        &format!("printf '%s\\n' '{text}'; printf 'secret-detail' >&2; exit 3"),
    );
    assert!(matches!(
        run(&failing, &CancellationToken::new()).await.0,
        Err(HarnessError::Transport)
    ));
    let malformed = script(directory.path(), "printf 'not json\\n'");
    assert!(matches!(
        run(&malformed, &CancellationToken::new()).await.0,
        Err(HarnessError::Response)
    ));
    let flood = script(
        directory.path(),
        r#"exec /usr/bin/yes '{"type":"step_start","timestamp":1,"sessionID":"s","part":{"type":"step-start"}}'"#,
    );
    assert!(matches!(
        run(&flood, &CancellationToken::new()).await.0,
        Err(HarnessError::Limit)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_a_native_opencode_run_kills_the_child() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let runner = script(
        directory.path(),
        &format!("echo $$ > '{}'\nexec /bin/sleep 30", pid_file.display()),
    );
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let watcher = pid_file.clone();
    tokio::spawn(async move {
        while !watcher.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    assert!(matches!(
        run(&runner, &cancel).await.0,
        Err(HarnessError::Cancelled)
    ));
    assert!(started.elapsed() < Duration::from_secs(10));
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    let mut alive = true;
    for _ in 0..50 {
        alive = std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap()
            .success();
        if !alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!alive, "opencode child {pid} survived cancellation");
}
