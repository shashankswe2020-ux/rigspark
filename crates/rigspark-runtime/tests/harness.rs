#[tokio::test]
async fn builtin_registry_preserves_order_and_disables_missing_configuration() {
    use rigspark_runtime::{
        harness::LocalHarness,
        harness_registry::HarnessRegistry,
        identity::NativeProcessProbe,
        lifecycle::Registry,
        opencode::NativeOpenCodeRunner,
        state::{Config, StateStore},
    };
    let root = tempfile::tempdir().unwrap();
    let state = StateStore::new(Config::from_home(root.path()).unwrap());
    let backends = Registry::new(Vec::new());
    let transport = Transport {
        body: Vec::new(),
        request: Mutex::new(None),
    };
    let runner = NativeOpenCodeRunner {
        binary: root.path().join("missing-opencode"),
        env: Default::default(),
    };
    let registry = HarnessRegistry::builtins(
        LocalHarness {
            state: &state,
            registry: &backends,
            probe: &NativeProcessProbe,
        },
        &transport,
        &runner,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        registry.all(),
        vec!["local", "claude", "openai", "openai-compatible", "opencode"]
    );
    assert!(registry.available().await.is_empty());
    assert!(registry.get("unknown").is_err());
    let env = [(
        "OPENAI_COMPAT_BASE_URL".into(),
        "http://127.0.0.1:3000/chat".into(),
    )]
    .into();
    let registry = HarnessRegistry::builtins(
        LocalHarness {
            state: &state,
            registry: &backends,
            probe: &NativeProcessProbe,
        },
        &transport,
        &runner,
        &env,
    )
    .unwrap();
    assert_eq!(registry.available().await, vec!["openai-compatible"]);
}
#[tokio::test]
async fn provider_caps_and_cancellation_fail_before_unbounded_output() {
    let transport = Transport {
        body: vec![b'x'; 65],
        request: Mutex::new(None),
    };
    let harness = RemoteHarness::new(
        Provider::Compatible,
        "http://127.0.0.1:3000/chat",
        None,
        &transport,
    )
    .unwrap()
    .with_limit(64)
    .unwrap();
    assert!(matches!(
        harness
            .chat(&input(), &CancellationToken::new(), &mut |_| Ok(()))
            .await,
        Err(HarnessError::Limit)
    ));
    struct Stalled;
    #[async_trait::async_trait]
    impl RemoteTransport for Stalled {
        async fn send(&self, _: RemoteRequest) -> Result<RemoteResponse, HarnessError> {
            std::future::pending().await
        }
    }
    let harness = RemoteHarness::new(
        Provider::Compatible,
        "http://127.0.0.1:3000/chat",
        None,
        &Stalled,
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let cancellation = cancel.clone();
    let input = input();
    let mut sink = |_: &str| Ok(());
    let request = harness.chat(&input, &cancel, &mut sink);
    let cancel_task = async move {
        tokio::task::yield_now().await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(request, cancel_task);
    assert!(matches!(result, Err(HarnessError::Cancelled)));
}
use rigspark_runtime::harness::{
    HarnessError, HarnessMessage, HarnessRequest, Provider, RemoteHarness, RemoteRequest,
    RemoteResponse, RemoteTransport, Secret,
};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
struct Transport {
    body: Vec<u8>,
    request: Mutex<Option<serde_json::Value>>,
}
#[async_trait::async_trait]
impl RemoteTransport for Transport {
    async fn send(&self, request: RemoteRequest) -> Result<RemoteResponse, HarnessError> {
        *self.request.lock().unwrap() = Some(request.body);
        Ok(RemoteResponse {
            status: 200,
            body: Box::pin(std::io::Cursor::new(self.body.clone())),
        })
    }
}
fn input() -> HarnessRequest {
    HarnessRequest {
        model: "model".into(),
        messages: vec![HarnessMessage {
            role: "user".into(),
            content: "hello".into(),
        }],
        temperature: Some(0.3),
    }
}
#[tokio::test]
async fn remote_harnesses_parse_sse_and_emit_incremental_text() {
    for (provider, url, body) in [
        (
            Provider::OpenAi,
            "https://api.openai.com/v1/chat/completions",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n",
        ),
        (
            Provider::Claude,
            "https://api.anthropic.com/v1/messages",
            "event: content_block_delta\ndata: {\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
        ),
    ] {
        let transport = Transport {
            body: body.as_bytes().to_vec(),
            request: Mutex::new(None),
        };
        let harness = RemoteHarness::new(
            provider,
            url,
            Some(Secret::new("secret-value").unwrap()),
            &transport,
        )
        .unwrap();
        let mut chunks = Vec::new();
        let result = harness
            .chat(&input(), &CancellationToken::new(), &mut |text| {
                chunks.push(text.to_owned());
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(result, "hello");
        assert_eq!(chunks, vec!["hello"]);
        assert_eq!(
            transport.request.lock().unwrap().as_ref().unwrap()["stream"],
            true
        );
    }
}
#[test]
fn endpoints_and_secret_debug_are_safe() {
    let transport = Transport {
        body: Vec::new(),
        request: Mutex::new(None),
    };
    assert!(
        RemoteHarness::new(
            Provider::OpenAi,
            "https://evil.example/v1",
            Some(Secret::new("secret-value").unwrap()),
            &transport
        )
        .is_err()
    );
    assert!(
        RemoteHarness::new(Provider::Compatible, "http://10.0.0.1/v1", None, &transport).is_err()
    );
    assert!(!format!("{:?}", Secret::new("secret-value").unwrap()).contains("secret-value"));
    assert!(Secret::new("secret\nheader").is_err());
}

#[tokio::test]
async fn usage_only_stream_frame_records_exact_counts_without_emitting_text() {
    use rigspark_runtime::usage::{InferenceUsage, scope};
    let transport = Transport { body: b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":60}}}\n\ndata: [DONE]\n\n".to_vec(), request: Mutex::new(None) };
    let harness = RemoteHarness::new(
        Provider::OpenAi,
        "https://api.openai.com/v1/chat/completions",
        Some(Secret::new("fixture-key").unwrap()),
        &transport,
    )
    .unwrap();
    let usage = std::sync::Arc::new(Mutex::new(InferenceUsage::default()));
    let mut emitted = String::new();
    let result = scope(usage.clone(), async {
        harness
            .chat(&input(), &CancellationToken::new(), &mut |text| {
                emitted.push_str(text);
                Ok(())
            })
            .await
    })
    .await
    .unwrap();
    assert_eq!(result, "hello");
    assert_eq!(emitted, "hello");
    assert_eq!(
        *usage.lock().unwrap(),
        InferenceUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_hit_tokens: Some(60),
            cache_miss_tokens: Some(40)
        }
    );
    assert_eq!(
        transport.request.lock().unwrap().as_ref().unwrap()["stream_options"]["include_usage"],
        true
    );
}

#[tokio::test]
async fn secrets_split_across_events_never_reach_delta_sink() {
    let body = ["safe sec", "ret-", "value tail"]
        .map(|text| {
            format!(
                "data: {}\n\n",
                serde_json::json!({"choices":[{"delta":{"content":text}}]})
            )
        })
        .join("");
    let transport = Transport {
        body: body.into_bytes(),
        request: Mutex::new(None),
    };
    let harness = RemoteHarness::new(
        Provider::OpenAi,
        "https://api.openai.com/v1/chat/completions",
        Some(Secret::new("secret-value").unwrap()),
        &transport,
    )
    .unwrap();
    let mut emitted = String::new();
    let result = harness
        .chat(&input(), &CancellationToken::new(), &mut |text| {
            emitted.push_str(text);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(result, "safe [REDACTED] tail");
    assert_eq!(result, emitted);
}

mod boundaries {
    use super::{Transport, input};
    use rigspark_runtime::harness::{
        ChatHarness, HarnessError, HarnessMessage, HarnessRequest, Provider, RemoteHarness, Secret,
        consume_sse, external_url,
    };
    use std::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    fn request(change: impl FnOnce(&mut HarnessRequest)) -> bool {
        let mut value = input();
        change(&mut value);
        value.validate().is_ok()
    }
    fn message(role: &str, content: String) -> HarnessMessage {
        HarnessMessage {
            role: role.into(),
            content,
        }
    }

    #[test]
    fn requests_are_bounded_field_by_field() {
        assert!(request(|_| {}));
        assert!(request(|r| r.model = "m".repeat(8192)));
        assert!(!request(|r| r.model = "m".repeat(8193)));
        assert!(!request(|r| r.model = "m\u{7}".into()));
        assert!(request(
            |r| r.messages = vec![message("user", "x".into()); 10000]
        ));
        assert!(!request(
            |r| r.messages = vec![message("user", "x".into()); 10001]
        ));
        for role in ["system", "user", "assistant"] {
            assert!(request(|r| r.messages[0].role = role.into()), "{role}");
        }
        assert!(!request(|r| r.messages[0].role = "tool".into()));
        let mib = 1024 * 1024;
        assert!(request(
            |r| r.messages = vec![message("user", "x".repeat(2 * mib)); 2]
        ));
        assert!(!request(|r| {
            r.messages = vec![
                message("user", "x".repeat(2 * mib)),
                message("user", "x".repeat(2 * mib + 1)),
            ]
        }));
        assert!(!request(|r| r.temperature = Some(f64::NAN)));
        assert!(!request(|r| r.temperature = Some(f64::INFINITY)));
        assert!(request(|r| r.temperature = None));
    }

    #[test]
    fn endpoints_are_https_or_loopback_http_without_credentials() {
        for good in [
            "https://api.example.com/v1",
            "http://localhost:8080/v1",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:8080/v1",
            "https://8.8.8.8/v1",
            "https://[2606:4700::1]/v1",
            // Exactly 2048 bytes.
            &format!("https://api.example.com/{}", "a".repeat(2048 - 24)),
        ] {
            assert!(external_url(good).is_ok(), "{good}");
        }
        for bad in [
            "http://api.example.com/v1",
            "https://user@api.example.com/v1",
            "https://user:pass@api.example.com/v1",
            "https://:pass@api.example.com/v1",
            "https://api.example.com/v1#fragment",
            "https://10.0.0.1/v1",
            "https://[fc00::1]/v1",
            "http://10.0.0.1/v1",
            "file:///etc/passwd",
            "https://api.example.com/v1\u{7}",
            &format!("https://api.example.com/{}", "a".repeat(2048)),
        ] {
            assert!(external_url(bad).is_err(), "{bad}");
        }
        assert_eq!(
            format!("{:?}", Secret::new("secret-value").unwrap()),
            "[REDACTED]"
        );
    }

    fn transport(body: &str) -> Transport {
        Transport {
            body: body.as_bytes().to_vec(),
            request: Mutex::new(None),
        }
    }
    fn compatible(transport: &Transport) -> RemoteHarness<'_> {
        RemoteHarness::new(
            Provider::Compatible,
            "http://127.0.0.1:9/v1",
            None,
            transport,
        )
        .unwrap()
    }
    fn frame(text: &str) -> String {
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n")
    }

    #[test]
    fn output_limits_are_between_one_byte_and_16_mib() {
        let mib = 1024 * 1024;
        let body = transport("");
        assert!(compatible(&body).with_limit(16 * mib).is_ok());
        assert!(compatible(&body).with_limit(16 * mib + 1).is_err());
        assert!(compatible(&body).with_limit(0).is_err());
        assert!(compatible(&body).with_limit(1).is_ok());
    }

    /// A harness whose one-character-per-match secret makes redacted text longer than the
    /// stream that carried it, so the text limit is reached before the stream limit.
    async fn redacted(secret: &str, text: &str, limit: usize) -> Result<String, HarnessError> {
        let body = transport(&frame(text));
        assert!(frame(text).len() <= limit, "the stream itself must fit");
        RemoteHarness::new(
            Provider::Compatible,
            "http://127.0.0.1:9/v1",
            Some(Secret::new(secret).unwrap()),
            &body,
        )
        .unwrap()
        .with_limit(limit)
        .unwrap()
        .chat(&input(), &CancellationToken::new(), &mut |_| Ok(()))
        .await
    }

    #[tokio::test]
    async fn redacted_text_counts_against_the_output_limit() {
        // Ten matches become 100 characters of markers from a 56-byte stream.
        let text = "k".repeat(10);
        assert_eq!(
            redacted("k", &text, 100).await.unwrap(),
            "[REDACTED]".repeat(10)
        );
        assert!(matches!(
            redacted("k", &text, 99).await,
            Err(HarnessError::Limit)
        ));
        // A trailing partial secret is held back and still counts when released at the end.
        let text = format!("{}a", "ab".repeat(10));
        let expected = format!("{}a", "[REDACTED]".repeat(10));
        assert_eq!(redacted("ab", &text, 101).await.unwrap(), expected);
        assert!(matches!(
            redacted("ab", &text, 100).await,
            Err(HarnessError::Limit)
        ));
    }

    #[tokio::test]
    async fn the_registered_harness_reports_availability_and_forwards_chat() {
        let body = transport(&frame("hello"));
        let open = compatible(&body);
        assert!(open.available().await);
        assert_eq!(
            ChatHarness::chat(&open, &input(), &CancellationToken::new(), &mut |_| Ok(()))
                .await
                .unwrap(),
            "hello"
        );
        let keyed = RemoteHarness::new(
            Provider::OpenAi,
            "https://api.openai.com/v1/chat/completions",
            Some(Secret::new("k").unwrap()),
            &body,
        )
        .unwrap();
        assert!(keyed.available().await);
        // Keyed providers cannot be built without a key, so only a keyed one is ever asked.
        assert!(
            RemoteHarness::new(
                Provider::OpenAi,
                "https://api.openai.com/v1/chat/completions",
                None,
                &body,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn each_frame_counts_on_top_of_the_text_already_sent() {
        // One byte, then 200 bytes of markers: 201 is over a 200 limit.
        let body = transport(&format!("{}{}", frame("x"), frame(&"k".repeat(20))));
        let seen = Mutex::new(String::new());
        let chat = |limit: usize| {
            let body = &body;
            let seen = &seen;
            async move {
                RemoteHarness::new(
                    Provider::Compatible,
                    "http://127.0.0.1:9/v1",
                    Some(Secret::new("k").unwrap()),
                    body,
                )
                .unwrap()
                .with_limit(limit)
                .unwrap()
                .chat(&input(), &CancellationToken::new(), &mut |text| {
                    seen.lock().unwrap().push_str(text);
                    Ok(())
                })
                .await
            }
        };
        assert!(matches!(chat(200).await, Err(HarnessError::Limit)));
        // The over-limit frame never reaches the sink.
        assert_eq!(std::mem::take(&mut *seen.lock().unwrap()), "x");
        assert_eq!(
            chat(201).await.unwrap(),
            format!("x{}", "[REDACTED]".repeat(20))
        );
    }

    #[tokio::test]
    async fn the_native_transport_resolves_localhost_to_loopback_itself() {
        use rigspark_runtime::harness::{NativeRemoteTransport, RemoteRequest, RemoteTransport};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 4096];
            let _ = socket.read(&mut buffer).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        });
        let response = NativeRemoteTransport::new()
            .unwrap()
            .send(RemoteRequest {
                url: format!("http://localhost:{port}/v1").parse().unwrap(),
                body: serde_json::json!({}),
                headers: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        let mut text = String::new();
        let mut reader = response.body;
        reader.read_to_string(&mut text).await.unwrap();
        assert_eq!(text, "ok");
        server.await.unwrap();
    }

    async fn frames(body: &str, maximum: usize) -> Result<Vec<String>, HarnessError> {
        let mut seen = Vec::new();
        consume_sse(
            Box::pin(std::io::Cursor::new(body.as_bytes().to_vec())),
            maximum,
            &mut |frame| {
                seen.push(frame.to_owned());
                Ok(())
            },
        )
        .await
        .map(|()| seen)
    }

    #[tokio::test]
    async fn sse_streams_are_bounded_and_flush_a_final_unterminated_frame() {
        assert_eq!(
            frames("data: a\n\ndata: b", 100).await.unwrap(),
            ["data: a\n", "data: b"]
        );
        assert_eq!(frames("data: a\n", 100).await.unwrap(), ["data: a\n"]);
        assert!(frames("", 100).await.unwrap().is_empty());
        assert!(frames("\n\n", 100).await.unwrap().is_empty());
        let body = "data: a\n\n";
        assert!(frames(body, body.len()).await.is_ok());
        assert!(matches!(
            frames(body, body.len() - 1).await,
            Err(HarnessError::Limit)
        ));
        // One frame may hold 1 MiB of lines; an unterminated line may also reach 1 MiB.
        let mib = 1024 * 1024;
        let line = |size: usize| format!("{}\n", "x".repeat(size - 1));
        assert!(frames(&line(mib), 4 * mib).await.is_ok());
        assert!(matches!(
            frames(&line(mib + 1), 4 * mib).await,
            Err(HarnessError::Limit)
        ));
        assert!(frames(&"x".repeat(mib), 4 * mib).await.is_ok());
        assert!(matches!(
            frames(&"x".repeat(mib + 1), 4 * mib).await,
            Err(HarnessError::Limit)
        ));
    }

    #[tokio::test]
    async fn claude_text_blocks_are_read_from_whole_messages_too() {
        let body = transport(
            "data: {\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"},{\"type\":\"tool_use\",\"text\":\"no\"}]}}\n\n",
        );
        let harness = RemoteHarness::new(
            Provider::Claude,
            "https://api.anthropic.com/v1/messages",
            Some(Secret::new("k").unwrap()),
            &body,
        )
        .unwrap();
        assert_eq!(
            harness
                .chat(&input(), &CancellationToken::new(), &mut |_| Ok(()))
                .await
                .unwrap(),
            "hi"
        );
    }
}
