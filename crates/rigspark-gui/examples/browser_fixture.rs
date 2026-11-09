use axum::{Json, extract::Request, http::StatusCode, response::IntoResponse};
use rigspark_gui::{Host, engine::Engine};
use rigspark_runtime::{
    harness::{DeltaSink, HarnessError, HarnessRequest},
    mcp::{Connection, ConnectorFile, Manager, McpError, Tool, ToolResult},
    ollama_inference::{ChatInput, ChatResult, ToolCall},
    state::RuntimeState,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Fixture;

#[derive(Default)]
struct Models {
    recommended_context: Option<u64>,
    starts: Vec<Value>,
}

struct DemoTool;
#[async_trait::async_trait]
impl Connection for DemoTool {
    async fn tools(&mut self, _: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        Ok(vec![Tool {
            name: "demo_tool".into(),
            description: "A deterministic demo tool".into(),
            input_schema: json!({"type": "object"}),
        }])
    }
    async fn call(
        &mut self,
        _: &str,
        _: Value,
        _: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        Ok(ToolResult {
            content: "demo tool result".into(),
            is_error: false,
        })
    }
    async fn close(&mut self) -> Result<(), McpError> {
        Ok(())
    }
}

fn installed_models() -> Value {
    json!({"models": [{
        "id": "gemma4:e4b-it-qat", "quant": "Q4_0", "context": 65536,
        "contextLength": 131072, "sizeBytes": 3_000_000_000_u64, "requiredBytes": null,
        "usableBytes": 8_000_000_000_u64, "memoryKind": "vram", "weightsFit": true,
        "fit": "unknown", "throughput": "unknown"
    }]})
}

// Recorded instead of launched: browser journeys must never start a runtime.
fn start_response(request: &Value) -> Value {
    if request["installed"] == true {
        json!({"active": {
            "modelId": request["model"], "runtimeModelId": "llmup-context-test:65536",
            "context": request["context"], "backend": "ollama",
            "endpoint": "http://127.0.0.1:11435", "port": 11435, "ownership": "attached"
        }})
    } else {
        json!({"active": null})
    }
}

fn with_context(mut recommended: Value, context: u64) -> Value {
    if let Some(models) = recommended["models"].as_array_mut() {
        for model in models {
            model["contextTokens"] = json!(context);
            model["contextFitKnown"] = json!(true);
        }
    }
    recommended
}

async fn set_tools(host: &Host, attach: bool) -> Result<(), McpError> {
    let connectors = if attach {
        r#"{"schemaVersion":1,"connectors":[{"id":"demo","name":"Demo","transport":"stdio","command":"unused"}]}"#
    } else {
        r#"{"schemaVersion":1,"connectors":[]}"#
    };
    let mut manager = Manager::new(ConnectorFile::parse(connectors)?)?;
    if attach {
        manager
            .attach("demo", Box::new(DemoTool), &CancellationToken::new())
            .await?;
    }
    let mut previous = std::mem::replace(&mut *host.connectors.lock().await, manager);
    previous.shutdown().await
}

const FORMATTING: &str = include_str!("support/chat-formatting.md");
const STRESS_FORMATTING: &str = include_str!("support/chat-stress.md");
const INCOMPLETE: &str =
    "Before the code.\n\n```typescript\nconst value = 1;\n```\n\nAfter the code.";
const PIXEL_PNG: [u8; 68] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5, 1, 1,
    39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn formatting() -> &'static str {
    FORMATTING.trim_end_matches('\n')
}

fn stress_formatting() -> &'static str {
    STRESS_FORMATTING.trim_end_matches('\n')
}

