use crate::{
    http::{HttpError, Request, Transport, read_json},
    identity::{ProcessIdentity, ProcessProbe},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    pub tool_name: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatInput {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    pub temperature: Option<f64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResult {
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}
#[derive(Debug, Serialize)]
pub struct EmbedResult {
    pub vectors: Vec<Vec<f64>>,
    pub dimension: usize,
}
fn model_id(value: &str) -> Result<(), HttpError> {
    if value.is_empty()
        || value.len() > 8192
        || !value.as_bytes()[0].is_ascii_lowercase() && !value.as_bytes()[0].is_ascii_digit()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:/-".contains(&byte)
        })
        || value.split('/').any(|part| part == "..")
    {
        return Err(HttpError::Invalid);
    }
    Ok(())
}
fn chat_body(input: &ChatInput, stream: bool) -> Result<Value, HttpError> {
    model_id(&input.model)?;
    if input.messages.len() > 10000
        || input.tools.len() > 1024
        || input.temperature.is_some_and(|value| !value.is_finite())
    {
        return Err(HttpError::Invalid);
    }
    let mut messages = Vec::new();
    for message in &input.messages {
        if !["system", "user", "assistant", "tool"].contains(&message.role.as_str()) {
            return Err(HttpError::Invalid);
        }
        let mut value = json!({"role":message.role,"content":message.content});
        if !message.tool_calls.is_empty() {
            value["tool_calls"] = json!(
                message
                    .tool_calls
                    .iter()
                    .map(|call| json!({"function":{"name":call.name,"arguments":call.arguments}}))
                    .collect::<Vec<_>>()
            );
        }
        if let Some(name) = &message.tool_name {
            value["tool_name"] = json!(name);
        }
        messages.push(value);
    }
    let mut body = json!({"model":input.model,"messages":messages,"stream":stream});
    if !input.tools.is_empty() {
        body["tools"] = json!(
            input
                .tools
                .iter()
                .map(|tool| json!({"type":"function","function":tool}))
                .collect::<Vec<_>>()
        );
    }
    if let Some(temperature) = input.temperature {
        body["options"] = json!({"temperature":temperature});
    }
    Ok(body)
}
/// Ollama's wire shape: newer releases add `id` and `function.index`, which are ignored.
#[derive(Deserialize)]
struct NativeFunction {
    name: String,
    #[serde(default)]
    arguments: BTreeMap<String, Value>,
}
#[derive(Deserialize)]
struct NativeCall {
    function: NativeFunction,
}
impl From<NativeCall> for ToolCall {
    fn from(call: NativeCall) -> Self {
        Self {
            name: call.function.name,
            arguments: call.function.arguments,
        }
    }
}
#[derive(Deserialize)]
struct NativeMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<NativeCall>,
}
#[derive(Deserialize)]
struct NativeChunk {
    message: Option<NativeMessage>,
    #[serde(rename = "done")]
    _done: Option<bool>,
}
fn decode_result(value: Value) -> Result<ChatResult, HttpError> {
    crate::usage::record(&value, crate::usage::Provider::Ollama);
    #[derive(Deserialize)]
    struct Reply {
        message: NativeMessage,
    }
    if value.get("error").is_some()
        || !value
            .get("message")
            .and_then(|message| message.get("content"))
            .is_some_and(Value::is_string)
    {
        return Err(HttpError::Response);
    }
    let response: Reply = serde_json::from_value(value).map_err(|_| HttpError::Response)?;
    Ok(ChatResult {
        content: response.message.content,
        tool_calls: response
            .message
            .tool_calls
            .into_iter()
            .map(ToolCall::from)
            .collect(),
    })
}
pub struct OllamaInference<'runtime> {
    transport: &'runtime dyn Transport,
    probe: &'runtime dyn ProcessProbe,
    executable: &'runtime str,
}
impl<'runtime> OllamaInference<'runtime> {
    pub fn new(
        transport: &'runtime dyn Transport,
        probe: &'runtime dyn ProcessProbe,
        executable: &'runtime str,
    ) -> Self {
        Self {
            transport,
            probe,
            executable,
        }
    }
    async fn check(
        &self,
        endpoint: &str,
        expected: &ProcessIdentity,
        cancel: &CancellationToken,
    ) -> Result<(), HttpError> {
        let url = crate::state::loopback(endpoint).map_err(|_| HttpError::Invalid)?;
        if expected.pid == 0
            || expected.executable != self.executable
            || expected.started.is_empty()
            || !std::path::Path::new(self.executable).is_absolute()
        {
            return Err(HttpError::Invalid);
        }
        let port = url.port_or_known_default().ok_or(HttpError::Invalid)?;
        let host = url.host_str().ok_or(HttpError::Invalid)?;
        let listener = crate::process_control::listener(self.probe, endpoint, cancel)
            .await
            .map_err(|_| {
                if cancel.is_cancelled() {
                    HttpError::Cancelled
                } else {
                    HttpError::Invalid
                }
            })?;
        crate::identity::choose_listener(std::slice::from_ref(&listener), port, host)
            .map_err(|_| HttpError::Invalid)?;
        if listener.identity.pid != expected.pid
            || listener.identity.executable != expected.executable
            || listener.identity.started != expected.started
        {
            return Err(HttpError::Invalid);
        }
        Ok(())
    }
    async fn trusted(
        &self,
        endpoint: &str,
        expected: &ProcessIdentity,
        cancel: &CancellationToken,
    ) -> Result<(), HttpError> {
        if cancel.is_cancelled() {
            return Err(HttpError::Cancelled);
        }
        self.check(endpoint, expected, cancel).await?;
        let version = read_json(
            self.transport,
            Request::new(endpoint, "/api/version", None, None)?,
            cancel,
            4096,
        )
        .await?;
        if !version
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= 100
                    && value.bytes().all(|byte| !byte.is_ascii_control())
            })
        {
            return Err(HttpError::Response);
        }
        self.check(endpoint, expected, cancel).await
    }
    pub async fn chat(
        &self,
        endpoint: &str,
        expected: &ProcessIdentity,
        input: &ChatInput,
        cancel: &CancellationToken,
    ) -> Result<ChatResult, HttpError> {
        let request = Request::new(endpoint, "/api/chat", Some(chat_body(input, false)?), None)?;
        crate::usage::begin();
        self.trusted(endpoint, expected, cancel).await?;
        let result =
            decode_result(read_json(self.transport, request, cancel, 16 * 1024 * 1024).await?)?;
        self.check(endpoint, expected, cancel).await?;
        Ok(result)
    }
    pub async fn chat_stream(
        &self,
        endpoint: &str,
        expected: &ProcessIdentity,
        input: &ChatInput,
        cancel: &CancellationToken,
        on_delta: &mut (dyn FnMut(&str) + Send),
    ) -> Result<ChatResult, HttpError> {
        let request = Request::new(endpoint, "/api/chat", Some(chat_body(input, true)?), None)?;
        crate::usage::begin();
        self.trusted(endpoint, expected, cancel).await?;
        let operation = async {
            let response = self.transport.send(request).await?;
            if !(200..300).contains(&response.status) {
                return Err(HttpError::Status(response.status));
            }
            let mut reader = BufReader::new(response.body);
            let mut result = ChatResult {
                content: String::new(),
                tool_calls: Vec::new(),
            };
            let mut total = 0usize;
            loop {
                let mut line = Vec::new();
                let count = (&mut reader)
                    .take(65537)
                    .read_until(b'\n', &mut line)
                    .await
                    .map_err(|_| HttpError::Transport)?;
                if count == 0 {
                    break;
                }
                total += count;
                if count > 65536 || total > 16 * 1024 * 1024 {
                    return Err(HttpError::Limit);
                }
                let terminated = line.last() == Some(&b'\n');
                let records: Vec<_> = line
                    .split(|byte| *byte == b'\r' || *byte == b'\n')
                    .collect();
                for record in records
                    .iter()
                    .take(records.len().saturating_sub(usize::from(!terminated)))
                {
                    let Ok(chunk) = serde_json::from_slice::<NativeChunk>(record) else {
                        continue;
                    };
                    if let Ok(value) = serde_json::from_slice::<Value>(record) {
                        crate::usage::record(&value, crate::usage::Provider::Ollama);
                    }
                    if let Some(message) = chunk.message {
                        if !message.content.is_empty() {
                            on_delta(&message.content);
                            result.content.push_str(&message.content);
                        }
                        result
                            .tool_calls
                            .extend(message.tool_calls.into_iter().map(ToolCall::from));
                    }
                }
            }
            Ok(result)
        };
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(HttpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(1800),operation)=>result.map_err(|_|HttpError::Timeout)?}?;
        self.check(endpoint, expected, cancel).await?;
        Ok(result)
    }
    pub async fn embed(
        &self,
        endpoint: &str,
        expected: &ProcessIdentity,
        model: &str,
        inputs: &[String],
        cancel: &CancellationToken,
    ) -> Result<EmbedResult, HttpError> {
        model_id(model)?;
        if inputs.is_empty()
            || inputs.len() > 1024
            || inputs
                .iter()
                .any(|input| input.is_empty() || input.len() > 1024 * 1024)
            || inputs.iter().map(String::len).sum::<usize>() > 4 * 1024 * 1024
        {
            return Err(HttpError::Invalid);
        }
        let request = Request::new(
            endpoint,
            "/api/embed",
            Some(json!({"model":model,"input":inputs})),
            None,
        )?;
        self.trusted(endpoint, expected, cancel).await?;
        let value = tokio::time::timeout(
            Duration::from_secs(30),
            read_json(self.transport, request, cancel, 16 * 1024 * 1024),
        )
        .await
        .map_err(|_| HttpError::Timeout)??;
        #[derive(Deserialize)]
        struct Embeddings {
            embeddings: Vec<Vec<f64>>,
        }
        let vectors = serde_json::from_value::<Embeddings>(value)
            .map_err(|_| HttpError::Response)?
            .embeddings;
        let dimension = vectors.first().map(Vec::len).ok_or(HttpError::Response)?;
        if vectors.len() != inputs.len()
            || dimension == 0
            || dimension > 8192
            || vectors.len() * dimension > 1000000
            || vectors.iter().any(|vector| {
                vector.len() != dimension || vector.iter().any(|value| !value.is_finite())
            })
        {
            return Err(HttpError::Response);
        }
        self.check(endpoint, expected, cancel).await?;
        Ok(EmbedResult { vectors, dimension })
    }
}

