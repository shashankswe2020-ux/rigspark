use rigspark_runtime::{
    bench::{
        BenchError, BenchOptions, Clock, FIXTURE_VERSION, Fixture, Protocol, Stat, render_text, run,
    },
    http::{HttpError, Request, Response, Transport},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Barrier;
use tokio_util::sync::CancellationToken;

const ENDPOINT: &str = "http://127.0.0.1:11434";

#[derive(Clone, Default)]
struct FakeClock(Arc<AtomicU64>);
impl FakeClock {
    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms * 1_000_000, Ordering::SeqCst);
    }
}
impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::SeqCst))
    }
}

struct FakeTransport {
    status: u16,
    body: Vec<u8>,
    clock: FakeClock,
    advance_ms: u64,
    barrier: Option<Barrier>,
    requests: Mutex<Vec<(String, Value)>>,
}
impl FakeTransport {
    fn new(body: impl Into<Vec<u8>>, clock: &FakeClock) -> Self {
        Self {
            status: 200,
            body: body.into(),
            clock: clock.clone(),
            advance_ms: 100,
            barrier: None,
            requests: Mutex::new(Vec::new()),
        }
    }
    fn requests(&self) -> Vec<(String, Value)> {
        self.requests.lock().unwrap().clone()
    }
}
#[async_trait::async_trait]
impl Transport for FakeTransport {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        self.requests.lock().unwrap().push((
            request.url.path().to_owned(),
            request.body().cloned().unwrap_or(Value::Null),
        ));
        self.clock.advance(self.advance_ms);
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        Ok(Response {
            status: self.status,
            body: Box::pin(std::io::Cursor::new(self.body.clone())),
        })
    }
}

fn ndjson(lines: &[Value]) -> Vec<u8> {
    lines
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>()
        .into_bytes()
}

fn ollama_body() -> Vec<u8> {
    ndjson(&[
        json!({"response": "", "done": false}),
        json!({"response": "Carrot", "done": false}),
        json!({"response": "", "done": true, "prompt_eval_count": 100,
            "prompt_eval_duration": 500_000_000u64, "eval_count": 50,
            "eval_duration": 1_000_000_000u64}),
    ])
}

fn options(iterations: u8, warmup: u8, concurrency: u8) -> BenchOptions {
    BenchOptions {
        iterations,
        warmup,
        concurrency,
        ..BenchOptions::default()
    }
}

