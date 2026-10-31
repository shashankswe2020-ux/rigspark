use rigspark_runtime::{
    http::{HttpError, Request, Response, Transport},
    identity::{Listener, ProcessIdentity, ProcessProbe},
    ollama_inference::{ChatInput, ChatMessage, OllamaInference},
};
use serde_json::json;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
#[cfg(windows)]
const OLLAMA: &str = "C:/opt/bin/ollama.exe";
#[cfg(not(windows))]
const OLLAMA: &str = "/opt/bin/ollama";
struct Probe;
fn identity() -> ProcessIdentity {
    ProcessIdentity {
        pid: 123,
        process: "ollama".into(),
        executable: OLLAMA.into(),
        started: "start".into(),
    }
}
#[async_trait::async_trait]
impl ProcessProbe for Probe {
    async fn listener(
        &self,
        port: u16,
        host: &str,
    ) -> Result<Listener, rigspark_runtime::state::StateError> {
        Ok(Listener {
            identity: identity(),
            address: host.into(),
            port,
        })
    }
    async fn process(
        &self,
        _pid: u32,
    ) -> Result<ProcessIdentity, rigspark_runtime::state::StateError> {
        Ok(identity())
    }
}
struct Http {
    bodies: Mutex<std::collections::VecDeque<Vec<u8>>>,
    requests: Mutex<Vec<serde_json::Value>>,
}
#[async_trait::async_trait]
impl Transport for Http {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        self.requests
            .lock()
            .unwrap()
            .push(request.body().cloned().unwrap_or(serde_json::Value::Null));
        Ok(Response {
            status: 200,
            body: Box::pin(std::io::Cursor::new(
                self.bodies.lock().unwrap().pop_front().unwrap(),
            )),
        })
    }
}
fn input() -> ChatInput {
    ChatInput {
        model: "test:latest".into(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "hello".into(),
            tool_calls: Vec::new(),
            tool_name: None,
        }],
        tools: Vec::new(),
        temperature: Some(0.2),
    }
}
#[tokio::test]
async fn native_chat_and_stream_use_exact_model_and_collect_tool_calls() {
    let http=Http{bodies:Mutex::new([b"{\"version\":\"0.11.4\"}".to_vec(),serde_json::to_vec(&json!({"message":{"content":"hello","tool_calls":[{"function":{"name":"lookup","arguments":{"query":"x"}}}]}})).unwrap(),b"{\"version\":\"0.11.4\"}".to_vec(),b"{\"message\":{\"content\":\"one\"},\"done\":false}\n{\"message\":{\"content\":\"two\"},\"done\":true}\n".to_vec()].into()),requests:Mutex::new(Vec::new())};
    let api = OllamaInference::new(&http, &Probe, OLLAMA);
    let cancel = CancellationToken::new();
    let result = api
        .chat("http://127.0.0.1:11435", &identity(), &input(), &cancel)
        .await
        .unwrap();
    assert_eq!(result.content, "hello");
    assert_eq!(result.tool_calls[0].name, "lookup");
    let mut deltas = Vec::new();
    let result = api
        .chat_stream(
            "http://127.0.0.1:11435",
            &identity(),
            &input(),
            &cancel,
            &mut |chunk| deltas.push(chunk.to_owned()),
        )
        .await
        .unwrap();
    assert_eq!(result.content, "onetwo");
    assert_eq!(deltas, vec!["one", "two"]);
    assert_eq!(http.requests.lock().unwrap()[1]["model"], "test:latest");
}
#[tokio::test]
async fn inference_refuses_identity_drift_before_sending_http() {
    let http = Http {
        bodies: Mutex::new(Default::default()),
        requests: Mutex::new(Vec::new()),
    };
    let api = OllamaInference::new(&http, &Probe, OLLAMA);
    let mut expected = identity();
    expected.started = "another process".into();
    assert!(
        api.chat(
            "http://127.0.0.1:11435",
            &expected,
            &input(),
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert!(http.requests.lock().unwrap().is_empty());
}
#[tokio::test]
async fn embeddings_validate_count_dimensions_and_finite_scalars() {
    for (vectors, valid) in [
        (json!([[0.1, 0.2], [0.3, 0.4]]), true),
        (json!([[0.1], [0.2, 0.3]]), false),
        (json!([[0.1, 0.2]]), false),
    ] {
        let http = Http {
            bodies: Mutex::new(
                [
                    b"{\"version\":\"0.11.4\"}".to_vec(),
                    serde_json::to_vec(&json!({"embeddings":vectors})).unwrap(),
                ]
                .into(),
            ),
            requests: Mutex::new(Vec::new()),
        };
        let api = OllamaInference::new(&http, &Probe, OLLAMA);
        let result = api
            .embed(
                "http://127.0.0.1:11435",
                &identity(),
                "embed:latest",
                &["first".into(), "second".into()],
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.is_ok(), valid);
    }
}

#[tokio::test]
async fn streaming_matches_lenient_ndjson_contract_with_bounded_records() {
    for (bytes, expected) in [
        (
            b"{\"message\":{\"content\":\"partial\"}}\n".to_vec(),
            Some("partial"),
        ),
        (b"not json\n".to_vec(), Some("")),
        (vec![b'x'; 65537], None),
        (
            b"{\"done\":true}\n{\"message\":{\"content\":\"late\"}}\n".to_vec(),
            Some("late"),
        ),
        (
            b"{\"message\":{\"content\":\"unterminated\"}}".to_vec(),
            Some(""),
        ),
    ] {
        let http = Http {
            bodies: Mutex::new([b"{\"version\":\"0.11.4\"}".to_vec(), bytes].into()),
            requests: Mutex::new(Vec::new()),
        };
        let api = OllamaInference::new(&http, &Probe, OLLAMA);
        let result = api
            .chat_stream(
                "http://127.0.0.1:11435",
                &identity(),
                &input(),
                &CancellationToken::new(),
                &mut |_| {},
            )
            .await;
        match expected {
            Some(content) => assert_eq!(result.unwrap().content, content),
            None => assert!(result.is_err()),
        }
    }
}
struct ChangingProbe(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl ProcessProbe for ChangingProbe {
    async fn listener(
        &self,
        port: u16,
        host: &str,
    ) -> Result<Listener, rigspark_runtime::state::StateError> {
        let mut observed = identity();
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2 {
            observed.started = "replacement".into();
        }
        Ok(Listener {
            identity: observed,
            address: host.into(),
            port,
        })
    }
    async fn process(
        &self,
        _pid: u32,
    ) -> Result<ProcessIdentity, rigspark_runtime::state::StateError> {
        Ok(identity())
    }
}
#[tokio::test]
async fn changed_listener_after_response_invalidates_completion() {
    let http = Http {
        bodies: Mutex::new(
            [
                b"{\"version\":\"0.11.4\"}".to_vec(),
                b"{\"message\":{\"content\":\"untrusted\"}}".to_vec(),
            ]
            .into(),
        ),
        requests: Mutex::new(Vec::new()),
    };
    let probe = ChangingProbe(std::sync::atomic::AtomicUsize::new(0));
    let api = OllamaInference::new(&http, &probe, OLLAMA);
    assert!(
        api.chat(
            "http://127.0.0.1:11435",
            &identity(),
            &input(),
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn tool_calls_with_ollama_call_ids_and_indexes_are_not_dropped() {
    // Shape returned by Ollama 0.32.5 for qwen3:30b-a3b with tools attached.
    let call =
        json!({"id":"call_d8t22shg","function":{"index":0,"name":"get_today","arguments":{}}});
    let reply = json!({"message":{"role":"assistant","content":"","thinking":"Checking.","tool_calls":[call]},"done":true});
    let http = Http {
        bodies: Mutex::new(
            [
                b"{\"version\":\"0.32.5\"}".to_vec(),
                serde_json::to_vec(&reply).unwrap(),
                b"{\"version\":\"0.32.5\"}".to_vec(),
                [serde_json::to_vec(&reply).unwrap(), b"\n".to_vec()].concat(),
            ]
            .into(),
        ),
        requests: Mutex::new(Vec::new()),
    };
    let api = OllamaInference::new(&http, &Probe, OLLAMA);
    let cancel = CancellationToken::new();
    let result = api
        .chat("http://127.0.0.1:11435", &identity(), &input(), &cancel)
        .await
        .unwrap();
    assert_eq!(result.tool_calls.len(), 1);
    assert_eq!(result.tool_calls[0].name, "get_today");
    let streamed = api
        .chat_stream(
            "http://127.0.0.1:11435",
            &identity(),
            &input(),
            &cancel,
            &mut |_| {},
        )
        .await
        .unwrap();
    assert_eq!(streamed.tool_calls.len(), 1);
    assert_eq!(streamed.tool_calls[0].name, "get_today");
}

struct Loader {
    status: u16,
    body: &'static str,
    seen: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
}
#[async_trait::async_trait]
impl Transport for Loader {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        self.seen.lock().unwrap().push((
            request.url.path().to_owned(),
            request.body().cloned().unwrap_or_default(),
        ));
        Ok(Response {
            status: self.status,
            body: Box::pin(std::io::Cursor::new(self.body.as_bytes().to_vec())),
        })
    }
}

#[tokio::test]
async fn preload_loads_one_token_and_reports_why_ollama_cannot_load_a_model() {
    use rigspark_runtime::ollama_inference::preload;
    let cancel = tokio_util::sync::CancellationToken::new();
    let ok = Loader {
        status: 200,
        body: r#"{"done":true}"#,
        seen: Default::default(),
    };
    preload(&ok, "http://127.0.0.1:11434", "hf.co/a/b:q4_k_m", &cancel)
        .await
        .unwrap();
    let (path, body) = ok.seen.lock().unwrap()[0].clone();
    assert_eq!(path, "/api/generate");
    assert_eq!(body["model"], "hf.co/a/b:q4_k_m");
    assert_eq!(body["options"]["num_predict"], 1);
    assert_eq!(body["stream"], false);
    let unsupported = Loader {
        status: 500,
        body: "{\"error\":\"llama-server process has terminated: error loading model: unknown model architecture: 'spark2_5'\\u001b[31m\"}",
        seen: Default::default(),
    };
    let reason = preload(&unsupported, "http://127.0.0.1:11434", "m:latest", &cancel)
        .await
        .unwrap_err();
    assert!(
        reason.contains("unknown model architecture: 'spark2_5'"),
        "{reason}"
    );
    assert!(!reason.contains('\u{1b}'), "runtime text is sanitized");
    let opaque = Loader {
        status: 502,
        body: "<html>",
        seen: Default::default(),
    };
    assert_eq!(
        preload(&opaque, "http://127.0.0.1:11434", "m:latest", &cancel)
            .await
            .unwrap_err(),
        "Ollama returned HTTP 502"
    );
}
