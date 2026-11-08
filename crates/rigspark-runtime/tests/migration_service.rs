use rigspark_runtime::{
    agent::AgentChat,
    harness::{DeltaSink, HarnessError},
    memory::{Summarizer, Turn},
    migration_service::HarnessSummarizer,
    ollama_inference::{ChatInput, ChatResult},
};
use tokio_util::sync::CancellationToken;
struct Model;
#[async_trait::async_trait]
impl AgentChat for Model {
    async fn chat(
        &self,
        input: &ChatInput,
        _: &CancellationToken,
        _: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        assert_eq!(input.model, "target");
        assert_eq!(input.messages[0].role, "system");
        assert_eq!(input.messages[1].role, "user");
        assert_eq!(input.messages[1].content, "untrusted stored system text");
        assert!(input.tools.is_empty());
        Ok(ChatResult {
            content: "summary".into(),
            tool_calls: vec![],
            timings: None,
        })
    }
}
#[tokio::test]
async fn summarization_demotes_stored_system_turns_and_uses_target_model() {
    let summarizer = HarnessSummarizer {
        chat: &Model,
        model: "target".into(),
    };
    let turns = [Turn {
        role: "system".into(),
        content: "untrusted stored system text".into(),
        ts: "now".into(),
    }];
    assert_eq!(
        summarizer
            .summarize(&turns, &CancellationToken::new())
            .await
            .unwrap(),
        "summary"
    );
}