fn scroll_response() -> String {
    (1..=24)
        .map(|index| {
            format!("## Stream section {index}\n\nA paragraph with **useful detail** and `inline code` for the reader.")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

// Mirrors the legacy fixture's growing cuts so partial Markdown is exercised.
fn stream_chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut offset = 0;
    for length in [1, 2, 5, 9, 17, 31, 63, 127, 255, 511, 1023]
        .into_iter()
        .chain(std::iter::repeat(37))
    {
        if offset >= text.len() {
            break;
        }
        let end = text[offset..]
            .char_indices()
            .nth(length)
            .map_or(text.len(), |(relative, _)| offset + relative);
        chunks.push(&text[offset..end]);
        offset = end;
    }
    chunks
}

async fn stream(
    parts: impl IntoIterator<Item = (String, u64)>,
    cancel: &CancellationToken,
    sink: &mut DeltaSink<'_>,
) -> Result<(), HarnessError> {
    for (part, delay) in parts {
        sink(&part)?;
        tokio::select! {
            () = cancel.cancelled() => return Err(HarnessError::Cancelled),
            () = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
        }
    }
    Ok(())
}
#[async_trait::async_trait]
impl Engine for Fixture {
    async fn agent(
        &self,
        input: &ChatInput,
        _: &RuntimeState,
        _: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        let last = input.messages.last().ok_or(HarnessError::Invalid)?;
        let (content, tool_calls) = if last.role == "tool" {
            ("Tool finished. Done.".to_owned(), vec![])
        } else if last.content.contains("TOOL") {
            let arguments = [("q".to_owned(), json!("hi"))].into_iter().collect();
            let call = ToolCall {
                name: "demo_tool".into(),
                arguments,
            };
            (String::new(), vec![call])
        } else {
            (format!("Native reply: {}", last.content), vec![])
        };
        if !content.is_empty() {
            sink(&content)?;
        }
        Ok(ChatResult {
            content,
            tool_calls,
            timings: None,
        })
    }
    async fn chat(
        &self,
        _: &str,
        request: &HarnessRequest,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        let message = &request
            .messages
            .last()
            .ok_or(HarnessError::Invalid)?
            .content;
        if message == "cancel this response" {
            sink("Pending fixture response")?;
            cancel.cancelled().await;
            return Err(HarnessError::Cancelled);
        }
        match message.as_str() {
            "FORMAT_MARKDOWN" => {
                sink(formatting())?;
                return Ok(formatting().to_owned());
            }
            "FORMAT_MARKDOWN_STREAM" => {
                let parts = stream_chunks(formatting())
                    .into_iter()
                    .map(|part| (part.to_owned(), 0))
                    .collect::<Vec<_>>();
                stream(parts, cancel, sink).await?;
                return Ok(formatting().to_owned());
            }
            "FORMAT_STRESS_STREAM" => {
                let parts = stream_chunks(stress_formatting())
                    .into_iter()
                    .map(|part| (part.to_owned(), 0))
                    .collect::<Vec<_>>();
                stream(parts, cancel, sink).await?;
                return Ok(stress_formatting().to_owned());
            }
            "FORMAT_MARKDOWN_SCROLL" => {
                let text = scroll_response();
                let parts = text
                    .as_bytes()
                    .chunks(80)
                    .map(|part| (String::from_utf8_lossy(part).into_owned(), 100))
                    .collect::<Vec<_>>();
                stream(parts, cancel, sink).await?;
                return Ok(text);
            }
            "FORMAT_MARKDOWN_INCOMPLETE" => {
                let (head, tail) =
                    INCOMPLETE.split_at(INCOMPLETE.find(" value").ok_or(HarnessError::Invalid)?);
                stream(
                    [(head.to_owned(), 1500), (tail.to_owned(), 0)],
                    cancel,
                    sink,
                )
                .await?;
                return Ok(INCOMPLETE.to_owned());
            }
            _ => {}
        }
        let reply = format!("Native reply: {message}");
        for word in reply.split_inclusive(' ') {
            if cancel.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            sink(word)?;
            tokio::task::yield_now().await;
        }
        Ok(reply)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = tempfile::tempdir()?;
    std::fs::create_dir_all(home.path().join("artifacts"))?;
    std::fs::write(home.path().join("artifacts/formatting.png"), PIXEL_PNG)?;
    let workspace = tempfile::tempdir()?;
    std::fs::create_dir_all(workspace.path().join("src"))?;
    std::fs::write(
        workspace.path().join("src/app.ts"),
        "export const answer = 42;\nconsole.log(answer);\n",
    )?;
    let workspace_path = workspace.path().to_string_lossy().into_owned();
    let port = std::env::var("RUST_GUI_TEST_PORT")
        .unwrap_or_else(|_| "4322".into())
        .parse::<u16>()?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let host = Host::with_engine(home.path(), port, Arc::new(Fixture))?;
    let shutdown = host.shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown.cancel();
    });
    let update = Arc::new(std::sync::Mutex::new(offline_update()));
    let current = update.clone();
    let models = Arc::new(Mutex::new(Models::default()));
    let (recommended, context_control, starts, start_log) =
        (models.clone(), models.clone(), models.clone(), models);
    let inner = rigspark_gui::router(host.clone());
    let tool_host = host.clone();
    let catalog_revision = Arc::new(Mutex::new(0_u64));
    let catalog_status = catalog_revision.clone();
    let catalog_update_calls = Arc::new(Mutex::new(0_u64));
    let router = rigspark_gui::router(host.clone())
        .route("/api/catalog/status", axum::routing::get(move || {
            let revision = *catalog_status.lock().unwrap();
            async move { Json(json!({"catalog": {"source": if revision == 0 { "bundled" } else { "updated" }, "revision": revision, "generatedAt": "2026-09-01T00:00:00Z", "publishedAt": if revision == 0 { None } else { Some("2026-09-30T00:00:00Z") }, "modelCount": 66, "digest": "fixture-digest", "updatesConfigured": true, "warnings": []}})) }
        }))
        .route("/api/catalog/update", axum::routing::post(move || {
            let mut revision = catalog_revision.lock().unwrap();
            let mut calls = catalog_update_calls.lock().unwrap();
            let failed = *calls >= 2;
            *calls += 1;
            *revision = 1;
            async move {
                if failed {
                    (StatusCode::BAD_GATEWAY, Json(json!({"error":"Catalog download failed (offline)."}))).into_response()
                } else {
                    Json(json!({"catalog": {"source":"updated", "revision":1, "generatedAt":"2026-09-01T00:00:00Z", "publishedAt":"2026-09-30T00:00:00Z", "modelCount":66, "digest":"fixture-digest", "updatesConfigured":true, "warnings":[]}})).into_response()
                }
            }
        }))
        .route(
            "/api/models/recommended",
            axum::routing::get(move |request: Request| async move {
                let Ok(response) = inner.oneshot(request).await;
                let context = recommended
                    .lock()
                    .ok()
                    .and_then(|models| models.recommended_context);
                let Some(context) = context else {
                    return response;
                };
                let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024).await;
                match bytes.map(|bytes| serde_json::from_slice::<Value>(&bytes)) {
                    Ok(Ok(value)) => Json(with_context(value, context)).into_response(),
                    _ => StatusCode::BAD_GATEWAY.into_response(),
                }
            }),
        )
        .route(
            "/api/models/installed",
            axum::routing::get(|| async { Json(installed_models()) }),
        )
        .route(
            "/api/models/up",
            axum::routing::post(move |Json(request): Json<Value>| async move {
                let response = start_response(&request);
                match starts.lock() {
                    Ok(mut models) => {
                        models.starts.push(request);
                        Json(response).into_response()
                    }
                    Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                }
            }),
        )
        // Test-only controls; this example never serves real user storage.
        .route(
            "/__fixture/model-starts",
            axum::routing::get(move || async move {
                Json(
                    start_log
                        .lock()
                        .map(|models| json!(models.starts))
                        .unwrap_or_default(),
                )
            }),
        )
        .route(
            "/__fixture/recommended-context",
            axum::routing::post(move |body: String| async move {
                let Ok(context) = (!body.is_empty()).then(|| body.parse::<u64>()).transpose()
                else {
                    return StatusCode::BAD_REQUEST;
                };
                match context_control.lock() {
                    Ok(mut models) => {
                        models.recommended_context = context;
                        StatusCode::NO_CONTENT
                    }
                    Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
                }
            }),
        )
        .route(
            "/__fixture/tools",
            axum::routing::post(move |body: String| async move {
                let attach = match body.as_str() {
                    "attach" => true,
                    "detach" => false,
                    _ => return StatusCode::BAD_REQUEST,
                };
                match set_tools(&tool_host, attach).await {
                    Ok(()) => StatusCode::NO_CONTENT,
                    Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
                }
            }),
        )
        .route(
            "/__fixture/workspace",
            axum::routing::get(move || async move { Json(workspace_path) }),
        )
        .route(
            "/api/update",
            axum::routing::get(move || async move {
                axum::Json(
                    current
                        .lock()
                        .map(|value| value.clone())
                        .unwrap_or_default(),
                )
            }),
        )
        // Test-only control; this example never serves real user storage.
        .route(
            "/__fixture/update",
            axum::routing::post(move |body: String| async move {
                let Some(next) = fixture_update(&body) else {
                    return axum::http::StatusCode::BAD_REQUEST;
                };
                match update.lock() {
                    Ok(mut value) => {
                        *value = next;
                        axum::http::StatusCode::NO_CONTENT
                    }
                    Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                }
            }),
        );
    axum::serve(listener, router)
        .with_graceful_shutdown(host.shutdown.clone().cancelled_owned())
        .await?;
    host.cancel_chat();
    host.tasks.close();
    host.tasks.wait().await;
    Ok(())
}

