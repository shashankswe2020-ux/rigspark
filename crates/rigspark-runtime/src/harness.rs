use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, pin::Pin, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("invalid harness input or configuration")]
    Invalid,
    #[error("provider is unavailable")]
    Unavailable,
    #[error("provider transport failed")]
    Transport,
    #[error("provider returned HTTP {0}")]
    Status(u16),
    #[error("invalid provider response")]
    Response,
    #[error("harness output exceeds limit")]
    Limit,
    #[error("harness request cancelled")]
    Cancelled,
    #[error("harness request timed out")]
    Timeout,
    #[error("active runtime changed")]
    Drift,
}
pub struct Secret(String);
impl Secret {
    pub fn new(raw: &str) -> Result<Self, HarnessError> {
        let value = raw.trim();
        if value.is_empty()
            || value.len() > 4096
            || value.bytes().any(|byte| !(32..=126).contains(&byte))
        {
            return Err(HarnessError::Invalid);
        }
        Ok(Self(value.into()))
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessMessage {
    pub role: String,
    pub content: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessRequest {
    pub model: String,
    pub messages: Vec<HarnessMessage>,
    pub temperature: Option<f64>,
}
impl HarnessRequest {
    pub fn validate(&self) -> Result<(), HarnessError> {
        if self.model.len() > 8192
            || self.model.chars().any(char::is_control)
            || self.messages.len() > 10000
            || self
                .messages
                .iter()
                .any(|message| !["system", "user", "assistant"].contains(&message.role.as_str()))
            || self
                .messages
                .iter()
                .map(|message| message.content.len())
                .sum::<usize>()
                > 4 * 1024 * 1024
            || self.temperature.is_some_and(|value| !value.is_finite())
        {
            return Err(HarnessError::Invalid);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Claude,
    Compatible,
}
pub struct RemoteRequest {
    pub url: url::Url,
    pub body: Value,
    pub headers: BTreeMap<String, String>,
}
pub struct RemoteResponse {
    pub status: u16,
    pub body: Pin<Box<dyn AsyncRead + Send>>,
}
#[async_trait::async_trait]
pub trait RemoteTransport: Send + Sync {
    async fn send(&self, request: RemoteRequest) -> Result<RemoteResponse, HarnessError>;
}
pub struct NativeRemoteTransport {
    client: reqwest::Client,
}
struct Resolver;
impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            if name.as_str() == "localhost" {
                return Ok(Box::new(
                    [
                        std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                        std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 0)),
                    ]
                    .into_iter(),
                ) as reqwest::dns::Addrs);
            }
            let addresses = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((name.as_str(), 0)),
            )
            .await
            .map_err(|_| std::io::Error::other("DNS timeout"))??;
            let addresses = crate::acquire::public_addresses(addresses.take(65).collect())
                .map_err(std::io::Error::other)?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}
impl NativeRemoteTransport {
    pub fn new() -> Result<Self, HarnessError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .dns_resolver(std::sync::Arc::new(Resolver))
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|_| HarnessError::Transport)?,
        })
    }
}
pub fn external_url(raw: &str) -> Result<url::Url, HarnessError> {
    if raw.len() > 2048 || raw.chars().any(char::is_control) {
        return Err(HarnessError::Invalid);
    }
    let url = url::Url::parse(raw).map_err(|_| HarnessError::Invalid)?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(HarnessError::Invalid);
    }
    let local = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain == "localhost",
        None => return Err(HarnessError::Invalid),
    };
    if (url.scheme() != "https" && !(local && url.scheme() == "http"))
        || (!local
            && url.host().is_some_and(|host| match host {
                url::Host::Ipv4(address) => {
                    crate::acquire::public_addresses(vec![std::net::SocketAddr::new(
                        address.into(),
                        443,
                    )])
                    .is_err()
                }
                url::Host::Ipv6(address) => {
                    crate::acquire::public_addresses(vec![std::net::SocketAddr::new(
                        address.into(),
                        443,
                    )])
                    .is_err()
                }
                _ => false,
            }))
    {
        return Err(HarnessError::Invalid);
    }
    Ok(url)
}
#[async_trait::async_trait]
impl RemoteTransport for NativeRemoteTransport {
    async fn send(&self, request: RemoteRequest) -> Result<RemoteResponse, HarnessError> {
        external_url(request.url.as_str())?;
        let mut builder = self.client.post(request.url).json(&request.body);
        for (name, value) in request.headers {
            let mut header = reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| HarnessError::Invalid)?;
            header.set_sensitive(true);
            builder = builder.header(name, header);
        }
        let response = builder.send().await.map_err(|_| HarnessError::Transport)?;
        let status = response.status().as_u16();
        Ok(RemoteResponse {
            status,
            body: Box::pin(tokio_util::io::StreamReader::new(
                response
                    .bytes_stream()
                    .map_err(|_| std::io::Error::other("provider body failed")),
            )),
        })
    }
}
pub type DeltaSink<'sink> =
    dyn for<'text> FnMut(&'text str) -> Result<(), HarnessError> + Send + 'sink;
