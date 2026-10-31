use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rigspark_gui::{Host, engine::Engine, router};
use rigspark_runtime::harness::{DeltaSink, HarnessError, HarnessRequest};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Default)]
struct Capture {
    requests: Mutex<Vec<HarnessRequest>>,
    started: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl Engine for Capture {
    async fn chat(
        &self,
        _: &str,
        request: &HarnessRequest,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        self.requests.lock().unwrap().push(request.clone());
        let last = request.messages.last().unwrap().content.clone();
        match last.as_str() {
            "wait" => {
                self.started.notify_one();
                cancel.cancelled().await;
                Err(HarnessError::Cancelled)
            }
            "fail" => Err(HarnessError::Unavailable),
            "split" => {
                for part in ["a\r", "\nb\u{1b}[3", "1mc\u{1b}]8;;https://x", "\u{7}d"] {
                    sink(part)?;
                }
                Ok("a\r\nb\u{1b}[31mc\u{1b}]8;;https://x\u{7}d".into())
            }
            _ => {
                let reply = format!("echo: {last}\r\nnext\u{1b}[31m");
                sink(&reply)?;
                Ok(reply)
            }
        }
    }
}

fn request(host: &Host, method: &str, path: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("127.0.0.1:{}", host.port))
        .header("origin", host.origin())
        .header("x-llmup-token", &host.token)
        .header("content-type", "application/json")
        .body(body)
        .unwrap()
}

async fn call(host: &Arc<Host>, method: &str, path: &str, payload: Value) -> (StatusCode, String) {
    let body = if method == "GET" {
        Body::empty()
    } else {
        Body::from(payload.to_string())
    };
    let response = router(host.clone())
        .oneshot(request(host, method, path, body))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn json_call(
    host: &Arc<Host>,
    method: &str,
    path: &str,
    payload: Value,
) -> (StatusCode, Value) {
    let (status, text) = call(host, method, path, payload).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn host(engine: Arc<Capture>) -> (tempfile::TempDir, Arc<Host>) {
    let home = tempfile::tempdir().unwrap();
    let host = Host::with_engine(home.path(), 43210, engine).unwrap();
    (home, host)
}

#[tokio::test]
async fn workspace_shell_names_its_primary_regions() {
    let (_home, host) = host(Arc::default());
    let (status, html) = call(&host, "GET", "/", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    for text in ["Workspace", "Current session", ">Model<"] {
        assert!(html.contains(text), "{text}");
    }
}

#[tokio::test]
async fn catalog_status_reports_offline_provenance_and_rejects_update_parameters() {
    let (home, host) = host(Arc::default());
    let (status, value) = json_call(&host, "GET", "/api/catalog/status", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["catalog"]["source"], "bundled");
    assert!(value["catalog"]["generatedAt"].is_string());
    assert!(value["catalog"]["modelCount"].as_u64().unwrap() > 0);
    assert!(!home.path().join("catalog").exists());
    let (status, _) = json_call(
        &host,
        "POST",
        "/api/catalog/update",
        json!({"url":"https://untrusted.invalid"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!home.path().join("catalog").exists());
    let response = router(host.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/catalog/update")
                .header("host", format!("127.0.0.1:{}", host.port))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn chat_forwards_prompt_options_and_rebuilds_turns_from_the_session() {
    let engine = Arc::new(Capture::default());
    let (_home, host) = host(engine.clone());
    let (status, first) = call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"hi"}],"systemPrompt":"You are a terse pirate.","temperature":0.3}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(first.contains("\"type\":\"done\""), "{first}");
    call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"again"}]}),
    )
    .await;
    let requests = engine.requests.lock().unwrap().clone();
    assert_eq!(requests[0].temperature, Some(0.3));
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|message| message.role == "system"
                && message.content.contains("You are a terse pirate."))
    );
    let turns = |request: &HarnessRequest| {
        request
            .messages
            .iter()
            .filter(|message| message.role != "system")
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(turns(&requests[0]), [("user".into(), "hi".into())]);
    assert_eq!(
        turns(&requests[1]),
        [
            ("user".into(), "hi".into()),
            ("assistant".into(), "echo: hi\nnext".into()),
            ("user".into(), "again".into()),
        ]
    );
    assert!(
        !requests[1]
            .messages
            .iter()
            .any(|message| message.content.contains("terse pirate"))
    );
}

