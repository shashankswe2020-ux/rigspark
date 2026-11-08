use crate::{
    adapters::{BackendAdapter, BackendError, ServeRequest},
    http::{Request, Transport, read_json},
    identity::{ProcessProbe, capture},
    ollama_inference::{ChatInput, ChatMessage, ChatResult, EmbedResult, ToolCall},
    process_control::listener,
    state::ServerState,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;
pub fn chat_result(value: Value) -> Result<ChatResult, BackendError> {
    crate::usage::begin();
    crate::usage::record(&value, crate::usage::Provider::OpenAi);
    #[derive(Deserialize)]
    struct Function {
        name: String,
        /// OpenAI sends the arguments as a JSON string; some servers send an object.
        #[serde(default)]
        arguments: Value,
    }
    #[derive(Deserialize)]
    struct Call {
        function: Function,
    }
    #[derive(Deserialize)]
    struct Message {
        /// `null` when the reply is only tool calls.
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        tool_calls: Vec<Call>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: Message,
    }
    #[derive(Deserialize)]
    struct Reply {
        choices: Vec<Choice>,
    }
    let timings = value
        .get("timings")
        .and_then(crate::ollama_inference::Timings::from_llama_cpp);
    let reply: Reply =
        serde_json::from_value(value).map_err(|_| BackendError("invalid chat response".into()))?;
    if reply.choices.is_empty() || reply.choices.len() > 8 {
        return Err(BackendError("invalid chat choice count".into()));
    }
    let message = reply
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| BackendError("no chat choice".into()))?
        .message;
    let content = message.content.unwrap_or_default();
    if content.len() > 1048576 {
        return Err(BackendError("generated content exceeds limit".into()));
    }
    if message.tool_calls.len() > 64 {
        return Err(BackendError("too many tool calls".into()));
    }
    let mut tool_calls = Vec::with_capacity(message.tool_calls.len());
    for call in message.tool_calls {
        let arguments = match call.function.arguments {
            Value::String(text) if text.trim().is_empty() => BTreeMap::new(),
            Value::String(text) => serde_json::from_str(&text)
                .map_err(|_| BackendError("tool call arguments are not a JSON object".into()))?,
            Value::Object(map) => map.into_iter().collect(),
            Value::Null => BTreeMap::new(),
            _ => {
                return Err(BackendError(
                    "tool call arguments are not a JSON object".into(),
                ));
            }
        };
        if call.function.name.is_empty() || call.function.name.len() > 128 {
            return Err(BackendError("invalid tool call name".into()));
        }
        tool_calls.push(ToolCall {
            name: call.function.name,
            arguments,
        });
    }
    Ok(ChatResult {
        content,
        tool_calls,
        timings,
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
                        slot_save_path: None,
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
        timeout: Duration,
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
            timeout,
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
        if input.tools.len() > 1024 {
            return Err(BackendError("too many tools".into()));
        }
        let messages = openai_messages(&input.messages);
        let mut body = json!({"model":model,"messages":messages,"stream":false});
        // Tools go to llama.cpp (its server handles them with the model's chat template); other
        // OpenAI-style servers here keep the plain chat they had.
        if !input.tools.is_empty() && handle.backend == "llamacpp" {
            body["tools"] = json!(
                input
                    .tools
                    .iter()
                    .map(|tool| json!({"type":"function","function":tool}))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(temperature) = input.temperature {
            if !temperature.is_finite() {
                return Err(BackendError("invalid temperature".into()));
            }
            body["temperature"] = json!(temperature);
        }
        if let Some(max_tokens) = input.max_tokens {
            if max_tokens == 0 {
                return Err(BackendError("max_tokens must be at least 1".into()));
            }
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(slot) = input.slot {
            if handle.backend != "llamacpp" || slot >= MAX_SLOTS {
                return Err(BackendError(
                    "a request slot applies only to llama.cpp, 0 to 15".into(),
                ));
            }
            body["id_slot"] = json!(slot);
            body["cache_prompt"] = json!(true);
        }
        let result = chat_result(
            self.json(handle, "/v1/chat/completions", body, CHAT_TIMEOUT, cancel)
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
                EMBED_TIMEOUT,
                cancel,
            )
            .await?,
            inputs.len(),
        )?;
        self.checked(handle, cancel).await?;
        Ok(result)
    }
}

/// Messages in OpenAI's shape. An assistant's tool calls get ids (`call_<message>_<n>`) and
/// each following `tool` message answers the next unanswered one, in order, so servers that
/// require `tool_call_id` accept the history.
pub fn openai_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let mut pending: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let mut value = json!({"role":message.role,"content":message.content});
            if !message.tool_calls.is_empty() {
                pending.clear();
                value["tool_calls"] = json!(
                    message
                        .tool_calls
                        .iter()
                        .enumerate()
                        .map(|(n, call)| {
                            let id = format!("call_{index}_{n}");
                            pending.push_back(id.clone());
                            json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": serde_json::to_string(&call.arguments)
                                        .unwrap_or_else(|_| "{}".into()),
                                },
                            })
                        })
                        .collect::<Vec<_>>()
                );
            }
            if message.role == "tool" {
                if let Some(id) = pending.pop_front() {
                    value["tool_call_id"] = json!(id);
                }
                if let Some(name) = &message.tool_name {
                    value["name"] = json!(name);
                }
            }
            value
        })
        .collect()
}

