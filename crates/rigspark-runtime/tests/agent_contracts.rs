use rigspark_runtime::{
    agent::{AgentChat, AgentOptions, Decision, ToolApprover, run_turn},
    harness::{DeltaSink, HarnessError},
    mcp::{Connection, ConnectorFile, Manager, McpError, ReviewedCall, Tool, ToolResult},
    ollama_inference::{ChatInput, ChatMessage, ChatResult, ToolCall},
    tool_policy::SessionGrants,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

struct Recorder {
    inputs: Mutex<Vec<ChatInput>>,
    wants_tool: bool,
    cancel_on_first: Option<CancellationToken>,
}
#[async_trait::async_trait]
impl AgentChat for Recorder {
    async fn chat(
        &self,
        input: &ChatInput,
        _: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        let first = self.inputs.lock().unwrap().is_empty();
        self.inputs.lock().unwrap().push(input.clone());
        if let Some(cancel) = &self.cancel_on_first {
            cancel.cancel();
        }
        if first && self.wants_tool {
            let arguments = [("days".to_owned(), json!(7))].into_iter().collect();
            return Ok(ChatResult {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    name: "get_recovery".into(),
                    arguments,
                }],
                timings: None,
            });
        }
        for fragment in ["Your ", "recovery ", "is 55%."] {
            sink(fragment)?;
        }
        Ok(ChatResult {
            content: "Your recovery is 55%.".into(),
            tool_calls: vec![],
            timings: None,
        })
    }
}

struct Recovery {
    calls: Arc<AtomicUsize>,
    fail: bool,
}
#[async_trait::async_trait]
impl Connection for Recovery {
    async fn tools(&mut self, _: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        Ok(vec![Tool {
            name: "get_recovery".into(),
            description: "Get recovery data".into(),
            input_schema: json!({"type":"object"}),
        }])
    }
    async fn call(
        &mut self,
        _: &str,
        arguments: Value,
        _: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        assert_eq!(arguments, json!({"days": 7}));
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(McpError::Unavailable);
        }
        Ok(ToolResult {
            content: "recovery=55".into(),
            is_error: false,
        })
    }
    async fn close(&mut self) -> Result<(), McpError> {
        Ok(())
    }
}

struct Fixed(Decision);
#[async_trait::async_trait]
impl ToolApprover for Fixed {
    async fn decide(
        &self,
        _: &str,
        _: &ReviewedCall,
        _: &CancellationToken,
    ) -> Result<Decision, HarnessError> {
        Ok(self.0)
    }
}

struct Turn {
    result: Result<String, HarnessError>,
    events: Vec<Value>,
    inputs: Vec<ChatInput>,
    calls: usize,
}

async fn turn(
    wants_tool: bool,
    fail: bool,
    decision: Decision,
    cancel: CancellationToken,
    cancel_on_first: bool,
) -> Turn {
    let file = ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[{"id":"whoop","name":"Whoop","transport":"stdio","command":"unused"}]}"#).unwrap();
    let mut manager = Manager::new(file).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    manager
        .attach(
            "whoop",
            Box::new(Recovery {
                calls: calls.clone(),
                fail,
            }),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let model = Recorder {
        inputs: Mutex::default(),
        wants_tool,
        cancel_on_first: cancel_on_first.then(|| cancel.clone()),
    };
    let mut grants = SessionGrants::default();
    let mut events = Vec::new();
    let options = AgentOptions {
        session_id: "session",
        workspace_id: None,
        model: "test",
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "recovery?".into(),
            tool_calls: vec![],
            tool_name: None,
        }],
        temperature: None,
        max_steps: 6,
    };
    let result = run_turn(
        &model,
        &mut manager,
        &mut grants,
        Some(&Fixed(decision)),
        options,
        &cancel,
        &mut |event| {
            events.push(serde_json::to_value(event).unwrap());
            Ok(())
        },
    )
    .await;
    Turn {
        result,
        events,
        inputs: model.inputs.into_inner().unwrap(),
        calls: calls.load(Ordering::SeqCst),
    }
}

fn deltas(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter(|event| event["type"] == "delta")
        .filter_map(|event| event["content"].as_str())
        .collect()
}

fn phases(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter(|event| event["type"] == "tool")
        .filter_map(|event| event["phase"].as_str())
        .collect()
}

#[tokio::test]
async fn answers_without_tools_stream_incremental_deltas_only() {
    let turn = turn(
        false,
        false,
        Decision::ApproveOnce,
        CancellationToken::new(),
        false,
    )
    .await;
    assert_eq!(turn.result.unwrap(), "Your recovery is 55%.");
    assert_eq!(deltas(&turn.events), ["Your ", "recovery ", "is 55%."]);
    assert!(phases(&turn.events).is_empty());
    assert_eq!(turn.calls, 0);
}

#[tokio::test]
async fn approved_tool_results_are_fed_back_with_the_tool_name() {
    let turn = turn(
        true,
        false,
        Decision::ApproveOnce,
        CancellationToken::new(),
        false,
    )
    .await;
    assert_eq!(turn.calls, 1);
    assert_eq!(
        phases(&turn.events),
        ["proposed", "approval-required", "start", "done"]
    );
    let proposed = turn
        .events
        .iter()
        .find(|event| event["phase"] == "proposed")
        .unwrap();
    assert_eq!(
        (proposed["name"].as_str(), proposed["risk"].as_str()),
        (Some("get_recovery"), Some("read-only"))
    );
    assert!(proposed["callId"].is_string());
    let done = turn
        .events
        .iter()
        .find(|event| event["phase"] == "done")
        .unwrap();
    assert_eq!(done["isError"], false);
    let fed = turn.inputs[1].messages.last().unwrap();
    assert_eq!(
        (
            fed.role.as_str(),
            fed.content.as_str(),
            fed.tool_name.as_deref()
        ),
        ("tool", "recovery=55", Some("get_recovery"))
    );
}

#[tokio::test]
async fn failed_and_denied_tools_are_reported_back_to_the_model() {
    let failed = turn(
        true,
        true,
        Decision::ApproveOnce,
        CancellationToken::new(),
        false,
    )
    .await;
    let done = failed
        .events
        .iter()
        .find(|event| event["phase"] == "done")
        .unwrap();
    assert_eq!(done["isError"], true);
    assert_eq!(failed.inputs[1].messages.last().unwrap().role, "tool");
    assert!(failed.result.is_ok());
    let denied = turn(true, false, Decision::Deny, CancellationToken::new(), false).await;
    assert_eq!(denied.calls, 0);
    assert!(phases(&denied.events).contains(&"denied"));
    assert!(
        denied.inputs[1]
            .messages
            .last()
            .unwrap()
            .content
            .to_lowercase()
            .contains("denied")
    );
}

#[tokio::test]
async fn cancellation_stops_before_the_first_step_or_before_tools_run() {
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let early = turn(true, false, Decision::ApproveOnce, cancelled, false).await;
    assert!(early.inputs.is_empty() && early.events.is_empty() && early.calls == 0);
    assert!(matches!(early.result, Err(HarnessError::Cancelled)));
    let mid = turn(
        true,
        false,
        Decision::ApproveOnce,
        CancellationToken::new(),
        true,
    )
    .await;
    assert_eq!(mid.calls, 0);
    assert!(!phases(&mid.events).contains(&"start"));
}