async fn bench(
    transport: &FakeTransport,
    clock: &FakeClock,
    protocol: Protocol,
    options: &BenchOptions,
) -> Result<rigspark_runtime::bench::BenchReport, BenchError> {
    run(
        transport,
        clock,
        ENDPOINT,
        "qwen3:4b",
        protocol,
        options,
        &CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn ollama_counters_produce_runtime_reported_throughput() {
    let clock = FakeClock::default();
    let transport = FakeTransport::new(ollama_body(), &clock);
    let report = bench(&transport, &clock, Protocol::Ollama, &options(3, 0, 1))
        .await
        .unwrap();
    assert_eq!(report.samples.len(), 3);
    let sample = &report.samples[0];
    assert_eq!(sample.prompt_tokens, Some(100));
    assert_eq!(sample.generated_tokens, Some(50));
    assert_eq!(sample.prefill_tps, Some(200.0));
    assert_eq!(sample.decode_tps, Some(50.0));
    assert_eq!(sample.ttft_ms, Some(100.0));
    assert_eq!(sample.wall_ms, 100.0);
    assert_eq!(report.prefill_tps.as_ref().unwrap().median, 200.0);
    assert_eq!(report.decode_tps.as_ref().unwrap().n, 3);
    assert_eq!(report.draft_acceptance, None);
    assert_eq!(report.aggregate_decode_tps, None);

    let requests = transport.requests();
    assert_eq!(requests[0].0, "/api/generate");
    let body = &requests[0].1;
    assert_eq!(body["model"], "qwen3:4b");
    assert_eq!(body["stream"], true);
    assert_eq!(body["options"]["num_predict"], 128);
    assert_eq!(body["options"]["temperature"], 0);
    assert_eq!(body["options"]["seed"], 42);
    assert!(
        body["prompt"]
            .as_str()
            .unwrap()
            .ends_with(Fixture::Short.text())
    );

    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["protocol"], "ollama");
    assert_eq!(json["fixture"]["version"], FIXTURE_VERSION);
    assert_eq!(json["fixture"]["sha256"], Fixture::Short.sha256());
    assert!(json["samples"][0]["ttftMs"].is_number());
    assert!(json["options"]["outputTokens"].is_number());
}

#[tokio::test]
async fn ollama_thinking_counts_as_first_token() {
    let clock = FakeClock::default();
    let body = ndjson(&[
        json!({"thinking": "hmm", "response": "", "done": false}),
        json!({"done": true}),
    ]);
    let transport = FakeTransport::new(body, &clock);
    let report = bench(&transport, &clock, Protocol::Ollama, &options(1, 0, 1))
        .await
        .unwrap();
    assert_eq!(report.samples[0].ttft_ms, Some(100.0));
}

#[tokio::test]
async fn llamacpp_sse_timings_and_draft_acceptance() {
    let clock = FakeClock::default();
    let body = ": ping\n\nevent: message\ndata: {\"content\":\"Hi\",\"stop\":false}\n\n\
data: {\"content\":\"\",\"stop\":true,\"timings\":{\"prompt_n\":40,\"prompt_ms\":100.0,\
\"predicted_n\":64,\"predicted_ms\":2000.0,\"draft_n\":20,\"draft_n_accepted\":15}}\n\n\
data: [DONE]\n\n";
    let transport = FakeTransport::new(body, &clock);
    let report = bench(&transport, &clock, Protocol::LlamaCpp, &options(2, 1, 1))
        .await
        .unwrap();
    let sample = &report.samples[0];
    assert_eq!(sample.prompt_tokens, Some(40));
    assert_eq!(sample.generated_tokens, Some(64));
    assert_eq!(sample.prefill_tps, Some(400.0));
    assert_eq!(sample.decode_tps, Some(32.0));
    assert_eq!(sample.draft_tokens, Some(20));
    assert_eq!(sample.draft_accepted, Some(15));
    assert_eq!(sample.ttft_ms, Some(100.0));
    assert_eq!(report.draft_acceptance, Some(0.75));
    assert!(render_text(&report).contains("75.0%"));

    let requests = transport.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].0, "/completion");
    let body = &requests[0].1;
    assert_eq!(body["n_predict"], 128);
    assert_eq!(body["stream"], true);
    assert_eq!(body["cache_prompt"], false);
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["seed"], 42);
    assert!(body.get("model").is_none());
}

#[tokio::test]
async fn missing_counters_are_unknown_not_fabricated() {
    let clock = FakeClock::default();
    let transport = FakeTransport::new(ndjson(&[json!({"done": true})]), &clock);
    let report = bench(&transport, &clock, Protocol::Ollama, &options(2, 0, 2))
        .await
        .unwrap();
    let sample = &report.samples[0];
    assert_eq!(sample.ttft_ms, None);
    assert_eq!(sample.prefill_tps, None);
    assert_eq!(sample.decode_tps, None);
    assert_eq!(sample.generated_tokens, None);
    assert_eq!(report.ttft_ms, None);
    assert_eq!(report.prefill_tps, None);
    assert_eq!(report.decode_tps, None);
    assert_eq!(report.aggregate_decode_tps, None);
    let text = render_text(&report);
    assert!(text.contains("unknown (runtime did not report prompt timing)"));
    assert!(text.contains("unknown (runtime did not report generation timing)"));
    assert!(text.contains("unknown (runtime did not stream content)"));
    assert!(text.contains("unknown (runtime did not report speculative draft counters)"));

    let clock = FakeClock::default();
    let sse = "data: {\"content\":\"x\",\"stop\":false}\ndata: {\"stop\":true}\n";
    let transport = FakeTransport::new(sse, &clock);
    let report = bench(&transport, &clock, Protocol::LlamaCpp, &options(1, 0, 1))
        .await
        .unwrap();
    assert_eq!(report.decode_tps, None);
    assert_eq!(report.draft_acceptance, None);
    assert!(report.ttft_ms.is_some());
}

#[tokio::test]
async fn zero_durations_are_unknown() {
    let clock = FakeClock::default();
    let body = ndjson(&[json!({"done": true, "prompt_eval_count": 10,
        "prompt_eval_duration": 0, "eval_count": 5, "eval_duration": 0})]);
    let transport = FakeTransport::new(body, &clock);
    let report = bench(&transport, &clock, Protocol::Ollama, &options(1, 0, 1))
        .await
        .unwrap();
    assert_eq!(report.samples[0].prompt_tokens, Some(10));
    assert_eq!(report.prefill_tps, None);
    assert_eq!(report.decode_tps, None);
}