#[tokio::test]
async fn multiline_input_and_reply_are_normalized_in_stream_and_history() {
    let engine = Arc::new(Capture::default());
    let (_home, host) = host(engine.clone());
    let (_, stream) = call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"line one\r\nline two\t\u{1b}[31m"}]}),
    )
    .await;
    assert_eq!(
        engine.requests.lock().unwrap()[0]
            .messages
            .last()
            .unwrap()
            .content,
        "line one\nline two\t"
    );
    assert!(
        stream.contains(r#""content":"echo: line one\nline two\t\nnext""#),
        "{stream}"
    );
    let (_, history) = json_call(&host, "GET", "/api/history", Value::Null).await;
    assert_eq!(
        history["history"],
        json!([
            {"role":"user","content":"line one\nline two\t"},
            {"role":"assistant","content":"echo: line one\nline two\t\nnext"}
        ])
    );
}

#[tokio::test]
async fn sequences_split_across_deltas_never_reach_the_browser() {
    let (_home, host) = host(Arc::default());
    let (_, stream) = call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"split"}]}),
    )
    .await;
    let deltas: String = stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .filter(|event| event["type"] == "delta")
        .map(|event| event["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(deltas, "a\nbcd");
    let (_, history) = json_call(&host, "GET", "/api/history", Value::Null).await;
    assert_eq!(history["history"][1]["content"], "a\nbcd");
}

#[tokio::test]
async fn backend_failure_streams_an_actionable_error_and_saves_nothing() {
    let (_home, host) = host(Arc::default());
    let (status, stream) = call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"fail"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        stream.contains(
            r#""message":"No model is ready. Start one from Models. No completed exchange was saved.""#
        ),
        "{stream}"
    );
    assert!(!stream.contains("\"type\":\"done\"") && !stream.contains("unavailable"));
    let (_, history) = json_call(&host, "GET", "/api/history", Value::Null).await;
    assert_eq!(history["history"], json!([]));
}

#[tokio::test]
async fn cancel_stops_the_active_run_without_a_terminal_event_or_history() {
    let engine = Arc::new(Capture::default());
    let (_home, host) = host(engine.clone());
    let (_, idle) = json_call(&host, "POST", "/api/chat/cancel", json!({})).await;
    assert_eq!(idle, json!({"cancelled":false}));
    let response = router(host.clone())
        .oneshot(request(
            &host,
            "POST",
            "/api/chat",
            Body::from(json!({"messages":[{"role":"user","content":"wait"}]}).to_string()),
        ))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), engine.started.notified())
        .await
        .expect("engine did not start");
    let (_, cancelled) = json_call(&host, "POST", "/api/chat/cancel", json!({})).await;
    assert_eq!(cancelled, json!({"cancelled":true}));
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        to_bytes(response.into_body(), 65536),
    )
    .await
    .expect("cancelled stream did not end")
    .unwrap();
    let stream = String::from_utf8(stream.to_vec()).unwrap();
    assert!(
        !stream.contains("\"type\":\"done\"") && !stream.contains("\"type\":\"error\""),
        "{stream}"
    );
    let (_, history) = json_call(&host, "GET", "/api/history", Value::Null).await;
    assert_eq!(history["history"], json!([]));
    let (_, again) = json_call(&host, "POST", "/api/chat/cancel", json!({})).await;
    assert_eq!(again, json!({"cancelled":false}));
}

#[tokio::test]
async fn completed_runs_leave_nothing_to_cancel() {
    let (_home, host) = host(Arc::default());
    call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"hi"}]}),
    )
    .await;
    let (_, idle) = json_call(&host, "POST", "/api/chat/cancel", json!({})).await;
    assert_eq!(idle, json!({"cancelled":false}));
}

