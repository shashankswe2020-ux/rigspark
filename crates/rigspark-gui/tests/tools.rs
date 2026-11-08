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
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
struct Model;
#[async_trait::async_trait]
impl Engine for Model {
    async fn chat(
        &self,
        _: &str,
        _: &HarnessRequest,
        _: &CancellationToken,
        _: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        panic!("expected agent routing")
    }
    async fn agent(
        &self,
        input: &ChatInput,
        _: &RuntimeState,
        _: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        if input.messages.last().unwrap().role == "tool" {
            sink("tool complete")?;
            Ok(ChatResult {
                content: "tool complete".into(),
                tool_calls: vec![],
                timings: None,
            })
        } else {
            Ok(ChatResult {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    name: "read_text".into(),
                    arguments: Default::default(),
                }],
                timings: None,
            })
        }
    }
}
struct ToolConnection(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Connection for ToolConnection {
    async fn tools(&mut self, _: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        Ok(vec![Tool {
            name: "read_text".into(),
            description: "read text".into(),
            input_schema: json!({"type":"object"}),
        }])
    }
    async fn call(
        &mut self,
        _: &str,
        _: serde_json::Value,
        _: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: "result".into(),
            is_error: false,
        })
    }
    async fn close(&mut self) -> Result<(), McpError> {
        Ok(())
    }
}
fn request(host: &Host, path: &str, value: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("host", format!("127.0.0.1:{}", host.port))
        .header("origin", host.origin())
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}
#[tokio::test]
async fn tool_approval_roundtrip_is_required_before_execution() {
    approval_flow(false).await;
}
#[tokio::test]
async fn disconnect_cancels_pending_approval_before_waiting_for_manager() {
    approval_flow(true).await;
}
async fn approval_flow(disconnect: bool) {
    let home = tempfile::tempdir().unwrap();
    let host = Host::with_engine(home.path(), 43210, Arc::new(Model)).unwrap();
    let definitions=ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[{"id":"fixture","name":"Fixture","transport":"stdio","command":"unused"}]}"#).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut manager = Manager::new(definitions).unwrap();
    manager
        .attach(
            "fixture",
            Box::new(ToolConnection(calls.clone())),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    *host.connectors.lock().await = manager;
    let response = router(host.clone())
        .oneshot(request(
            &host,
            "/api/chat",
            json!({"messages":[{"role":"user","content":"use tool"}]}),
        ))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    use tokio_stream::StreamExt;
    let call_id = loop {
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        if text.contains("approval-required") {
            let data = text.trim().strip_prefix("data: ").unwrap();
            let value: serde_json::Value = serde_json::from_str(data).unwrap();
            break value["callId"].as_str().unwrap().to_owned();
        }
    };
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    if disconnect {
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            router(host.clone()).oneshot(request(
                &host,
                "/api/connectors/fixture/disconnect",
                json!({}),
            )),
        )
        .await
        .expect("disconnect must cancel the run before waiting for its lock")
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            !host
                .approvals
                .resolve(&call_id, rigspark_runtime::agent::Decision::ApproveOnce)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        host.tasks.close();
        tokio::time::timeout(std::time::Duration::from_secs(2), host.tasks.wait())
            .await
            .unwrap();
        return;
    }
    let approved = router(host.clone())
        .oneshot(request(
            &host,
            "/api/chat/tool-decision",
            json!({"callId":call_id,"decision":"approve-once"}),
        ))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let _ = to_bytes(approved.into_body(), 1024).await.unwrap();
    let mut output = String::new();
    while let Some(chunk) = stream.next().await {
        output.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
    }
    assert!(output.contains("tool complete"));
    assert!(output.contains("\"type\":\"done\""));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