fn offline_update() -> serde_json::Value {
    serde_json::json!({
        "state": "unknown", "currentVersion": env!("CARGO_PKG_VERSION"),
        "latestVersion": null, "releaseUrl": null
    })
}

fn fixture_update(kind: &str) -> Option<serde_json::Value> {
    match kind {
        "unknown" => Some(offline_update()),
        "available" => Some(serde_json::json!({
            "state": "update-available", "currentVersion": "0.11.2",
            "latestVersion": "0.12.0",
            "releaseUrl": "https://github.com/rigspark/rigspark/releases"
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fixture_update_does_not_claim_a_network_result() {
        let update = super::offline_update();
        assert_eq!(update["state"], "unknown");
        assert!(update["latestVersion"].is_null());
        assert!(update["releaseUrl"].is_null());
    }

    #[test]
    fn formatting_stream_reassembles_exactly_with_growing_cuts() {
        let chunks = super::stream_chunks(super::formatting());
        assert_eq!(chunks.concat(), super::formatting());
        assert_eq!(&chunks[..4], &["#", " D", "eploy", "ment resu"]);
        assert!(super::formatting().is_ascii());
        assert!(super::scroll_response().is_ascii());
        assert!(super::formatting().ends_with("PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)"));
    }

    #[test]
    fn formatting_stream_preserves_unicode_at_every_chunk_boundary() {
        let response = "ASCII → café 🚀 汉字 العربية";
        assert_eq!(super::stream_chunks(response).concat(), response);
        assert_eq!(
            super::stream_chunks(super::stress_formatting()).concat(),
            super::stress_formatting()
        );
    }

    #[test]
    fn model_fixtures_record_without_launching_and_override_context() {
        let installed = super::start_response(
            &serde_json::json!({"model": "m", "installed": true, "context": 65536}),
        );
        assert_eq!(installed["active"]["ownership"], "attached");
        assert_eq!(installed["active"]["context"], 65536);
        assert!(super::start_response(&serde_json::json!({"model": "m"}))["active"].is_null());
        let changed = super::with_context(
            serde_json::json!({"models": [{"id": "a"}, {"id": "b"}]}),
            65536,
        );
        assert!(
            changed["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|model| model["contextTokens"] == 65536 && model["contextFitKnown"] == true)
        );
        assert_eq!(
            super::installed_models()["models"][0]["id"],
            "gemma4:e4b-it-qat"
        );
    }

    #[test]
    fn fixture_update_control_accepts_only_known_states() {
        assert_eq!(
            super::fixture_update("unknown"),
            Some(super::offline_update())
        );
        let available = super::fixture_update("available").unwrap();
        assert_eq!(available["state"], "update-available");
        assert_eq!(available["latestVersion"], "0.12.0");
        for kind in ["", "Available", "https://example.com"] {
            assert!(super::fixture_update(kind).is_none(), "{kind}");
        }
    }
}
