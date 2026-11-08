use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use rigspark_gui::{Host, engine::Engine, router};
use rigspark_runtime::{
    harness::{DeltaSink, HarnessError, HarnessRequest},
    mcp::{Connection, ConnectorFile, Manager, McpError, Tool, ToolResult},
    ollama_inference::{ChatInput, ChatResult, ToolCall},
    state::RuntimeState,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Default)]
struct Model(AtomicUsize);
#[async_trait::async_trait]
impl Engine for Model {
    async fn chat(
        &self,
        _: &str,
        _: &HarnessRequest,
        _: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        sink("reply")?;
        Ok("reply".into())
    }
    async fn agent(
        &self,
        input: &ChatInput,
        _: &RuntimeState,
        _: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        if input.messages.last().unwrap().role == "tool" {
            sink("all done")?;
            return Ok(ChatResult {
                content: "all done".into(),
                tool_calls: vec![],
                timings: None,
            });
        }
        let arguments = [("path".to_owned(), json!("a.txt"))].into_iter().collect();
        Ok(ChatResult {
            content: String::new(),
            tool_calls: vec![ToolCall {
                name: "do_write".into(),
                arguments,
            }],
            timings: None,
        })
    }
}

struct Writer(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Connection for Writer {
    async fn tools(&mut self, _: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        Ok(vec![Tool {
            name: "do_write".into(),
            description: "write a file".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        }])
    }
    async fn call(
        &mut self,
        _: &str,
        _: Value,
        _: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: "wrote a.txt".into(),
            is_error: false,
        })
    }
    async fn close(&mut self) -> Result<(), McpError> {
        Ok(())
    }
}

fn request(host: &Host, method: &str, path: &str, payload: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("127.0.0.1:{}", host.port))
        .header("origin", host.origin())
        .header("x-llmup-token", &host.token)
        .header("content-type", "application/json")
        .body(if payload.is_null() {
            Body::empty()
        } else {
            Body::from(payload.to_string())
        })
        .unwrap()
}

