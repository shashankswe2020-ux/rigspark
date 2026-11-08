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

/// A backend that streams `streamed` bytes in one chunk, then `extra` more, and replies
/// with `content` bytes.
struct Sized {
    streamed: usize,
    extra: usize,
    content: usize,
}
#[async_trait::async_trait]
impl BackendAdapter for Sized {
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
        unreachable!()
    }
    async fn chat_stream(
        &self,
        _: &ServerState,
        _: &ChatInput,
        _: &CancellationToken,
        sink: &mut (dyn for<'chunk> FnMut(&'chunk str) + Send),
    ) -> Result<ChatResult, BackendError> {
        if self.streamed > 0 {
            sink(&"s".repeat(self.streamed));
        }
        if self.extra > 0 {
            sink(&"e".repeat(self.extra));
        }
        Ok(ChatResult {
            content: "c".repeat(self.content),
            tool_calls: vec![],
            timings: None,
        })
    }
}

fn store(home: &std::path::Path) -> StateStore {
    let state = StateStore::new(Config::from_home(home).unwrap());
    let value: RuntimeState = serde_json::from_value(serde_json::json!({"schemaVersion":2,"active":{"modelId":"test:latest","backend":"ollama","endpoint":"http://127.0.0.1:11434","port":11434,"ownedByUs":false,"pid":123,"processExecutable":"/fixture/ollama","processStartedAt":"instance"}})).unwrap();
    let guard = state.lock(Duration::from_secs(1)).unwrap();
    state.write(&guard, &value).unwrap();
    guard.release().unwrap();
    state
}

fn chat_input(messages: usize, tools: usize, temperature: Option<f64>) -> ChatInput {
    use rigspark_runtime::ollama_inference::{ChatMessage, Tool};
    ChatInput {
        model: String::new(),
        messages: (0..messages)
            .map(|_| ChatMessage {
                role: "user".into(),
                content: String::new(),
                tool_calls: vec![],
                tool_name: None,
            })
            .collect(),
        tools: (0..tools)
            .map(|_| Tool {
                name: "t".into(),
                description: String::new(),
                parameters: Default::default(),
            })
            .collect(),
        temperature,
        slot: None,
    }
}

async fn local_chat(
    backend: &Sized,
    input: &ChatInput,
) -> Result<(ChatResult, usize), HarnessError> {
    use rigspark_runtime::agent::AgentChat;
    let home = tempfile::tempdir().unwrap();
    let state = store(home.path());
    let registry = Registry::new(vec![backend]);
    let harness = LocalHarness {
        state: &state,
        registry: &registry,
        probe: &Fixture,
    };
    let mut seen = 0;
    let result = AgentChat::chat(&harness, input, &CancellationToken::new(), &mut |text| {
        seen += text.len();
        Ok(())
    })
    .await?;
    Ok((result, seen))
}

#[tokio::test]
async fn local_requests_are_bounded_before_reaching_the_backend() {
    let quiet = Sized {
        streamed: 0,
        extra: 0,
        content: 1,
    };
    let invalid = |result: Result<(ChatResult, usize), HarnessError>| {
        matches!(result, Err(HarnessError::Invalid))
    };
    assert!(
        local_chat(&quiet, &chat_input(10000, 0, None))
            .await
            .is_ok()
    );
    assert!(invalid(
        local_chat(&quiet, &chat_input(10001, 0, None)).await
    ));
    assert!(local_chat(&quiet, &chat_input(1, 1024, None)).await.is_ok());
    assert!(invalid(
        local_chat(&quiet, &chat_input(1, 1025, None)).await
    ));
    assert!(
        local_chat(&quiet, &chat_input(1, 0, Some(0.5)))
            .await
            .is_ok()
    );
    assert!(invalid(
        local_chat(&quiet, &chat_input(1, 0, Some(f64::NAN))).await
    ));
    assert!(invalid(
        local_chat(&quiet, &chat_input(1, 0, Some(f64::INFINITY))).await
    ));
    // Exactly 4 MiB serialized is accepted; one byte more is not.
    let mut input = chat_input(1, 0, None);
    let room = 4 * 1024 * 1024 - serde_json::to_vec(&input).unwrap().len();
    input.messages[0].content = "m".repeat(room);
    assert_eq!(serde_json::to_vec(&input).unwrap().len(), 4 * 1024 * 1024);
    assert!(local_chat(&quiet, &input).await.is_ok());
    input.messages[0].content.push('m');
    assert!(invalid(local_chat(&quiet, &input).await));
}

#[tokio::test]
async fn local_output_is_capped_at_16_mib_streamed_and_returned() {
    let mib = 1024 * 1024;
    let input = chat_input(1, 0, None);
    let limited = |result: &Result<(ChatResult, usize), HarnessError>| {
        matches!(result, Err(HarnessError::Limit))
    };
    // Streamed text: 16 MiB in total reaches the sink, a byte more stops the reply.
    let (_, seen) = local_chat(
        &Sized {
            streamed: 16 * mib - 1,
            extra: 1,
            content: 1,
        },
        &input,
    )
    .await
    .unwrap();
    assert_eq!(seen, 16 * mib);
    assert!(limited(
        &local_chat(
            &Sized {
                streamed: 16 * mib,
                extra: 1,
                content: 1,
            },
            &input,
        )
        .await
    ));
    // The final reply.
    let (reply, _) = local_chat(
        &Sized {
            streamed: 0,
            extra: 0,
            content: 16 * mib,
        },
        &input,
    )
    .await
    .unwrap();
    assert_eq!(reply.content.len(), 16 * mib);
    assert!(limited(
        &local_chat(
            &Sized {
                streamed: 0,
                extra: 0,
                content: 16 * mib + 1,
            },
            &input,
        )
        .await
    ));
}
