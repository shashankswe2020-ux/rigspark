use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use rigspark_gui::{Host, router};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

fn wav() -> Vec<u8> {
    let mut bytes = b"RIFF\x24\0\0\0WAVEfmt ".to_vec();
    bytes.extend_from_slice(&[
        16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0, 0x7d, 0, 0, 2, 0, 16, 0,
    ]);
    bytes.extend_from_slice(b"data\0\0\0\0");
    bytes
}

/// An OpenAI-style audio server on loopback: WAV for speech, JSON for everything else.
async fn audio_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = vec![0u8; 65536];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let head = String::from_utf8_lossy(&request);
                let (kind, body): (&str, Vec<u8>) = if head.starts_with("POST /v1/audio/speech") {
                    ("audio/wav", wav())
                } else if head.starts_with("POST /v1/audio/transcriptions") {
                    (
                        "application/json",
                        br#"{"text":"hello from the microphone"}"#.to_vec(),
                    )
                } else {
                    ("application/json", br#"{"data":[]}"#.to_vec())
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    format!("http://{address}")
}

async fn send(
    host: &Arc<Host>,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    token: bool,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("127.0.0.1:{}", host.port))
        .header("origin", host.origin());
    if token {
        builder = builder.header("x-llmup-token", &host.token);
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let response = router(host.clone())
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, kind, bytes)
}
async fn call(
    host: &Arc<Host>,
    method: &str,
    path: &str,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) = send(
        host,
        method,
        path,
        payload.as_ref().map(|_| "application/json"),
        payload
            .map(|value| value.to_string().into_bytes())
            .unwrap_or_default(),
        true,
    )
    .await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn the_voice_catalog_lists_both_kinds_with_fit_and_engines() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43221).unwrap();
    let (status, value) = call(&host, "GET", "/api/voice/models", None).await;
    assert_eq!(status, StatusCode::OK);
    let models = value["models"].as_array().unwrap();
    assert!(
        models
            .iter()
            .any(|model| model["kind"] == "speak" && model["default"] == true)
    );
    assert!(
        models
            .iter()
            .any(|model| model["kind"] == "listen" && model["default"] == true)
    );
    for model in models {
        assert_eq!(model["fit"]["speed"], "unknown");
        assert!(model["integrity"].as_str().unwrap().len() > 10);
    }
    let engines: Vec<_> = value["engines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|engine| engine["engine"].as_str().unwrap())
        .collect();
    assert_eq!(engines, ["whisper-cpp", "mlx-audio"]);
    assert_eq!(value["status"], json!([]));
    // Queries are refused.
    let (status, _) = call(&host, "GET", "/api/voice/models?x=1", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mutations_need_the_capability_token_and_known_models() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43222).unwrap();
    for (path, payload) in [
        ("/api/voice/start", json!({"model": "kokoro-82m"})),
        ("/api/voice/stop", json!({"kind": "speak"})),
        ("/api/voice/speak", json!({"text": "hi"})),
    ] {
        let (status, _, _) = send(
            &host,
            "POST",
            path,
            Some("application/json"),
            payload.to_string().into_bytes(),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = call(
        &host,
        "POST",
        "/api/voice/start",
        Some(json!({"model": "nope"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &host,
        "POST",
        "/api/voice/stop",
        Some(json!({"kind": "sing"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, value) = call(
        &host,
        "POST",
        "/api/voice/speak",
        Some(json!({"text": "hi"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("no speak model is running")
    );
}

#[tokio::test]
async fn an_attached_server_speaks_and_transcribes_wav() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43223).unwrap();
    let endpoint = audio_server().await;
    for kind in ["speak", "listen"] {
        let (status, value) = call(
            &host,
            "POST",
            "/api/voice/attach",
            Some(json!({"kind": kind, "endpoint": endpoint, "model": "kokoro"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
    }
    let (_, value) = call(&host, "GET", "/api/voice/status", None).await;
    assert_eq!(value["status"].as_array().unwrap().len(), 2);

    let (status, kind, audio) = send(
        &host,
        "POST",
        "/api/voice/speak",
        Some("application/json"),
        json!({"text": "Hello", "voice": "af_heart", "speed": 1.2})
            .to_string()
            .into_bytes(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(kind.as_deref(), Some("audio/wav"));
    assert_eq!(audio, wav());

    let (status, _, bytes) = send(
        &host,
        "POST",
        "/api/voice/transcribe",
        Some("audio/wav"),
        wav(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["text"], "hello from the microphone");

    // Only WAV is accepted, and only with the token.
    let (status, _, _) = send(
        &host,
        "POST",
        "/api/voice/transcribe",
        Some("audio/webm"),
        wav(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (status, _, _) = send(
        &host,
        "POST",
        "/api/voice/transcribe",
        Some("audio/wav"),
        wav(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = send(
        &host,
        "POST",
        "/api/voice/transcribe",
        Some("audio/wav"),
        b"not a wav".to_vec(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let (status, value) = call(
        &host,
        "POST",
        "/api/voice/stop",
        Some(json!({"kind": "speak"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["stopped"], true);
    assert_eq!(value["status"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn attaching_refuses_non_loopback_endpoints() {
    let home = tempfile::tempdir().unwrap();
    let host = Host::new(home.path(), 43224).unwrap();
    for endpoint in [
        "http://example.com:8000",
        "https://127.0.0.1:8000",
        "http://127.0.0.1:1",
    ] {
        let (status, _) = call(
            &host,
            "POST",
            "/api/voice/attach",
            Some(json!({"kind": "speak", "endpoint": endpoint, "model": "kokoro"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{endpoint}");
    }
}

#[test]
fn the_page_offers_voice_in_models_chat_and_runtime() {
    let index = include_str!("../static/index.html");
    let voice = include_str!("../static/voice.js");
    let chat = include_str!("../static/chat.js");
    for marker in [
        "data-model-kind=\"voice\"",
        "data-mode=\"voice\"",
        "id=\"voice-model-panel\"",
        "id=\"dictate\"",
        "id=\"speak-replies\"",
        "id=\"voice-engines\"",
        "id=\"memory-bar\"",
        "/static/voice.js",
    ] {
        assert!(index.contains(marker), "{marker}");
    }
    assert!(chat.contains("rigspark:assistant-final"));
    assert!(voice.contains("rigspark:assistant-final"));
    assert!(voice.contains("/api/voice/transcribe"));
}