#[tokio::test]
async fn warmup_rounds_are_excluded() {
    let clock = FakeClock::default();
    let transport = FakeTransport::new(ollama_body(), &clock);
    let report = bench(&transport, &clock, Protocol::Ollama, &options(2, 3, 1))
        .await
        .unwrap();
    assert_eq!(transport.requests().len(), 5);
    assert_eq!(report.samples.len(), 2);
    assert_eq!(report.decode_tps.unwrap().n, 2);
}

#[tokio::test]
async fn concurrency_issues_parallel_requests_and_aggregates_per_round() {
    let clock = FakeClock::default();
    let mut transport = FakeTransport::new(ollama_body(), &clock);
    // A 3-party barrier deadlocks unless all three requests are in flight together.
    transport.barrier = Some(Barrier::new(3));
    let report = tokio::time::timeout(
        Duration::from_secs(10),
        bench(&transport, &clock, Protocol::Ollama, &options(2, 1, 3)),
    )
    .await
    .expect("requests were not concurrent")
    .unwrap();
    assert_eq!(transport.requests().len(), 9);
    assert_eq!(report.samples.len(), 6);
    let aggregate = report.aggregate_decode_tps.clone().unwrap();
    assert_eq!(aggregate.n, 2);
    // 3 × 50 generated tokens over a 300 ms round.
    assert!((aggregate.median - 500.0).abs() < 1e-9);
    assert!(render_text(&report).contains("Aggregate (wall clock):"));
}

#[test]
fn percentiles_use_nearest_rank_and_true_median() {
    let values: Vec<f64> = (1..=10).rev().map(f64::from).collect();
    let stat = Stat::from_samples(&values).unwrap();
    assert_eq!(
        (stat.median, stat.p10, stat.p90, stat.n),
        (5.5, 1.0, 9.0, 10)
    );
    let stat = Stat::from_samples(&[3.0, 1.0, 2.0]).unwrap();
    assert_eq!(
        (stat.median, stat.p10, stat.p90, stat.n),
        (2.0, 1.0, 3.0, 3)
    );
    let stat = Stat::from_samples(&[7.0]).unwrap();
    assert_eq!((stat.median, stat.p10, stat.p90), (7.0, 7.0, 7.0));
    assert_eq!(Stat::from_samples(&[]), None);
}

#[test]
fn options_validation_bounds() {
    assert!(BenchOptions::default().validate().is_ok());
    let base = BenchOptions::default();
    assert_eq!(
        (
            base.iterations,
            base.warmup,
            base.output_tokens,
            base.concurrency
        ),
        (5, 1, 128, 1)
    );
    assert_eq!(base.fixture, Fixture::Short);
    let invalid = [
        BenchOptions {
            iterations: 0,
            ..base.clone()
        },
        BenchOptions {
            iterations: 21,
            ..base.clone()
        },
        BenchOptions {
            warmup: 6,
            ..base.clone()
        },
        BenchOptions {
            output_tokens: 15,
            ..base.clone()
        },
        BenchOptions {
            output_tokens: 2049,
            ..base.clone()
        },
        BenchOptions {
            concurrency: 0,
            ..base.clone()
        },
        BenchOptions {
            concurrency: 17,
            ..base.clone()
        },
    ];
    for options in invalid {
        assert!(matches!(
            options.validate(),
            Err(BenchError::InvalidOptions(_))
        ));
    }
    let edges = BenchOptions {
        iterations: 20,
        warmup: 0,
        output_tokens: 2048,
        concurrency: 16,
        fixture: Fixture::Long,
    };
    assert!(edges.validate().is_ok());
}

#[test]
fn backend_protocol_mapping() {
    assert_eq!(Protocol::for_backend("ollama").unwrap(), Protocol::Ollama);
    assert_eq!(
        Protocol::for_backend("llamacpp").unwrap(),
        Protocol::LlamaCpp
    );
    assert_eq!(Protocol::for_backend("bitnet").unwrap(), Protocol::LlamaCpp);
    assert!(matches!(
        Protocol::for_backend("mlx"),
        Err(BenchError::Unsupported(name)) if name == "mlx"
    ));
}

#[tokio::test]
async fn non_success_status_is_an_error() {
    let clock = FakeClock::default();
    let mut transport = FakeTransport::new(ollama_body(), &clock);
    transport.status = 503;
    let error = bench(&transport, &clock, Protocol::Ollama, &options(1, 0, 1))
        .await
        .unwrap_err();
    assert!(matches!(error, BenchError::Status(503)));
}

