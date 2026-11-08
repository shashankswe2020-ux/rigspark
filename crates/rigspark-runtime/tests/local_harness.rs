use rigspark_runtime::{
    adapters::{BackendAdapter, BackendError, ServeRequest},
    harness::{ChatHarness, HarnessError, HarnessMessage, HarnessRequest, LocalHarness},
    identity::{Listener, ProcessIdentity, ProcessProbe},
    lifecycle::Registry,
    ollama_inference::{ChatInput, ChatResult},
    state::{Config, RuntimeState, ServerState, StateError, StateStore},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct Fixture;
#[async_trait::async_trait]
impl ProcessProbe for Fixture {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Ok(Listener {
            identity: ProcessIdentity {
                pid: 123,
                process: "ollama".into(),
                executable: "/fixture/ollama".into(),
                started: "instance".into(),
            },
            address: host.into(),
            port,
        })
    }
    async fn process(&self, _: u32) -> Result<ProcessIdentity, StateError> {
        Ok(self.listener(11434, "127.0.0.1").await?.identity)
    }
}
#[async_trait::async_trait]
impl BackendAdapter for Fixture {
    fn name(&self) -> &'static str {
        "ollama"
    }
    fn can_embed(&self) -> bool {
        false
    }
    fn trusts(&self, _: &ProcessIdentity) -> bool {
        true
    }
    async fn serve(
        &self,
        _: &ServeRequest,
        _: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        unreachable!()
    }
    async fn ready(&self, _: &ServeRequest, _: &CancellationToken) -> Result<(), BackendError> {
        unreachable!()
    }
    async fn stop(&self, _: &ServerState, _: &CancellationToken) -> Result<(), BackendError> {
        unreachable!()
    }
    async fn chat(
        &self,
        _: &ServerState,
        _: &ChatInput,
        _: &CancellationToken,
    ) -> Result<ChatResult, BackendError> {
        Ok(ChatResult {
            content: "first second".into(),
            tool_calls: vec![],
            timings: None,
        })
    }
    async fn chat_stream(
        &self,
        _: &ServerState,
        _: &ChatInput,
        cancel: &CancellationToken,
        sink: &mut (dyn for<'chunk> FnMut(&'chunk str) + Send),
    ) -> Result<ChatResult, BackendError> {
        sink("first");
        if cancel.is_cancelled() {
            return Err(BackendError("cancelled".into()));
        }
        sink(" second");
        Ok(ChatResult {
            content: "first second".into(),
            tool_calls: vec![],
            timings: None,
        })
    }
}

#[tokio::test]
async fn local_harness_streams_and_propagates_sink_failure() {
    let home = tempfile::tempdir().unwrap();
    let state = StateStore::new(Config::from_home(home.path()).unwrap());
    let value: RuntimeState = serde_json::from_value(serde_json::json!({"schemaVersion":2,"active":{"modelId":"test:latest","backend":"ollama","endpoint":"http://127.0.0.1:11434","port":11434,"ownedByUs":false,"pid":123,"processExecutable":"/fixture/ollama","processStartedAt":"instance"}})).unwrap();
    let guard = state.lock(Duration::from_secs(1)).unwrap();
    state.write(&guard, &value).unwrap();
    guard.release().unwrap();
    let registry = Registry::new(vec![&Fixture]);
    let harness = LocalHarness {
        state: &state,
        registry: &registry,
        probe: &Fixture,
    };
    let input = HarnessRequest {
        model: "test:latest".into(),
        messages: vec![HarnessMessage {
            role: "user".into(),
            content: "question".into(),
        }],
        temperature: None,
    };
    let mut chunks = Vec::new();
    let reply = harness
        .chat(&input, &CancellationToken::new(), &mut |text| {
            chunks.push(text.to_owned());
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(chunks, vec!["first", " second"]);
    assert_eq!(reply, chunks.concat());
    let result = harness
        .chat(&input, &CancellationToken::new(), &mut |_| {
            Err(HarnessError::Limit)
        })
        .await;
    assert!(matches!(result, Err(HarnessError::Limit)));
    let source = rigspark_runtime::memory::MemoryStore::open(home.path(), "source", "now").unwrap();
    let cancel = CancellationToken::new();
    source
        .capture(
            &"long history ".repeat(500),
            "reply",
            rigspark_runtime::memory::CaptureOptions {
                timestamp: "now",
                embedder: None,
                embedding_unsupported: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    let migration = rigspark_runtime::migration_service::MigrationService {
        state: &state,
        registry: &registry,
        probe: &Fixture,
    };
    let summary = migration
        .run(
            rigspark_runtime::migration_service::MigrationRequest {
                source: "source",
                target: "test:latest",
                context: 300,
                dry_run: false,
                move_source: false,
                timestamp: "now",
                embedder: None,
                target_dimension: None,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(summary.strategy, "summarize");
    let target =
        rigspark_runtime::memory::MemoryStore::existing(home.path(), "test:latest").unwrap();
    assert!(
        target.load().unwrap().turns[0]
            .content
            .contains("first second")
    );
    assert_eq!(target.meta.embedding_unsupported, Some(true));
}