/// A chat reply may need to read a long prompt first, several requests at once sharing the
/// server, so it gets as long as the local harness allows a whole call (120 s).
pub const CHAT_TIMEOUT: Duration = Duration::from_secs(120);
/// Embeddings are short reads.
pub const EMBED_TIMEOUT: Duration = Duration::from_secs(30);

/// llama.cpp request slots (`--parallel`) never exceed 16.
pub const MAX_SLOTS: u32 = 16;

/// What to do with one llama.cpp request slot's KV cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvSlotAction {
    /// Write the slot's KV cache to `<name>.bin` in the server's slot directory.
    Save { name: String },
    /// Load `<name>.bin` into the slot.
    Restore { name: String },
    /// Drop the slot's KV cache, freeing it for other requests.
    Erase,
}

/// What llama.cpp reported for a slot action.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KvSlotReport {
    pub slot: u32,
    /// Tokens saved, restored or erased.
    pub tokens: u64,
    /// Bytes written or read (save and restore only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// How long the server took (save and restore only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<f64>,
}

/// A slot file name: 1 to 64 ASCII letters, digits, `-` or `_`; `.bin` is added.
pub fn kv_slot_file(name: &str) -> Result<String, BackendError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(BackendError(
            "KV slot names are 1 to 64 letters, digits, - or _".into(),
        ));
    }
    Ok(format!("{name}.bin"))
}

pub fn kv_slot_report(
    slot: u32,
    action: &KvSlotAction,
    value: &Value,
) -> Result<KvSlotReport, BackendError> {
    let invalid = || BackendError("invalid KV slot response".into());
    let count = |key: &str| value.get(key).and_then(Value::as_u64);
    let (tokens, bytes, ms) = match action {
        KvSlotAction::Save { .. } => (
            count("n_saved").ok_or_else(invalid)?,
            count("n_written"),
            value.pointer("/timings/save_ms").and_then(Value::as_f64),
        ),
        KvSlotAction::Restore { .. } => (
            count("n_restored").ok_or_else(invalid)?,
            count("n_read"),
            value.pointer("/timings/restore_ms").and_then(Value::as_f64),
        ),
        KvSlotAction::Erase => (count("n_erased").ok_or_else(invalid)?, None, None),
    };
    if value.get("id_slot").and_then(Value::as_u64) != Some(u64::from(slot))
        || ms.is_some_and(|ms| !ms.is_finite() || ms < 0.0)
    {
        return Err(invalid());
    }
    Ok(KvSlotReport {
        slot,
        tokens,
        bytes,
        ms,
    })
}

impl OpenAiInference<'_> {
    /// Saves, restores or erases one request slot's KV cache on a llama.cpp server that
    /// rigspark started with a slot directory. The listener is re-identified before and
    /// after, as for chat.
    pub async fn kv_slot(
        &self,
        handle: &ServerState,
        slot: u32,
        action: &KvSlotAction,
        cancel: &CancellationToken,
    ) -> Result<KvSlotReport, BackendError> {
        if handle.backend != "llamacpp" || slot >= MAX_SLOTS {
            return Err(BackendError(
                "KV slots apply only to llama.cpp, slots 0 to 15".into(),
            ));
        }
        let (query, body) = match action {
            KvSlotAction::Save { name } => ("save", json!({"filename": kv_slot_file(name)?})),
            KvSlotAction::Restore { name } => ("restore", json!({"filename": kv_slot_file(name)?})),
            KvSlotAction::Erase => ("erase", json!({})),
        };
        self.checked(handle, cancel).await?;
        let request = Request::new(
            &handle.endpoint,
            &format!("/slots/{slot}"),
            Some(body),
            None,
        )
        .and_then(|request| request.with_query(&[("action", query)]))
        .map_err(|error| BackendError(error.to_string()))?;
        let value = tokio::time::timeout(
            Duration::from_secs(120),
            read_json(self.http, request, cancel, 64 * 1024),
        )
        .await
        .map_err(|_| BackendError("KV slot request timed out".into()))?
        .map_err(|error| BackendError(error.to_string()))?;
        let report = kv_slot_report(slot, action, &value)?;
        self.checked(handle, cancel).await?;
        Ok(report)
    }
}