#[tokio::test]
async fn oversize_line_hits_limit() {
    let clock = FakeClock::default();
    let mut body = vec![b'a'; 70_000];
    body.push(b'\n');
    let transport = FakeTransport::new(body, &clock);
    let error = bench(&transport, &clock, Protocol::Ollama, &options(1, 0, 1))
        .await
        .unwrap_err();
    assert!(matches!(error, BenchError::Limit));
}

#[tokio::test]
async fn runtime_errors_malformed_and_truncated_streams_fail() {
    let cases: [(Protocol, Vec<u8>); 5] = [
        (
            Protocol::Ollama,
            ndjson(&[json!({"error": "model not found"})]),
        ),
        (
            Protocol::Ollama,
            ndjson(&[json!({"done": true, "eval_count": -1})]),
        ),
        (
            Protocol::Ollama,
            ndjson(&[json!({"response": "x", "done": false})]),
        ),
        (
            Protocol::LlamaCpp,
            b"data: {\"stop\":true,\"timings\":{\"prompt_ms\":\"NaN\"}}\n".to_vec(),
        ),
        (
            Protocol::LlamaCpp,
            b"data: {\"stop\":true,\"timings\":{\"draft_n\":2,\"draft_n_accepted\":3}}\n".to_vec(),
        ),
    ];
    for (index, (protocol, body)) in cases.into_iter().enumerate() {
        let clock = FakeClock::default();
        let transport = FakeTransport::new(body, &clock);
        let error = bench(&transport, &clock, protocol, &options(1, 0, 1))
            .await
            .unwrap_err();
        if index == 0 {
            assert!(matches!(error, BenchError::Runtime(ref m) if m == "model not found"));
        } else {
            assert!(
                matches!(error, BenchError::Response),
                "case {index}: {error:?}"
            );
        }
    }
}

#[tokio::test]
async fn nonce_differs_per_request() {
    let clock = FakeClock::default();
    let transport = FakeTransport::new(ollama_body(), &clock);
    bench(&transport, &clock, Protocol::Ollama, &options(3, 1, 2))
        .await
        .unwrap();
    let prompts: Vec<String> = transport
        .requests()
        .iter()
        .map(|(_, body)| body["prompt"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(prompts.len(), 8);
    let unique: std::collections::BTreeSet<_> = prompts.iter().collect();
    assert_eq!(unique.len(), prompts.len());
    assert!(prompts[0].starts_with("Benchmark request 0-0.\n"));
    assert!(prompts[1].starts_with("Benchmark request 0-1.\n"));
}

#[tokio::test]
async fn cancellation_and_invalid_inputs_are_rejected() {
    let clock = FakeClock::default();
    let transport = FakeTransport::new(ollama_body(), &clock);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let opts = options(1, 0, 1);
    let error = run(
        &transport,
        &clock,
        ENDPOINT,
        "m",
        Protocol::Ollama,
        &opts,
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, BenchError::Cancelled));
    let live = CancellationToken::new();
    for (endpoint, model) in [
        ("http://example.com:11434", "m"),
        (ENDPOINT, ""),
        (ENDPOINT, "bad\nmodel"),
    ] {
        let error = run(
            &transport,
            &clock,
            endpoint,
            model,
            Protocol::Ollama,
            &opts,
            &live,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, BenchError::InvalidRequest));
    }
    assert!(transport.requests().is_empty());
}

#[test]
fn fixtures_are_versioned_and_stable() {
    assert_eq!(FIXTURE_VERSION, 1);
    assert_eq!(Fixture::Short.id(), "short");
    assert_eq!(Fixture::Long.id(), "long");
    for fixture in [Fixture::Short, Fixture::Long] {
        let sha = fixture.sha256();
        assert_eq!(sha.len(), 64);
        assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(sha, fixture.sha256());
    }
    assert_ne!(Fixture::Short.sha256(), Fixture::Long.sha256());
    // Pin the bytes: any edit to a fixture must bump FIXTURE_VERSION.
    assert_eq!(Fixture::Short.sha256(), SHORT_SHA256);
    assert_eq!(Fixture::Long.sha256(), LONG_SHA256);
    // Roughly 1500–2000 tokens at ~4 bytes per token.
    let long = Fixture::Long.text().len();
    assert!(
        (6_000..=8_500).contains(&long),
        "long fixture is {long} bytes"
    );
}

const SHORT_SHA256: &str = "7f3951fab5d5316889c1cf4b240c2dae4b483b0d6c9211ff27270726493323ff";
const LONG_SHA256: &str = "24c64e147b17793b758cbcbe1bec52abebe036fe7551ab64d30a78d0a36baafb";
