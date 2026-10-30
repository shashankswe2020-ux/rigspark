use crate::{
    adapters::{BackendAdapter, BackendError, ServeRequest},
    http::{Request, Transport, read_json},
    identity::{ProcessProbe, capture},
    ollama_inference::{ChatInput, ChatResult, EmbedResult},
    process_control::listener,
    state::ServerState,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;
pub fn chat_result(value: Value) -> Result<ChatResult, BackendError> {
    crate::usage::begin();
    crate::usage::record(&value, crate::usage::Provider::OpenAi);
    #[derive(Deserialize)]
    struct Message {
        content: String,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Message,
    }
    #[derive(Deserialize)]
    struct Reply {
        choices: Vec<Choice>,
    }
    let reply: Reply =
        serde_json::from_value(value).map_err(|_| BackendError("invalid chat response".into()))?;
    if reply.choices.is_empty() || reply.choices.len() > 8 {
        return Err(BackendError("invalid chat choice count".into()));
    }
    let content = reply
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| BackendError("no chat choice".into()))?
        .message
        .content;
    if content.len() > 1048576 {
        return Err(BackendError("generated content exceeds limit".into()));
    }
    Ok(ChatResult {
        content,
        tool_calls: Vec::new(),
    })
}
pub fn embedding_result(value: Value, count: usize) -> Result<EmbedResult, BackendError> {
    #[derive(Deserialize)]
    struct Vector {
        index: usize,
        embedding: Vec<f64>,
    }
    #[derive(Deserialize)]
    struct Reply {
        data: Vec<Vector>,
    }
    let mut reply: Reply = serde_json::from_value(value)
        .map_err(|_| BackendError("invalid embedding response".into()))?;
    if count == 0 || count > 1024 || reply.data.len() != count {
        return Err(BackendError("embedding count mismatch".into()));
    }
    reply.data.sort_by_key(|entry| entry.index);
    let dimension = reply.data[0].embedding.len();
    if dimension == 0
        || dimension > 65536
        || reply.data.iter().enumerate().any(|(index, entry)| {
            entry.index != index
                || entry.embedding.len() != dimension
                || entry.embedding.iter().any(|value| !value.is_finite())
        })
    {
        return Err(BackendError("inconsistent embedding vectors".into()));
    }
    Ok(EmbedResult {
        vectors: reply
            .data
            .into_iter()
            .map(|entry| entry.embedding)
            .collect(),
        dimension,
    })
}
pub struct OpenAiInference<'runtime> {
    pub http: &'runtime dyn Transport,
    pub probe: &'runtime dyn ProcessProbe,
    pub adapter: &'runtime dyn BackendAdapter,
    pub token: Option<&'runtime str>,
}
impl OpenAiInference<'_> {
    async fn checked(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        if handle.backend != self.adapter.name() {
            return Err(BackendError("inference backend mismatch".into()));
        }
        let observed = listener(self.probe, &handle.endpoint, cancel).await?;
        capture(handle, &observed, self.adapter.trusts(&observed.identity))
            .map_err(|error| BackendError(error.to_string()))?;
        if handle.backend == "lmstudio" {
            self.adapter
                .ready_handle(
                    &ServeRequest {
                        draft_model_path: None,
                        model_id: handle.model_id.clone(),
                        endpoint: handle.endpoint.clone(),
                        model_path: handle.model_path.as_ref().map(PathBuf::from),
                        context: None,
                        cache: None,
                    },
                    handle,
                    cancel,
                )
                .await?;
        }
        Ok(())
    }
    async fn json(
        &self,
        handle: &ServerState,
        path: &str,
        body: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, BackendError> {
        if serde_json::to_vec(&body)
            .map_err(|error| BackendError(error.to_string()))?
            .len()
            > 4 * 1024 * 1024
        {
            return Err(BackendError("inference request exceeds limit".into()));
        }
        let token = if handle.backend == "mlx" {
            handle.auth_token.clone()
        } else {
            self.token.map(str::to_owned)
        };
        let request = Request::new(&handle.endpoint, path, Some(body), token)
            .map_err(|error| BackendError(error.to_string()))?;
        tokio::time::timeout(
            Duration::from_secs(30),
            read_json(self.http, request, cancel, 2 * 1024 * 1024),
        )
        .await
        .map_err(|_| BackendError("inference timed out".into()))?
        .map_err(|error| BackendError(error.to_string()))
    }
    pub async fn chat(
        &self,
        handle: &ServerState,
        input: &ChatInput,
        cancel: &CancellationToken,
    ) -> Result<ChatResult, BackendError> {
        crate::adapters::model_id(&input.model)?;
        if input.model != handle.model_id
            || input.messages.len() > 10000
            || input.messages.iter().any(|message| {
                !["user", "assistant", "system", "tool"].contains(&message.role.as_str())
            })
        {
            return Err(BackendError("invalid chat target or messages".into()));
        }
        self.checked(handle, cancel).await?;
        let model = if handle.backend == "mlx" {
            "default_model"
        } else {
            &input.model
        };
        let messages: Vec<_> = input
            .messages
            .iter()
            .map(|message| json!({"role":message.role,"content":message.content}))
            .collect();
        let result = chat_result(
            self.json(
                handle,
                "/v1/chat/completions",
                json!({"model":model,"messages":messages,"stream":false}),
                cancel,
            )
            .await?,
        )?;
        self.checked(handle, cancel).await?;
        Ok(result)
    }
    pub async fn embed(
        &self,
        handle: &ServerState,
        model: &str,
        inputs: &[String],
        cancel: &CancellationToken,
    ) -> Result<EmbedResult, BackendError> {
        if handle.backend != "lmstudio" {
            return Err(BackendError("backend does not support embeddings".into()));
        }
        crate::adapters::model_id(model)?;
        if model != handle.model_id || inputs.is_empty() || inputs.len() > 1024 {
            return Err(BackendError(
                "invalid embedding target or input count".into(),
            ));
        }
        self.checked(handle, cancel).await?;
        let result = embedding_result(
            self.json(
                handle,
                "/v1/embeddings",
                json!({"model":model,"input":inputs}),
                cancel,
            )
            .await?,
            inputs.len(),
        )?;
        self.checked(handle, cancel).await?;
        Ok(result)
    }
}
