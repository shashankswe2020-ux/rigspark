use crate::mcp::{ClientFactory, Connection, Connector, McpError, Tool, ToolResult, loopback_url};
use crate::mcp_http::BoundedHttp;
use rmcp::{RoleClient, ServiceExt, service::RunningService};
use std::{process::Stdio, time::Duration};
use tokio::io::AsyncRead;
use tokio_util::sync::CancellationToken;
pub struct BoundedLines<Reader> {
    inner: Reader,
    line: usize,
    total: usize,
}
impl<Reader> BoundedLines<Reader> {
    pub fn new(inner: Reader) -> Self {
        Self {
            inner,
            line: 0,
            total: 0,
        }
    }
}
impl<Reader: AsyncRead + Unpin> AsyncRead for BoundedLines<Reader> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        output: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let mut bytes = [0u8; 8192];
        let length = output.remaining().min(bytes.len());
        if length == 0 {
            return std::task::Poll::Ready(Ok(()));
        }
        let mut buffer = tokio::io::ReadBuf::new(&mut bytes[..length]);
        match std::pin::Pin::new(&mut this.inner).poll_read(context, &mut buffer) {
            std::task::Poll::Ready(Ok(())) => {
                for byte in buffer.filled() {
                    this.total += 1;
                    if *byte == b'\n' {
                        this.line = 0;
                    } else {
                        this.line += 1;
                    }
                    if this.line > 1048576 || this.total > 64 * 1024 * 1024 {
                        return std::task::Poll::Ready(Err(std::io::Error::other(
                            "MCP input limit",
                        )));
                    }
                }
                output.put_slice(buffer.filled());
                std::task::Poll::Ready(Ok(()))
            }
            result => result,
        }
    }
}
pub struct SdkFactory;
struct SdkConnection {
    service: RunningService<RoleClient, ()>,
    child: Option<tokio::process::Child>,
}
#[async_trait::async_trait]
impl ClientFactory for SdkFactory {
    async fn connect(
        &self,
        connector: &Connector,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn Connection>, McpError> {
        connector.validate()?;
        if cancel.is_cancelled() {
            return Err(McpError::Cancelled);
        }
        let mut child = None;
        let service = match connector {
            Connector::Stdio {
                command, args, env, ..
            } => {
                let binary = crate::process_control::resolve_binary(command)
                    .map_err(|_| McpError::Unavailable)?;
                let mut process = tokio::process::Command::new(binary);
                process
                    .args(args)
                    .env_clear()
                    .envs(crate::process_control::minimal_env());
                if let Some(env) = env {
                    process.envs(env);
                }
                let mut process = process
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|_| McpError::Transport)?;
                let output = process.stdout.take().ok_or(McpError::Transport)?;
                let input = process.stdin.take().ok_or(McpError::Transport)?;
                child = Some(process);
                let transport = rmcp::transport::async_rw::AsyncRwTransport::new_client(
                    BoundedLines::new(output),
                    input,
                );
                tokio::select! {biased;_=cancel.cancelled()=>return Err(McpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(15),().serve(transport))=>result.map_err(|_|McpError::Transport)?.map_err(|_|McpError::Transport)?}
            }
            // Native connectors run in-process; see `source_control::NativeFactory`.
            Connector::Github { .. } => return Err(McpError::Unavailable),
            Connector::Http { url, .. } => {
                let url = loopback_url(url)?;
                let addresses = [
                    std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
                    std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 0)),
                ];
                let client = reqwest_mcp::Client::builder()
                    .redirect(reqwest_mcp::redirect::Policy::none())
                    .no_proxy()
                    .resolve_to_addrs("localhost", &addresses)
                    .connect_timeout(Duration::from_secs(5))
                    .timeout(Duration::from_secs(60))
                    .build()
                    .map_err(|_| McpError::Transport)?;
                let mut config=rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(url.as_str());
                config.max_sse_event_size = 1048576;
                config.max_concurrent_requests = 1;
                config.channel_buffer_capacity = 16;
                config.reinit_on_expired_session = false;
                let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
                    BoundedHttp(client.clone()),
                    config,
                );
                let connected = tokio::select! {biased;_=cancel.cancelled()=>return Err(McpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(7),().serve(transport))=>result};
                match connected {
                    Ok(Ok(service)) => service,
                    _ => {
                        let legacy = async {
                            let transport =
                                crate::mcp_legacy::LegacySse::open(client, url, cancel).await?;
                            ().serve(transport).await.map_err(|_| McpError::Transport)
                        };
                        tokio::select! {biased;_=cancel.cancelled()=>return Err(McpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(7),legacy)=>result.map_err(|_|McpError::Transport)??}
                    }
                }
            }
        };
        Ok(Box::new(SdkConnection { service, child }))
    }
}
#[async_trait::async_trait]
impl Connection for SdkConnection {
    async fn tools(&mut self, cancel: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let params = cursor.clone().map(|cursor| {
                let mut params = rmcp::model::PaginatedRequestParams::default();
                params.cursor = Some(cursor);
                params
            });
            let result = tokio::select! {biased;_=cancel.cancelled()=>return Err(McpError::Cancelled),result=self.service.list_tools(params)=>result.map_err(|_|McpError::Transport)?};
            for tool in result.tools {
                tools.push(Tool {
                    name: tool.name.into_owned(),
                    description: tool
                        .description
                        .map(|value| value.into_owned())
                        .unwrap_or_default(),
                    input_schema: serde_json::to_value(tool.input_schema)
                        .map_err(|_| McpError::Invalid)?,
                });
                if tools.len() > 1024 {
                    return Err(McpError::Limit);
                }
            }
            cursor = result.next_cursor;
            if let Some(cursor) = &cursor {
                if !seen.insert(cursor.clone()) || seen.len() > 32 {
                    return Err(McpError::Limit);
                }
            } else {
                break;
            }
        }
        Ok(tools)
    }
    async fn call(
        &mut self,
        name: &str,
        args: serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        let params: rmcp::model::CallToolRequestParams =
            serde_json::from_value(serde_json::json!({"name":name,"arguments":args}))
                .map_err(|_| McpError::Invalid)?;
        let result = tokio::select! {biased;_=cancel.cancelled()=>return Err(McpError::Cancelled),result=self.service.call_tool(params)=>result.map_err(|_|McpError::Transport)?};
        let value = serde_json::to_value(result).map_err(|_| McpError::Invalid)?;
        let mut content = Vec::new();
        let mut bytes = 0;
        for block in value["content"].as_array().ok_or(McpError::Invalid)? {
            if block["type"] == "text" {
                let text = block["text"].as_str().ok_or(McpError::Invalid)?;
                bytes += text.len();
                if bytes > 1048576 {
                    return Err(McpError::Limit);
                }
                content.push(text);
            }
        }
        Ok(ToolResult {
            content: content.join("\n"),
            is_error: value["isError"] == true,
        })
    }
    async fn close(&mut self) -> Result<(), McpError> {
        let closed = self
            .service
            .close_with_timeout(Duration::from_secs(3))
            .await
            .map_err(|_| McpError::Transport);
        if let Some(mut child) = self.child.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), child.kill()).await;
        }
        if closed?.is_none() {
            return Err(McpError::Transport);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn official_sdk_discovers_and_invokes_over_in_memory_stdio() {
        let (client, server) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            let (input, mut output) = tokio::io::split(server);
            let mut lines = BufReader::new(input).lines();
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let Some(id) = request.get("id") else {
                    continue;
                };
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => {
                        serde_json::json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
                    }
                    "tools/list" => {
                        serde_json::json!({"tools":[{"name":"echo","description":"read echo","inputSchema":{"type":"object"}}]})
                    }
                    "tools/call" => {
                        serde_json::json!({"content":[{"type":"text","text":request["params"]["arguments"]["text"]}]})
                    }
                    other => panic!("unexpected request {other}"),
                };
                output
                    .write_all(
                        format!(
                            "{}\n",
                            serde_json::json!({"jsonrpc":"2.0","id":id,"result":result})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let (input, output) = tokio::io::split(client);
        let transport = rmcp::transport::async_rw::AsyncRwTransport::new_client(
            BoundedLines::new(input),
            output,
        );
        let service = tokio::time::timeout(Duration::from_secs(1), ().serve(transport))
            .await
            .unwrap()
            .unwrap();
        let mut connection = SdkConnection {
            service,
            child: None,
        };
        let cancel = CancellationToken::new();
        assert_eq!(connection.tools(&cancel).await.unwrap()[0].name, "echo");
        let result = connection
            .call("echo", serde_json::json!({"text":"hello"}), &cancel)
            .await
            .unwrap();
        assert_eq!(result.content, "hello");
        connection.close().await.unwrap();
        drop(connection);
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }
}
