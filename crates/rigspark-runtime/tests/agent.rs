use rigspark_runtime::{
    agent::{AgentChat, AgentOptions, Decision, ToolApprover, run_turn},
    harness::{DeltaSink, HarnessError},
    mcp::{Connection, ConnectorFile, Manager, McpError, ReviewedCall, Tool, ToolResult},
    ollama_inference::{ChatInput, ChatMessage, ChatResult, ToolCall},
    tool_policy::SessionGrants,
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

struct Model;
#[async_trait::async_trait]
impl AgentChat for Model {
    async fn chat(
        &self,
        input: &ChatInput,
        _: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        assert_eq!(input.tools[0].name, "read_text");
        if input.messages.last().unwrap().role == "tool" {
            sink("final ")?;
            sink("answer")?;
            return Ok(ChatResult {
                content: "final answer".into(),
                tool_calls: vec![],
                timings: None,
            });
        }
        Ok(ChatResult {
            content: String::new(),
            tool_calls: vec![ToolCall {
                name: input.tools[0].name.clone(),
                arguments: [("text".into(), json!("hello"))].into(),
            }],
            timings: None,
        })
    }
}
struct ConnectionFixture(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Connection for ConnectionFixture {
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
            content: "tool reply".into(),
            is_error: false,
        })
    }
    async fn close(&mut self) -> Result<(), McpError> {
        Ok(())
    }
}
struct Approver;
#[async_trait::async_trait]
impl ToolApprover for Approver {
    async fn decide(
        &self,
        _: &str,
        _: &ReviewedCall,
        _: &CancellationToken,
    ) -> Result<Decision, HarnessError> {
        Ok(Decision::ApproveOnce)
    }
}
#[tokio::test]
async fn model_tool_result_cycle_requires_explicit_approval() {
    for approved in [false, true] {
        let definitions=ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[{"id":"fixture","name":"Fixture","transport":"stdio","command":"unused"}]}"#).unwrap();
        let mut manager = Manager::new(definitions).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        manager
            .attach(
                "fixture",
                Box::new(ConnectionFixture(calls.clone())),
                &cancel,
            )
            .await
            .unwrap();
        let mut grants = SessionGrants::default();
        let options = AgentOptions {
            session_id: "session",
            workspace_id: None,
            model: "test",
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "question".into(),
                tool_calls: vec![],
                tool_name: None,
            }],
            temperature: None,
            max_steps: 6,
        };
        let mut events = Vec::new();
        let result = run_turn(
            &Model,
            &mut manager,
            &mut grants,
            approved.then_some(&Approver as &dyn ToolApprover),
            options,
            &cancel,
            &mut |event| {
                events.push(serde_json::to_value(event).unwrap());
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "final answer");
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(approved));
        assert!(
            events
                .iter()
                .any(|event| event["phase"] == if approved { "done" } else { "denied" })
        );
        let text: String = events
            .iter()
            .filter_map(|event| event["content"].as_str())
            .collect();
        assert_eq!(text, "final answer");
    }
}

#[tokio::test]
async fn service_commits_tool_enabled_turn_as_one_exchange() {
    use rigspark_runtime::{
        chat_service::{AgentRuntime, ChatOptions, ChatService},
        context::DisclosureStore,
        library::Library,
        sessions::SessionRepository,
        workspace::WorkspaceService,
    };
    let home = tempfile::tempdir().unwrap();
    let sessions = SessionRepository::new(home.path());
    let session = sessions.create("", "now").unwrap();
    let library = Library::new(home.path());
    let workspace = WorkspaceService::new();
    let mut disclosures = DisclosureStore::default();
    let service = ChatService {
        sessions: &sessions,
        library: &library,
        workspace: &workspace,
        disclosures: &mut disclosures,
    };
    let prepared = service
        .prepare(ChatOptions {
            session_id: &session.id,
            revision: 0,
            provider: "local",
            model: "test",
            message: "question",
            context: &[],
            agent: None,
            skills: &[],
            temperature: None,
        })
        .unwrap();
    let definitions=ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[{"id":"fixture","name":"Fixture","transport":"stdio","command":"unused"}]}"#).unwrap();
    let mut manager = Manager::new(definitions).unwrap();
    let cancel = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    manager
        .attach(
            "fixture",
            Box::new(ConnectionFixture(calls.clone())),
            &cancel,
        )
        .await
        .unwrap();
    let mut grants = SessionGrants::default();
    let runtime = AgentRuntime {
        chat: &Model,
        manager: &mut manager,
        grants: &mut grants,
        approver: Some(&Approver),
        workspace_id: None,
        max_steps: 6,
    };
    let outcome = service
        .run_agent(prepared, runtime, "now", None, &cancel, &mut |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(outcome.session.revision, 1);
    assert_eq!(outcome.session.messages.len(), 2);
    assert_eq!(outcome.session.messages[1].content, "final answer");
}

struct RepeatingModel;
#[async_trait::async_trait]
impl AgentChat for RepeatingModel {
    async fn chat(
        &self,
        input: &ChatInput,
        _: &CancellationToken,
        _: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        Ok(if input.tools.is_empty() {
            ChatResult {
                content: "budget final".into(),
                tool_calls: vec![],
                timings: None,
            }
        } else {
            ChatResult {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    name: input.tools[0].name.clone(),
                    arguments: Default::default(),
                }],
                timings: None,
            }
        })
    }
}
#[tokio::test]
async fn agent_budget_and_cancelled_approval_prevent_extra_tool_calls() {
    struct CancelApproval;
    #[async_trait::async_trait]
    impl ToolApprover for CancelApproval {
        async fn decide(
            &self,
            _: &str,
            _: &ReviewedCall,
            cancel: &CancellationToken,
        ) -> Result<Decision, HarnessError> {
            cancel.cancel();
            Ok(Decision::ApproveOnce)
        }
    }
    let definitions=ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[{"id":"fixture","name":"Fixture","transport":"stdio","command":"unused"}]}"#).unwrap();
    let mut manager = Manager::new(definitions).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let cancel = CancellationToken::new();
    manager
        .attach(
            "fixture",
            Box::new(ConnectionFixture(calls.clone())),
            &cancel,
        )
        .await
        .unwrap();
    let mut grants = SessionGrants::default();
    let options = || AgentOptions {
        session_id: "session",
        workspace_id: None,
        model: "test",
        messages: vec![],
        temperature: None,
        max_steps: 2,
    };
    assert_eq!(
        run_turn(
            &RepeatingModel,
            &mut manager,
            &mut grants,
            Some(&Approver),
            options(),
            &cancel,
            &mut |_| Ok(())
        )
        .await
        .unwrap(),
        "budget final"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        run_turn(
            &RepeatingModel,
            &mut manager,
            &mut grants,
            Some(&CancelApproval),
            options(),
            &cancel,
            &mut |_| Ok(())
        )
        .await,
        Err(HarnessError::Cancelled)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