/// Loads `model` in Ollama with a one-token generation so a runtime that cannot run it
/// (for example an unknown architecture) fails now, with Ollama's reason, rather than on
/// the first chat. The reason is sanitized and bounded.
pub async fn preload(
    transport: &dyn Transport,
    endpoint: &str,
    model: &str,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let request = Request::new(
        endpoint,
        "/api/generate",
        Some(serde_json::json!({
            "model": model,
            "prompt": "Hi",
            "stream": false,
            "options": {"num_predict": 1},
            "keep_alive": "30s",
        })),
        None,
    )
    .map_err(|error| error.to_string())?;
    let operation = async {
        let response = transport
            .send(request)
            .await
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        response
            .body
            .take(64 * 1024)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "unreadable Ollama response".to_owned())?;
        if (200..300).contains(&response.status) {
            return Ok(());
        }
        let reason = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .map(|text| {
                rigspark_core::reports::strip_control(&text)
                    .chars()
                    .take(400)
                    .collect::<String>()
            })
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| format!("Ollama returned HTTP {}", response.status));
        Err(reason)
    };
    tokio::select! {biased;_=cancel.cancelled()=>Err("cancelled".into()),result=tokio::time::timeout(Duration::from_secs(600),operation)=>result.map_err(|_|"loading the model timed out".to_owned())?}
}