async fn call(
    host: &Arc<Host>,
    method: &str,
    path: &str,
    payload: Value,
) -> (StatusCode, Value, String) {
    let response = router(host.clone())
        .oneshot(request(host, method, path, &payload))
        .await
        .unwrap();
    let status = response.status();
    let text = String::from_utf8(
        to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

fn events(stream: &str) -> Vec<Value> {
    stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

#[tokio::test]
async fn cloud_context_requires_disclosure_once_and_local_or_contextless_sends_do_not() {
    let home = tempfile::tempdir().unwrap();
    let model = Arc::new(Model::default());
    let host = Host::with_engine(home.path(), 43210, model.clone()).unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.ts"), "export const x = 1;\n").unwrap();
    let (_, registered, _) = call(
        &host,
        "POST",
        "/api/workspace/root",
        json!({"path": root.path()}),
    )
    .await;
    let id = registered["root"]["id"].clone();
    let send = |harness: Option<&str>, attach: bool, ack: bool| {
        let mut body = json!({"messages":[{"role":"user","content":"review this"}]});
        if let Some(harness) = harness {
            body["harness"] = json!(harness);
        }
        if attach {
            body["attachments"] = json!([{"workspaceId": id, "path": "index.ts"}]);
        }
        if ack {
            body["disclosureAck"] = json!(true);
        }
        body
    };
    let (_, _, blocked) = call(
        &host,
        "POST",
        "/api/chat",
        send(Some("claude"), true, false),
    )
    .await;
    assert!(
        blocked.contains("\"type\":\"disclosure-required\"")
            && blocked.contains("\"provider\":\"claude\""),
        "{blocked}"
    );
    assert!(!blocked.contains("\"type\":\"delta\""));
    assert_eq!(model.0.load(Ordering::SeqCst), 0);
    let (_, _, acknowledged) =
        call(&host, "POST", "/api/chat", send(Some("claude"), true, true)).await;
    assert!(
        acknowledged.contains("\"type\":\"context\"")
            && acknowledged.contains("\"type\":\"delta\""),
        "{acknowledged}"
    );
    let (_, _, remembered) = call(
        &host,
        "POST",
        "/api/chat",
        send(Some("claude"), true, false),
    )
    .await;
    assert!(
        !remembered.contains("disclosure-required") && remembered.contains("\"type\":\"delta\""),
        "{remembered}"
    );
    assert_eq!(model.0.load(Ordering::SeqCst), 2);
    let (_, _, local) = call(&host, "POST", "/api/chat", send(None, true, false)).await;
    let (_, _, contextless) = call(
        &host,
        "POST",
        "/api/chat",
        send(Some("openai"), false, false),
    )
    .await;
    for stream in [local, contextless] {
        assert!(
            !stream.contains("disclosure-required") && stream.contains("\"type\":\"delta\""),
            "{stream}"
        );
    }
    assert_eq!(model.0.load(Ordering::SeqCst), 4);
}

async fn tool_host() -> (tempfile::TempDir, Arc<Host>, Arc<AtomicUsize>) {
    let home = tempfile::tempdir().unwrap();
    let host = Host::with_engine(home.path(), 43210, Arc::new(Model::default())).unwrap();
    let file = ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[{"id":"fs","name":"fs","transport":"stdio","command":"unused"}]}"#).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut manager = Manager::new(file).unwrap();
    manager
        .attach(
            "fs",
            Box::new(Writer(calls.clone())),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    *host.connectors.lock().await = manager;
    (home, host, calls)
}

async fn turn(host: &Arc<Host>, decision: Option<&str>) -> Vec<Value> {
    let response = router(host.clone())
        .oneshot(request(
            host,
            "POST",
            "/api/chat",
            &json!({"messages":[{"role":"user","content":"write it"}]}),
        ))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let mut output = Vec::new();
    while let Some(chunk) = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap()
    {
        let text = String::from_utf8(chunk.unwrap().to_vec()).unwrap();
        for event in events(&text) {
            if event["phase"] == "approval-required"
                && let Some(decision) = decision
            {
                let (status, _, _) = call(
                    host,
                    "POST",
                    "/api/chat/tool-decision",
                    json!({"callId": event["callId"], "decision": decision}),
                )
                .await;
                assert_eq!(status, StatusCode::OK);
            }
            output.push(event);
        }
    }
    output
}

fn phases(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event["type"] == "tool")
        .map(|event| event["phase"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn approved_tools_run_once_in_documented_phase_order() {
    let (_home, host, calls) = tool_host().await;
    let events = turn(&host, Some("approve-once")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        phases(&events),
        ["proposed", "approval-required", "start", "done"]
    );
    assert!(
        events
            .iter()
            .any(|event| event["type"] == "delta" && event["content"] == "all done")
    );
}

#[tokio::test]
async fn denied_tools_never_run() {
    let (_home, host, calls) = tool_host().await;
    let events = turn(&host, Some("deny")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(phases(&events).contains(&"denied".to_owned()));
}

#[tokio::test]
async fn session_grants_skip_the_next_prompt_for_the_same_tool() {
    let (_home, host, calls) = tool_host().await;
    assert!(
        phases(&turn(&host, Some("allow-session")).await).contains(&"approval-required".to_owned())
    );
    let second = phases(&turn(&host, None).await);
    assert!(
        !second.contains(&"approval-required".to_owned()) && second.contains(&"done".to_owned()),
        "{second:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let (status, _, _) = call(
        &host,
        "POST",
        "/api/chat/tool-decision",
        json!({"callId":"nope","decision":"approve-once"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn connector_routes_list_add_replace_disconnect_and_remove_without_spawning() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    let (status, added, _) = call(
        &host,
        "POST",
        "/api/connectors",
        json!({"name":"fs","transport":"stdio","command":"never-run"}),
    )
    .await;
    assert_eq!(
        (status, added["connector"]["id"].as_str()),
        (StatusCode::CREATED, Some("fs"))
    );
    let (status, listed, _) = call(&host, "GET", "/api/connectors", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["connectors"][0]["status"], "disconnected");
    for (name, expected) in [
        ("FS", "fs-2"),
        ("My Remote!", "my-remote"),
        ("!!!", "connector"),
    ] {
        let (status, added, _) = call(
            &host,
            "POST",
            "/api/connectors",
            json!({"name":name,"transport":"http","url":"http://127.0.0.1:9000/mcp"}),
        )
        .await;
        assert_eq!(
            (status, added["connector"]["id"].as_str()),
            (StatusCode::CREATED, Some(expected))
        );
    }
    let (_, config, _) = call(&host, "GET", "/api/connectors/config", Value::Null).await;
    assert_eq!(config["config"]["connectors"][0]["args"], json!([]));
    for (name, value) in [
        ("http", "http://10.0.0.1/mcp"),
        ("http", "http://user:pass@127.0.0.1/mcp"),
        ("http", "ftp://127.0.0.1/mcp"),
    ] {
        let body = json!({"name":"bad","transport":name,"url":value});
        assert_eq!(
            call(&host, "POST", "/api/connectors", body).await.0,
            StatusCode::BAD_REQUEST,
            "{value}"
        );
    }
    for extra in ["fs-2", "my-remote", "connector"] {
        assert_eq!(
            call(
                &host,
                "DELETE",
                &format!("/api/connectors/{extra}"),
                Value::Null
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let (_, config, _) = call(&host, "GET", "/api/connectors/config", Value::Null).await;
    assert_eq!(config["config"]["schemaVersion"], 1);
    assert_eq!(config["config"]["connectors"][0]["command"], "never-run");
    let (status, disconnected, _) =
        call(&host, "POST", "/api/connectors/fs/disconnect", json!({})).await;
    assert_eq!(
        (status, disconnected["connector"]["id"].as_str()),
        (StatusCode::OK, Some("fs"))
    );
    assert_eq!(
        call(&host, "POST", "/api/connectors", json!({"bogus": true}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &host,
            "PUT",
            "/api/connectors/config",
            json!({"bogus": true})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&host, "DELETE", "/api/connectors/config", Value::Null)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&host, "DELETE", "/api/connectors/fs", Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    let (status, replaced, _) = call(
        &host,
        "PUT",
        "/api/connectors/config",
        json!({"schemaVersion":1,"connectors":[]}),
    )
    .await;
    assert_eq!(
        (status, replaced["connectors"].clone()),
        (StatusCode::OK, json!([]))
    );
}

#[tokio::test]
async fn connector_config_masks_env_secrets_and_preserves_them_on_round_trip() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43210).unwrap();
    let secret = "s3cr3t-client-value";
    let (status, _, _) = call(
        &host,
        "POST",
        "/api/connectors",
        json!({"name":"whoop","transport":"stdio","command":"never-run","env":{"CLIENT_SECRET":secret,"REGION":"eu"}}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let stored = || std::fs::read_to_string(home.path().join("connectors.json")).unwrap();
    let (status, config, raw) = call(&host, "GET", "/api/connectors/config", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!raw.contains(secret), "{raw}");
    let masked = config["config"]["connectors"][0]["env"]["CLIENT_SECRET"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(masked, secret);
    let (_, listed, raw) = call(&host, "GET", "/api/connectors", Value::Null).await;
    assert!(!raw.contains(secret), "{listed}");
    let (status, _, _) = call(
        &host,
        "PUT",
        "/api/connectors/config",
        config["config"].clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        stored().contains(secret),
        "unchanged masked values keep the stored secret"
    );
    let mut changed = config["config"].clone();
    changed["connectors"][0]["env"]["CLIENT_SECRET"] = json!("rotated-value");
    let (status, _, _) = call(&host, "PUT", "/api/connectors/config", changed).await;
    assert_eq!(status, StatusCode::OK);
    assert!(stored().contains("rotated-value") && !stored().contains(secret));
    let mut orphan = config["config"].clone();
    orphan["connectors"][0]["env"] = json!({"NEW_KEY": masked});
    assert_eq!(
        call(&host, "PUT", "/api/connectors/config", orphan).await.0,
        StatusCode::BAD_REQUEST
    );
    assert!(stored().contains("rotated-value"));
}