#[async_trait::async_trait]
pub trait ChatHarness: Send + Sync {
    fn name(&self) -> &'static str;
    async fn available(&self) -> bool;
    async fn chat(
        &self,
        request: &HarnessRequest,
        cancel: &CancellationToken,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError>;
}
pub struct RemoteHarness<'runtime> {
    provider: Provider,
    url: url::Url,
    key: Option<Secret>,
    transport: &'runtime dyn RemoteTransport,
    maximum: usize,
}
impl<'runtime> RemoteHarness<'runtime> {
    pub fn new(
        provider: Provider,
        endpoint: &str,
        key: Option<Secret>,
        transport: &'runtime dyn RemoteTransport,
    ) -> Result<Self, HarnessError> {
        let url = external_url(endpoint)?;
        let required = match provider {
            Provider::OpenAi => Some("api.openai.com"),
            Provider::Claude => Some("api.anthropic.com"),
            Provider::Compatible => None,
        };
        if required.is_some_and(|host| url.scheme() != "https" || url.host_str() != Some(host))
            || required.is_some() && key.is_none()
        {
            return Err(HarnessError::Invalid);
        }
        Ok(Self {
            provider,
            url,
            key,
            transport,
            maximum: 16 * 1024 * 1024,
        })
    }
    pub fn with_limit(mut self, maximum: usize) -> Result<Self, HarnessError> {
        if maximum == 0 || maximum > 16 * 1024 * 1024 {
            return Err(HarnessError::Invalid);
        }
        self.maximum = maximum;
        Ok(self)
    }
    pub async fn chat(
        &self,
        input: &HarnessRequest,
        cancel: &CancellationToken,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        input.validate()?;
        if cancel.is_cancelled() {
            return Err(HarnessError::Cancelled);
        }
        let model = if input.model.is_empty() {
            match self.provider {
                Provider::OpenAi => "gpt-4o-mini",
                Provider::Claude => "claude-3-5-haiku-20241022",
                Provider::Compatible => "local-model",
            }
        } else {
            &input.model
        };
        let messages:Vec<_>=input.messages.iter().filter(|message|self.provider!=Provider::Claude||message.role!="system").map(|message|json!({"role":message.role,"content":rigspark_core::reports::strip_control(&message.content)})).collect();
        let mut body = json!({"model":model,"messages":messages,"stream":true});
        crate::usage::begin();
        if self.provider != Provider::Claude {
            body["stream_options"] = json!({"include_usage":true});
        }
        if self.provider == Provider::Claude {
            body["max_tokens"] = json!(1024);
        }
        if let Some(temperature) = input.temperature {
            body["temperature"] = json!(temperature);
        }
        let mut headers = BTreeMap::new();
        if let Some(key) = &self.key {
            if self.provider == Provider::Claude {
                headers.insert("x-api-key".into(), key.expose().into());
                headers.insert("anthropic-version".into(), "2023-06-01".into());
            } else {
                headers.insert("authorization".into(), format!("Bearer {}", key.expose()));
            }
        }
        let operation = async {
            let response = self
                .transport
                .send(RemoteRequest {
                    url: self.url.clone(),
                    body,
                    headers,
                })
                .await?;
            if !(200..300).contains(&response.status) {
                return Err(HarnessError::Status(response.status));
            }
            let mut output = String::new();
            let mut redactor = crate::redaction::StreamRedactor::new(
                self.key.iter().map(|key| key.expose().to_owned()),
            );
            let mut done = false;
            consume_sse(response.body, self.maximum, &mut |frame| {
                if done {
                    return Ok(());
                }
                if frame_is_done(frame) {
                    done = true;
                    return Ok(());
                }
                let text = redactor.push(&parse_frame(self.provider, frame)?);
                if output.len() + text.len() > self.maximum {
                    return Err(HarnessError::Limit);
                }
                if !text.is_empty() {
                    on_delta(&text)?;
                    output.push_str(&text);
                }
                Ok(())
            })
            .await?;
            let pending = redactor.finish();
            if output.len() + pending.len() > self.maximum {
                return Err(HarnessError::Limit);
            }
            if !pending.is_empty() {
                on_delta(&pending)?;
                output.push_str(&pending);
            }
            Ok(output)
        };
        tokio::select! {biased;_=cancel.cancelled()=>Err(HarnessError::Cancelled),result=tokio::time::timeout(Duration::from_secs(120),operation)=>result.map_err(|_|HarnessError::Timeout)?}
    }
}
#[async_trait::async_trait]
impl ChatHarness for RemoteHarness<'_> {
    fn name(&self) -> &'static str {
        match self.provider {
            Provider::OpenAi => "openai",
            Provider::Claude => "claude",
            Provider::Compatible => "openai-compatible",
        }
    }
    async fn available(&self) -> bool {
        self.provider == Provider::Compatible || self.key.is_some()
    }
    async fn chat(
        &self,
        request: &HarnessRequest,
        cancel: &CancellationToken,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        RemoteHarness::chat(self, request, cancel, on_delta).await
    }
}
pub async fn consume_sse(
    mut body: Pin<Box<dyn AsyncRead + Send>>,
    maximum: usize,
    on_frame: &mut (dyn FnMut(&str) -> Result<(), HarnessError> + Send),
) -> Result<(), HarnessError> {
    let mut bytes = Vec::new();
    let mut buffer = vec![0u8; 8192];
    let mut total = 0;
    let mut frame = String::new();
    loop {
        let count = body
            .read(&mut buffer)
            .await
            .map_err(|_| HarnessError::Transport)?;
        if count == 0 {
            break;
        }
        total += count;
        if total > maximum {
            return Err(HarnessError::Limit);
        }
        bytes.extend_from_slice(&buffer[..count]);
        while let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
            let line = std::str::from_utf8(&bytes[..end])
                .map_err(|_| HarnessError::Response)?
                .trim_end_matches('\r');
            if line.is_empty() {
                if !frame.is_empty() {
                    on_frame(&frame)?;
                    frame.clear();
                }
            } else {
                frame.push_str(line);
                frame.push('\n');
            }
            bytes.drain(..=end);
            if frame.len() > 1048576 {
                return Err(HarnessError::Limit);
            }
        }
        if bytes.len() > 1048576 {
            return Err(HarnessError::Limit);
        }
    }
    if !bytes.is_empty() {
        frame.push_str(std::str::from_utf8(&bytes).map_err(|_| HarnessError::Response)?);
    }
    if !frame.is_empty() {
        on_frame(&frame)?;
    }
    Ok(())
}
fn frame_data(frame: &str) -> String {
    frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .collect::<Vec<_>>()
        .join("\n")
}
fn frame_is_done(frame: &str) -> bool {
    frame_data(frame).trim() == "[DONE]"
}
fn parse_frame(provider: Provider, frame: &str) -> Result<String, HarnessError> {
    let raw = frame_data(frame);
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(String::new());
    }
    let value: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) if provider == Provider::Claude => return Ok(String::new()),
        Err(_) => return Err(HarnessError::Response),
    };
    crate::usage::record(
        &value,
        if provider == Provider::Claude {
            crate::usage::Provider::Claude
        } else {
            crate::usage::Provider::OpenAi
        },
    );
    let mut result = String::new();
    if provider == Provider::Claude {
        if value["delta"]["type"] == "text_delta" {
            if let Some(text) = value["delta"]["text"].as_str() {
                result.push_str(text);
            }
        } else if let Some(parts) = value["message"]["content"].as_array() {
            for part in parts {
                if part["type"] == "text"
                    && let Some(text) = part["text"].as_str()
                {
                    result.push_str(text);
                }
            }
        }
    } else {
        let choices = value["choices"].as_array().ok_or(HarnessError::Response)?;
        for choice in choices {
            let content = &choice["delta"]["content"];
            if let Some(text) = content.as_str() {
                result.push_str(text);
            } else if let Some(parts) = content.as_array() {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        result.push_str(text);
                    }
                }
            }
        }
    }
    Ok(rigspark_core::reports::strip_control(&result))
}
pub struct LocalHarness<'runtime> {
    pub state: &'runtime crate::state::StateStore,
    pub registry: &'runtime crate::lifecycle::Registry<'runtime>,
    pub probe: &'runtime dyn crate::identity::ProcessProbe,
}
pub struct BoundLocalAgent<'bound, 'runtime> {
    local: &'bound LocalHarness<'runtime>,
    expected: crate::state::RuntimeState,
}
impl LocalHarness<'_> {
    pub fn bind(&self) -> Result<BoundLocalAgent<'_, '_>, crate::memory::MemoryError> {
        Ok(BoundLocalAgent {
            local: self,
            expected: self.state.read()?,
        })
    }
}
#[async_trait::async_trait]
impl crate::agent::AgentChat for BoundLocalAgent<'_, '_> {
    async fn chat(
        &self,
        input: &crate::ollama_inference::ChatInput,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<crate::ollama_inference::ChatResult, HarnessError> {
        if self.local.state.read().map_err(|_| HarnessError::Drift)? != self.expected {
            return Err(HarnessError::Drift);
        }
        let result = crate::agent::AgentChat::chat(self.local, input, cancel, sink).await?;
        if self.local.state.read().map_err(|_| HarnessError::Drift)? != self.expected {
            return Err(HarnessError::Drift);
        }
        Ok(result)
    }
}
#[async_trait::async_trait]
impl ChatHarness for LocalHarness<'_> {
    fn name(&self) -> &'static str {
        "local"
    }
    async fn available(&self) -> bool {
        self.state.read().is_ok_and(|state| state.active.is_some())
    }
    async fn chat(
        &self,
        input: &HarnessRequest,
        cancel: &CancellationToken,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        input.validate()?;
        let request = crate::ollama_inference::ChatInput {
            model: input.model.clone(),
            messages: input
                .messages
                .iter()
                .map(|message| crate::ollama_inference::ChatMessage {
                    role: message.role.clone(),
                    content: message.content.clone(),
                    tool_calls: Vec::new(),
                    tool_name: None,
                })
                .collect(),
            tools: Vec::new(),
            temperature: input.temperature,
            slot: None,
            max_tokens: None,
        };
        Ok(
            crate::agent::AgentChat::chat(self, &request, cancel, on_delta)
                .await?
                .content,
        )
    }
}
#[async_trait::async_trait]
impl crate::agent::AgentChat for LocalHarness<'_> {
    async fn chat(
        &self,
        input: &crate::ollama_inference::ChatInput,
        cancel: &CancellationToken,
        on_delta: &mut DeltaSink<'_>,
    ) -> Result<crate::ollama_inference::ChatResult, HarnessError> {
        if input.messages.len() > 10000
            || input.tools.len() > 1024
            || input.temperature.is_some_and(|value| !value.is_finite())
            || serde_json::to_vec(input)
                .map_err(|_| HarnessError::Invalid)?
                .len()
                > 4 * 1024 * 1024
        {
            return Err(HarnessError::Invalid);
        }
        let prior = self.state.read().map_err(|_| HarnessError::Unavailable)?;
        let mut active = prior.active.clone().ok_or(HarnessError::Unavailable)?;
        let adapter = self
            .registry
            .get(&active.backend)
            .map_err(|_| HarnessError::Unavailable)?;
        let observed = crate::process_control::listener(self.probe, &active.endpoint, cancel)
            .await
            .map_err(|_| HarnessError::Drift)?;
        let live = crate::identity::capture(&active, &observed, adapter.trusts(&observed.identity))
            .map_err(|_| HarnessError::Drift)?;
        active.pid = Some(live.expected.pid);
        active.process_executable = Some(live.expected.executable);
        active.process_started_at = Some(live.expected.started);
        let mut request = input.clone();
        if request.model.is_empty() {
            request.model = active
                .runtime_model_id
                .clone()
                .unwrap_or_else(|| active.model_id.clone());
        }
        let stream_cancel = cancel.child_token();
        let mut emitted = 0usize;
        let mut sink_error = None;
        let mut emit = |text: &str| {
            if sink_error.is_some() || stream_cancel.is_cancelled() {
                return;
            }
            emitted = emitted.saturating_add(text.len());
            let result = if emitted > 16 * 1024 * 1024 {
                Err(HarnessError::Limit)
            } else {
                on_delta(text)
            };
            if let Err(error) = result {
                sink_error = Some(error);
                stream_cancel.cancel();
            }
        };
        let response = tokio::select! {biased;_=cancel.cancelled()=>return Err(HarnessError::Cancelled),result=tokio::time::timeout(Duration::from_secs(120),adapter.chat_stream(&active,&request,&stream_cancel,&mut emit))=>result};
        if let Some(error) = sink_error {
            return Err(error);
        }
        let response = response
            .map_err(|_| HarnessError::Timeout)?
            .map_err(|_| HarnessError::Transport)?;
        if self.state.read().map_err(|_| HarnessError::Drift)? != prior {
            return Err(HarnessError::Drift);
        }
        if response.content.len() > 16 * 1024 * 1024 {
            return Err(HarnessError::Limit);
        }
        Ok(response)
    }
}
