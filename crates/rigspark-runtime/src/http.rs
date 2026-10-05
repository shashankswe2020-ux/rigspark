use futures_util::TryStreamExt;
use serde_json::Value;
use std::{pin::Pin, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::{io::StreamReader, sync::CancellationToken};

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("invalid loopback HTTP request")]
    Invalid,
    #[error("local runtime HTTP {0}")]
    Status(u16),
    #[error("local runtime transport failed")]
    Transport,
    #[error("local runtime response exceeds limit")]
    Limit,
    #[error("invalid local runtime response")]
    Response,
    #[error("operation cancelled")]
    Cancelled,
    #[error("local runtime request timed out")]
    Timeout,
}
pub struct Request {
    pub url: url::Url,
    body: Option<Value>,
    token: Option<String>,
}
impl Request {
    pub fn new(
        endpoint: &str,
        path: &str,
        body: Option<Value>,
        token: Option<String>,
    ) -> Result<Self, HttpError> {
        let mut url = crate::state::loopback(endpoint).map_err(|_| HttpError::Invalid)?;
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.split('/').any(|part| part == ".." || part == ".")
            || path
                .bytes()
                .any(|byte| byte.is_ascii_control() || b"\\%?#".contains(&byte))
            || path.len() > 2048
            || token.as_ref().is_some_and(|value| {
                value.is_empty()
                    || value.len() > 4096
                    || value.bytes().any(|byte| !(0x20..=0x7e).contains(&byte))
            })
        {
            return Err(HttpError::Invalid);
        }
        url.set_path(path);
        url.set_query(None);
        url.set_fragment(None);
        if body.as_ref().is_some_and(|value| {
            serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > 16 * 1024 * 1024)
        }) {
            return Err(HttpError::Limit);
        }
        Ok(Self { url, body, token })
    }
    pub fn body(&self) -> Option<&Value> {
        self.body.as_ref()
    }
    /// Adds percent-encoded query pairs; keys are ASCII identifiers, values bounded text.
    pub fn with_query(mut self, pairs: &[(&str, &str)]) -> Result<Self, HttpError> {
        if pairs.is_empty()
            || pairs.len() > 16
            || pairs.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 64
                    || !key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    || value.len() > 1024
                    || value.chars().any(char::is_control)
            })
        {
            return Err(HttpError::Invalid);
        }
        self.url.query_pairs_mut().clear().extend_pairs(pairs);
        Ok(self)
    }
}
pub struct Response {
    pub status: u16,
    pub body: Pin<Box<dyn AsyncRead + Send>>,
}
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, request: Request) -> Result<Response, HttpError>;
}
pub struct NativeTransport {
    client: reqwest::Client,
}
impl NativeTransport {
    pub fn new() -> Result<Self, HttpError> {
        let addresses = [
            std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 0)),
        ];
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs("localhost", &addresses)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(1800))
            .build()
            .map_err(|_| HttpError::Transport)?;
        Ok(Self { client })
    }
}
#[async_trait::async_trait]
impl Transport for NativeTransport {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        crate::state::loopback(request.url.as_str()).map_err(|_| HttpError::Invalid)?;
        let mut builder = if let Some(body) = request.body {
            self.client.post(request.url).json(&body)
        } else {
            self.client.get(request.url)
        };
        if let Some(token) = request.token {
            builder = builder.bearer_auth(token);
        }
        let response = builder.send().await.map_err(|_| HttpError::Transport)?;
        let status = response.status().as_u16();
        Ok(Response {
            status,
            body: Box::pin(StreamReader::new(
                response.bytes_stream().map_err(std::io::Error::other),
            )),
        })
    }
}
pub async fn read_json(
    transport: &dyn Transport,
    request: Request,
    cancel: &CancellationToken,
    maximum: usize,
) -> Result<Value, HttpError> {
    if maximum == 0 || maximum > 16 * 1024 * 1024 {
        return Err(HttpError::Limit);
    }
    if cancel.is_cancelled() {
        return Err(HttpError::Cancelled);
    }
    let operation = async {
        let response = transport.send(request).await?;
        if !(200..300).contains(&response.status) {
            return Err(HttpError::Status(response.status));
        }
        let mut bytes = Vec::new();
        response
            .body
            .take(maximum as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| HttpError::Transport)?;
        if bytes.len() > maximum {
            return Err(HttpError::Limit);
        }
        serde_json::from_slice(&bytes).map_err(|_| HttpError::Response)
    };
    tokio::select! { biased; _=cancel.cancelled()=>Err(HttpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(120),operation)=>result.map_err(|_|HttpError::Timeout)? }
}