#[tokio::test]
async fn oversized_chat_bodies_are_rejected() {
    let (_home, host) = host(Arc::default());
    let (status, _) = call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"x".repeat(70 * 1024)}]}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn model_routes_validate_queries_before_touching_runtimes() {
    let (_home, host) = host(Arc::default());
    let (_, runtimes) = json_call(&host, "GET", "/api/runtimes", Value::Null).await;
    assert_eq!(
        runtimes,
        json!({"runtimes":["ollama","llamacpp","mlx","lmstudio","bitnet"]})
    );
    let (_, active) = json_call(&host, "GET", "/api/models/active", Value::Null).await;
    assert_eq!(active, json!({"active":null}));
    for query in [
        "/api/models/recommended?runtime=bogus",
        "/api/models/recommended?context=extreme",
        "/api/models/recommended?context=high&kvCache=q2_k",
        "/api/models/recommended?kvCache=q8_0",
        "/api/models/installed?port=notaport",
        "/api/models/installed?tokens=-1",
    ] {
        assert_eq!(
            call(&host, "GET", query, Value::Null).await.0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    let (status, scoped) = json_call(
        &host,
        "GET",
        "/api/models/recommended?runtime=llamacpp&context=high",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (scoped["runtime"].as_str(), scoped["contextPreset"].as_str()),
        (Some("llamacpp"), Some("high"))
    );
    let models = scoped["models"].as_array().unwrap();
    assert!(!models.is_empty());
    for field in ["id", "verdict", "throughput"] {
        assert!(
            models.iter().all(|model| model.get(field).is_some()),
            "{field}"
        );
    }
    let kv_bytes = |body: &Value| -> std::collections::BTreeMap<String, f64> {
        body["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| {
                Some((
                    model["id"].as_str()?.to_owned(),
                    model["contextSizing"]["kvCacheBytes"].as_f64()?,
                ))
            })
            .collect()
    };
    let (status, quantized) = json_call(
        &host,
        "GET",
        "/api/models/recommended?runtime=llamacpp&context=high&kvCache=q8_0",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(quantized["kvCache"], "q8_0");
    assert!(
        quantized["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|model| model["kvPrecision"] == "q8_0")
    );
    assert!(scoped["kvCache"].is_null());
    // A small fixed context keeps both top-8 lists overlapping on any detected host (CI runners are small).
    let (status, fp16_small) = json_call(
        &host,
        "GET",
        "/api/models/recommended?runtime=llamacpp&tokens=2048",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, q8_small) = json_call(
        &host,
        "GET",
        "/api/models/recommended?runtime=llamacpp&tokens=2048&kvCache=q8_0",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (fp16, q8) = (kv_bytes(&fp16_small), kv_bytes(&q8_small));
    let shared: Vec<_> = q8.keys().filter(|id| fp16.contains_key(*id)).collect();
    assert!(!shared.is_empty());
    for id in shared {
        assert!(q8[id] < fp16[id], "{id}: {} !< {}", q8[id], fp16[id]);
    }
    assert_eq!(
        call(
            &host,
            "POST",
            "/api/models/up",
            json!({"model":"qwen3:0.6b","launch":true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // Only refusals that happen before any runtime is probed or signalled are exercised here.
    for (method, path) in [
        ("POST", "/api/runtimes/mlx/start"),
        ("POST", "/api/runtimes/llamacpp/stop"),
        ("GET", "/api/runtimes/ollama/start"),
        ("POST", "/api/runtimes/status"),
    ] {
        assert_eq!(
            call(&host, method, path, json!({})).await.0,
            StatusCode::BAD_REQUEST,
            "{method} {path}"
        );
    }
    let (status, hardware) = json_call(&host, "GET", "/api/hardware", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(hardware["hardware"].is_object(), "{hardware}");
}

#[tokio::test]
async fn up_lifecycle_failures_report_their_reason_instead_of_a_generic_error() {
    let (home, host) = host(Arc::default());
    let (status, text) = call(
        &host,
        "POST",
        "/api/models/up",
        json!({"model":"zz-not-a-catalog-model"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error = serde_json::from_str::<Value>(&text).unwrap()["error"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(error, "invalid request");
    assert!(error.contains("zz-not-a-catalog-model"), "{error}");
    assert!(!error.chars().any(char::is_control), "{error}");
    let config = rigspark_runtime::state::Config::from_home(home.path()).unwrap();
    assert!(
        rigspark_runtime::state::StateStore::new(config)
            .read()
            .unwrap()
            .active
            .is_none()
    );
}

#[tokio::test]
async fn up_requests_are_validated_before_state_or_runtime_access() {
    let (home, host) = host(Arc::default());
    for body in [
        json!({"model":""}),
        json!({"model":"   "}),
        json!({"model":"x","port":70000}),
        json!({"model":"x","port":0}),
        json!({"model":"x","context":0}),
        json!({"model":"x","backend":"vllm"}),
        json!({"model":"x","installed":true}),
        json!({"model":"x","installed":true,"bypass":true,"backend":"llamacpp"}),
        json!({"model":"x","extra":1}),
        json!({"model":"x","kvCache":"q2_k"}),
        json!({"model":"x","flashAttention":"yes"}),
        json!({"model":"x","installed":true,"bypass":true,"kvCache":"q8_0"}),
        json!({"model":"x","speculative":"eagle"}),
        json!({"model":"x","parallel":0}),
        json!({"model":"x","parallel":17}),
        json!({"model":"x","parallel":-1}),
        json!({"model":"x","installed":true,"bypass":true,"parallel":2}),
    ] {
        assert_eq!(
            call(&host, "POST", "/api/models/up", body.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{body}"
        );
    }
    let (status, text) = call(
        &host,
        "POST",
        "/api/models/up",
        json!({"model":"qwen3:0.6b","kvCache":"q8_0","flashAttention":"off"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(text.contains("requires flash attention"), "{text}");
    let config = rigspark_runtime::state::Config::from_home(home.path()).unwrap();
    assert!(
        rigspark_runtime::state::StateStore::new(config)
            .read()
            .unwrap()
            .active
            .is_none()
    );
}

#[tokio::test]
async fn active_summary_reports_ownership_runtime_variant_and_context() {
    use rigspark_runtime::state::{Config, RuntimeState, ServerState, StateStore};
    let (home, host) = host(Arc::default());
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    for (owned, variant) in [(false, None), (true, Some(65_536))] {
        let guard = store.lock(std::time::Duration::from_secs(5)).unwrap();
        let active = ServerState {
            backend: "ollama".into(),
            model_id: "gemma4:e4b-it-qat".into(),
            endpoint: "http://127.0.0.1:11434".into(),
            port: 11434,
            owned_by_us: owned,
            // Placeholder identity only: the active route reads state and never signals a process.
            pid: owned.then_some(999_999),
            runtime_model_id: variant.map(|tokens| format!("llmup-context-test:{tokens}")),
            context: variant,
            integrity: None,
            local_manifest_digest: None,
            model_path: None,
            process_executable: None,
            process_started_at: None,
            auth_token: None,
            cache: owned.then(|| rigspark_runtime::cache::CacheProfile {
                kv_k: rigspark_core::sizing::KvCacheType::Q8_0,
                kv_v: rigspark_core::sizing::KvCacheType::Q8_0,
                ..Default::default()
            }),
        };
        store
            .write(&guard, &RuntimeState::for_active(active))
            .unwrap();
        drop(guard);
        let (_, body) = json_call(&host, "GET", "/api/models/active", Value::Null).await;
        let summary = &body["active"];
        assert_eq!(
            (
                summary["modelId"].as_str(),
                summary["backend"].as_str(),
                summary["port"].as_u64()
            ),
            (Some("gemma4:e4b-it-qat"), Some("ollama"), Some(11434))
        );
        assert_eq!(
            summary["ownership"],
            if owned { "owned" } else { "attached" }
        );
        if owned {
            assert_eq!(summary["cache"]["kvK"], "q8_0");
        } else {
            assert!(summary.get("cache").is_none());
        }
        match variant {
            None => {
                assert!(summary.get("runtimeModelId").is_none() && summary.get("context").is_none())
            }
            Some(tokens) => {
                assert_eq!(summary["context"], tokens);
                assert_eq!(summary["runtimeModelId"], "llmup-context-test:65536");
            }
        }
    }
}

#[tokio::test]
async fn attachments_inject_selected_lines_and_record_a_manifest() {
    let engine = Arc::new(Capture::default());
    let (_home, host) = host(engine.clone());
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("notes.txt"),
        "alpha line\nbeta line\ngamma line\n",
    )
    .unwrap();
    let (_, registered) = json_call(
        &host,
        "POST",
        "/api/workspace/root",
        json!({"path":root.path()}),
    )
    .await;
    let workspace = registered["root"]["id"].clone();
    let (_, whole) = call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"review"}],"attachments":[{"workspaceId":workspace,"path":"notes.txt"}]}),
    )
    .await;
    assert!(whole.contains("\"type\":\"context\""), "{whole}");
    call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"line two"}],"attachments":[{"workspaceId":workspace,"path":"notes.txt","range":{"startLine":2,"endLine":2}}]}),
    )
    .await;
    let requests = engine.requests.lock().unwrap().clone();
    let context = |request: &HarnessRequest| {
        request
            .messages
            .iter()
            .filter(|message| message.content.contains("FILE: notes.txt"))
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let first = context(&requests[0]);
    assert!(
        first.contains("alpha line") && first.contains("gamma line"),
        "{first}"
    );
    let ranged = context(&requests[1]);
    assert!(ranged.contains("beta line"), "{ranged}");
    assert!(
        !ranged.contains("alpha line") && !ranged.contains("gamma line"),
        "{ranged}"
    );
    let id = host.ui.lock().await.session.clone().unwrap();
    let doc = host.sessions.get(&id).unwrap().unwrap();
    let manifest = doc.messages[0].attachments.as_ref().unwrap();
    assert_eq!(manifest[0].path.as_deref(), Some("notes.txt"));
    assert!(!manifest[0].hash.is_empty() && manifest[0].included);
    let (status, missing) = call(
        &host,
        "POST",
        "/api/chat",
        json!({"messages":[{"role":"user","content":"missing"}],"attachments":[{"workspaceId":workspace,"path":"absent.txt"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !missing.contains("\"type\":\"context\"") && missing.contains("\"type\":\"done\""),
        "{missing}"
    );
    let requests = engine.requests.lock().unwrap().clone();
    assert!(
        !requests[2]
            .messages
            .iter()
            .any(|message| message.content.contains("FILE:"))
    );
}

#[tokio::test]
async fn connect_reports_a_terminal_command_or_setup_for_the_active_server() {
    use rigspark_runtime::state::{Config, RuntimeState, ServerState, StateStore};
    let (home, host) = host(Arc::default());
    let (status, _) = call(&host, "GET", "/api/connect/claude", Value::Null).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "nothing to connect without a running model"
    );
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(std::time::Duration::from_secs(5)).unwrap();
    let active = ServerState {
        backend: "ollama".into(),
        model_id: "qwen3:14b".into(),
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
    };
    store
        .write(&guard, &RuntimeState::for_active(active))
        .unwrap();
    drop(guard);
    let (_, claude) = json_call(&host, "GET", "/api/connect/claude", Value::Null).await;
    assert_eq!(claude["kind"], "launch");
    assert_eq!(claude["label"], "Claude Code");
    assert_eq!(claude["command"], "rigspark connect claude");
    let (_, cline) = json_call(&host, "GET", "/api/connect/cline", Value::Null).await;
    assert_eq!(cline["kind"], "instructions");
    assert!(
        cline["text"]
            .as_str()
            .unwrap()
            .contains("http://127.0.0.1:11434/v1")
    );
    let (_, agents) = json_call(&host, "GET", "/api/connect", Value::Null).await;
    assert_eq!(agents["agents"].as_array().unwrap().len(), 7);
    for bad in [
        "/api/connect/bash",
        "/api/connect/../claude",
        "/api/connect/Claude",
    ] {
        assert!(
            call(&host, "GET", bad, Value::Null)
                .await
                .0
                .is_client_error(),
            "{bad}"
        );
    }
}

fn hugging_face_fixture() -> rigspark_runtime::admission::RecordedAdmissionTransport {
    fn string(out: &mut Vec<u8>, text: &str) {
        out.extend((text.len() as u64).to_le_bytes());
        out.extend(text.as_bytes());
    }
    let mut header = b"GGUF".to_vec();
    header.extend(3u32.to_le_bytes());
    header.extend(1u64.to_le_bytes());
    header.extend(3u64.to_le_bytes());
    for (key, text) in [
        ("general.architecture", "tiny"),
        ("general.license", "apache-2.0"),
    ] {
        string(&mut header, key);
        header.extend(8u32.to_le_bytes());
        string(&mut header, text);
    }
    string(&mut header, "tiny.context_length");
    header.extend(4u32.to_le_bytes());
    header.extend(32768u32.to_le_bytes());
    string(&mut header, "output.weight");
    header.extend(1u32.to_le_bytes());
    header.extend(1_000_000_000u64.to_le_bytes());
    header.extend(12u32.to_le_bytes());
    header.extend(0u64.to_le_bytes());
    let revision = "a".repeat(40);
    let metadata = json!({
        "id": "owner/Tiny-1B-GGUF", "sha": revision, "gated": false, "private": false,
        "tags": ["gguf", "license:apache-2.0"], "pipeline_tag": "text-generation",
        "siblings": [
            {"rfilename": "Tiny-1B-Q4_K_M.gguf", "lfs": {"sha256": "b".repeat(64), "size": 700_000_000}},
            {"rfilename": "Tiny-1B-Q8_0.gguf", "lfs": {"sha256": "c".repeat(64), "size": 1_100_000_000}}
        ]
    });
    let search = rigspark_runtime::hf_add::search_url("tiny")
        .unwrap()
        .to_string();
    rigspark_runtime::admission::RecordedAdmissionTransport::new(
        [
            (
                "https://huggingface.co/api/models/owner/Tiny-1B-GGUF?blobs=true".to_string(),
                (200, serde_json::to_vec(&metadata).unwrap()),
            ),
            (
                format!("https://huggingface.co/owner/Tiny-1B-GGUF/resolve/{revision}/Tiny-1B-Q4_K_M.gguf"),
                (206, header),
            ),
            (
                search,
                (200, serde_json::to_vec(&json!([{"id": "owner/Tiny-1B-GGUF", "downloads": 9}])).unwrap()),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .unwrap()
}

#[tokio::test]
async fn hugging_face_models_are_searched_inspected_added_and_removed() {
    let (_home, host) = host(Arc::default());
    host.set_hugging_face(Arc::new(hugging_face_fixture()));
    let (_, found) = json_call(
        &host,
        "POST",
        "/api/catalog/local/search",
        json!({"query":"tiny"}),
    )
    .await;
    assert_eq!(found["results"][0]["id"], "owner/Tiny-1B-GGUF");
    let (_, inspected) = json_call(
        &host,
        "POST",
        "/api/catalog/local/inspect",
        json!({"reference":"https://huggingface.co/owner/Tiny-1B-GGUF"}),
    )
    .await;
    assert_eq!(inspected["license"], "apache-2.0");
    let quants: Vec<&str> = inspected["choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|choice| choice["quant"].as_str().unwrap())
        .collect();
    assert_eq!(quants, ["Q4_K_M", "Q8_0"]);
    assert!(inspected["choices"][0]["fits"].is_boolean());
    let (_, added) = json_call(
        &host,
        "POST",
        "/api/catalog/local/add",
        json!({"reference":"owner/Tiny-1B-GGUF","quant":"Q8_0","bypass":true}),
    )
    .await;
    assert_eq!(added["model"], "hf.co/owner/tiny-1b-gguf:q8_0");
    let (_, listed) = json_call(&host, "GET", "/api/catalog/local", Value::Null).await;
    assert_eq!(listed["models"][0]["id"], "hf.co/owner/tiny-1b-gguf:q8_0");
    let (_, status) = json_call(&host, "GET", "/api/catalog/status", Value::Null).await;
    assert_eq!(status["catalog"]["localCount"], 1);
    let (_, removed) = json_call(
        &host,
        "POST",
        "/api/catalog/local/remove",
        json!({"id":"hf.co/owner/tiny-1b-gguf:q8_0"}),
    )
    .await;
    assert_eq!(removed["removed"], true);
}

#[tokio::test]
async fn hugging_face_requests_are_validated() {
    let (_home, host) = host(Arc::default());
    host.set_hugging_face(Arc::new(hugging_face_fixture()));
    for (path, body) in [
        ("/api/catalog/local/search", json!({"query":"a&b"})),
        (
            "/api/catalog/local/inspect",
            json!({"reference":"../etc/passwd"}),
        ),
        (
            "/api/catalog/local/inspect",
            json!({"reference":"owner/Tiny-1B-GGUF","extra":1}),
        ),
        (
            "/api/catalog/local/add",
            json!({"reference":"owner/Tiny-1B-GGUF","quant":"Q3_K_S"}),
        ),
        ("/api/catalog/local/remove", json!({"id":"missing"})),
    ] {
        let (status, _) = call(&host, "POST", path, body.clone()).await;
        assert!(status.is_client_error(), "{path} {body} -> {status}");
    }
    let (_, listed) = json_call(&host, "GET", "/api/catalog/local", Value::Null).await;
    assert_eq!(listed["models"], json!([]));
}

#[tokio::test]
async fn add_from_hugging_face_sits_in_the_models_header() {
    let (_home, host) = host(Arc::default());
    let (_, html) = call(&host, "GET", "/", Value::Null).await;
    let header = html.find("<h2>Models</h2>").unwrap();
    let button = html.find("id=\"hf-add-open\"").unwrap();
    let panel = html.find("id=\"model-catalog-panel\"").unwrap();
    assert!(
        header < button && button < panel,
        "the button belongs to the Models page head"
    );
    assert_eq!(html.matches("id=\"hf-add-open\"").count(), 1);
}
