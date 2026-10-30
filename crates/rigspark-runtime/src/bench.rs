//! Active-mode runtime benchmark engine.
//!
//! Pure and injectable: the HTTP [`Transport`] and monotonic [`Clock`] are
//! supplied by the caller so tests never touch real runtimes. Metrics the
//! runtime does not report stay `None`; nothing is derived across runtimes.

use crate::http::{HttpError, Request, Transport};
use futures_util::future::try_join_all;
use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_util::sync::CancellationToken;

pub const FIXTURE_VERSION: u32 = 1;

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_COUNT: u64 = 1_000_000_000;
const MAX_MILLIS: f64 = 1.0e10;
const MAX_NANOS: u64 = 10_000_000_000_000_000;
const MAX_ERROR_CHARS: usize = 256;
const SEED: u64 = 42;

const SHORT_TEXT: &str = "You are taking part in a fixed performance benchmark. \
Reply with exactly five numbered lines. Each line must name one common garden \
vegetable followed by a single short sentence describing its colour. Do not add \
any introduction, conclusion, or extra commentary.\n";

macro_rules! long_paragraph {
    () => {
        "The keeper of the harbour light kept a ledger of small and ordinary things. \
Each morning she wrote down the direction of the wind, the colour of the water near \
the breakwater, the number of gulls resting on the old stone pier, and whether the \
fishing boats had left before or after the bell in the square rang six times. She did \
not believe the ledger was important to anyone else, yet she kept it with great care, \
because a careful record of quiet days made the unusual days easier to recognise. When \
the fog arrived early, she compared it with earlier pages and noticed patterns in the \
tides, the temperature of the air, and the way the lamp oil thickened in the cold. Over \
many seasons the pages became a patient map of the coast, drawn not with lines and \
coordinates but with plain sentences, steady handwriting, and the habit of looking at \
the same horizon every single day without growing tired of it.\n\n"
    };
}

const LONG_TEXT: &str = concat!(
    "You are taking part in a fixed performance benchmark. Read the following \
passage carefully. Afterwards, write a numbered list of exactly five short \
observations about the habits of the keeper described in the passage. Do not \
quote the passage and do not add any introduction or conclusion.\n\n",
    "Section 1. ",
    long_paragraph!(),
    "Section 2. ",
    long_paragraph!(),
    "Section 3. ",
    long_paragraph!(),
    "Section 4. ",
    long_paragraph!(),
    "Section 5. ",
    long_paragraph!(),
    "Section 6. ",
    long_paragraph!(),
    "Section 7. ",
    long_paragraph!(),
    "Section 8. ",
    long_paragraph!(),
    "Now write the five numbered observations.\n",
);

#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    #[error("benchmarking is not supported for backend `{0}`")]
    Unsupported(String),
    #[error("invalid benchmark options: {0}")]
    InvalidOptions(&'static str),
    #[error("invalid benchmark request")]
    InvalidRequest,
    #[error("local runtime HTTP {0}")]
    Status(u16),
    #[error("local runtime transport failed")]
    Transport,
    #[error("local runtime response exceeds limit")]
    Limit,
    #[error("invalid local runtime benchmark response")]
    Response,
    #[error("local runtime reported an error: {0}")]
    Runtime(String),
    #[error("benchmark cancelled")]
    Cancelled,
    #[error("benchmark request timed out")]
    Timeout,
}

impl From<HttpError> for BenchError {
    fn from(error: HttpError) -> Self {
        match error {
            HttpError::Invalid => Self::InvalidRequest,
            HttpError::Status(status) => Self::Status(status),
            HttpError::Transport => Self::Transport,
            HttpError::Limit => Self::Limit,
            HttpError::Response => Self::Response,
            HttpError::Cancelled => Self::Cancelled,
            HttpError::Timeout => Self::Timeout,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fixture {
    #[default]
    Short,
    Long,
}

impl Fixture {
    pub fn id(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Long => "long",
        }
    }
    pub fn text(self) -> &'static str {
        match self {
            Self::Short => SHORT_TEXT,
            Self::Long => LONG_TEXT,
        }
    }
    pub fn sha256(self) -> String {
        hex(&Sha256::digest(self.text().as_bytes()))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Ollama,
    #[serde(rename = "llamacpp")]
    LlamaCpp,
}

impl Protocol {
    pub fn for_backend(backend: &str) -> Result<Self, BenchError> {
        match backend {
            "ollama" => Ok(Self::Ollama),
            "llamacpp" | "bitnet" => Ok(Self::LlamaCpp),
            other => Err(BenchError::Unsupported(sanitize(other, 64))),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::LlamaCpp => "llama.cpp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchOptions {
    pub iterations: u8,
    pub warmup: u8,
    pub output_tokens: u32,
    pub concurrency: u8,
    pub fixture: Fixture,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self {
            iterations: 5,
            warmup: 1,
            output_tokens: 128,
            concurrency: 1,
            fixture: Fixture::Short,
        }
    }
}

impl BenchOptions {
    pub fn validate(&self) -> Result<(), BenchError> {
        if !(1..=20).contains(&self.iterations) {
            return Err(BenchError::InvalidOptions("iterations must be 1..=20"));
        }
        if self.warmup > 5 {
            return Err(BenchError::InvalidOptions("warmup must be 0..=5"));
        }
        if !(16..=2048).contains(&self.output_tokens) {
            return Err(BenchError::InvalidOptions(
                "output tokens must be 16..=2048",
            ));
        }
        if !(1..=16).contains(&self.concurrency) {
            return Err(BenchError::InvalidOptions("concurrency must be 1..=16"));
        }
        Ok(())
    }
}

/// Monotonic clock measured from an arbitrary fixed origin.
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSample {
    /// Wall clock from request dispatch to first streamed content.
    pub ttft_ms: Option<f64>,
    /// Wall clock from request dispatch to the terminal event.
    pub wall_ms: f64,
    pub prompt_tokens: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub prefill_tps: Option<f64>,
    pub decode_tps: Option<f64>,
    pub draft_tokens: Option<u64>,
    pub draft_accepted: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub median: f64,
    pub p10: f64,
    pub p90: f64,
    pub n: usize,
}

impl Stat {
    /// Median is the middle value (mean of the two middle values for even n);
    /// p10/p90 use nearest rank: index `clamp(ceil(p * n) - 1, 0, n - 1)`.
    pub fn from_samples(values: &[f64]) -> Option<Self> {
        let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let median = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };
        Some(Self {
            median,
            p10: nearest_rank(&sorted, 10),
            p90: nearest_rank(&sorted, 90),
            n,
        })
    }
}

fn nearest_rank(sorted: &[f64], percent: usize) -> f64 {
    let n = sorted.len();
    let rank = (percent * n).div_ceil(100);
    sorted[rank.saturating_sub(1).min(n - 1)]
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureInfo {
    pub id: &'static str,
    pub version: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchReport {
    pub protocol: Protocol,
    pub model: String,
    pub fixture: FixtureInfo,
    pub options: BenchOptions,
    /// Wall clock, measured.
    pub ttft_ms: Option<Stat>,
    /// Runtime reported.
    pub prefill_tps: Option<Stat>,
    /// Runtime reported.
    pub decode_tps: Option<Stat>,
    /// Sum of accepted over sum of drafted tokens across measured runs.
    pub draft_acceptance: Option<f64>,
    /// Wall clock, measured per round; only computed when concurrency > 1.
    pub aggregate_decode_tps: Option<Stat>,
    pub samples: Vec<RunSample>,
}

fn sanitize(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && !model.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn nonce(round: u32, slot: u8) -> String {
    format!("Benchmark request {round}-{slot}.\n")
}

fn request_body(
    protocol: Protocol,
    model: &str,
    prompt: &str,
    options: &BenchOptions,
) -> (&'static str, Value) {
    match protocol {
        Protocol::Ollama => (
            "/api/generate",
            json!({
                "model": model,
                "prompt": prompt,
                "stream": true,
                "options": {
                    "num_predict": options.output_tokens,
                    "temperature": 0,
                    "seed": SEED,
                },
            }),
        ),
        Protocol::LlamaCpp => (
            "/completion",
            json!({
                "prompt": prompt,
                "n_predict": options.output_tokens,
                "stream": true,
                "cache_prompt": false,
                "temperature": 0,
                "seed": SEED,
            }),
        ),
    }
}

fn count(object: &Map<String, Value>, key: &str) -> Result<Option<u64>, BenchError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|n| *n <= MAX_COUNT)
            .map(Some)
            .ok_or(BenchError::Response),
    }
}

fn nanos(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, BenchError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|n| *n <= MAX_NANOS)
            .map(|n| Some(n as f64 / 1.0e9))
            .ok_or(BenchError::Response),
    }
}

fn millis(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, BenchError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|ms| ms.is_finite() && (0.0..=MAX_MILLIS).contains(ms))
            .map(|ms| Some(ms / 1000.0))
            .ok_or(BenchError::Response),
    }
}

fn rate(tokens: Option<u64>, seconds: Option<f64>) -> Option<f64> {
    let (tokens, seconds) = (tokens?, seconds?);
    if tokens == 0 || !(seconds.is_finite() && seconds > 0.0) {
        return None;
    }
    Some(tokens as f64 / seconds).filter(|tps| tps.is_finite() && *tps > 0.0)
}

#[derive(Default)]
struct Counters {
    prompt_tokens: Option<u64>,
    generated_tokens: Option<u64>,
    prefill_tps: Option<f64>,
    decode_tps: Option<f64>,
    draft_tokens: Option<u64>,
    draft_accepted: Option<u64>,
}

enum Event {
    Skip,
    Content,
    Final(Counters),
}

fn runtime_error(value: &Value) -> Option<BenchError> {
    let error = value.get("error")?;
    let message = match error {
        Value::Null => return None,
        Value::String(text) => text.as_str(),
        Value::Object(object) => object
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error"),
        _ => "unknown error",
    };
    Some(BenchError::Runtime(sanitize(message, MAX_ERROR_CHARS)))
}

fn non_empty_str(object: &Map<String, Value>, key: &str) -> Result<bool, BenchError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::String(text)) => Ok(!text.is_empty()),
        Some(_) => Err(BenchError::Response),
    }
}

fn flag(object: &Map<String, Value>, key: &str) -> Result<bool, BenchError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(BenchError::Response),
    }
}

fn ollama_event(line: &[u8]) -> Result<Event, BenchError> {
    let value: Value = serde_json::from_slice(line).map_err(|_| BenchError::Response)?;
    if let Some(error) = runtime_error(&value) {
        return Err(error);
    }
    let object = value.as_object().ok_or(BenchError::Response)?;
    if flag(object, "done")? {
        let prompt_tokens = count(object, "prompt_eval_count")?;
        let generated_tokens = count(object, "eval_count")?;
        return Ok(Event::Final(Counters {
            prompt_tokens,
            generated_tokens,
            prefill_tps: rate(prompt_tokens, nanos(object, "prompt_eval_duration")?),
            decode_tps: rate(generated_tokens, nanos(object, "eval_duration")?),
            draft_tokens: None,
            draft_accepted: None,
        }));
    }
    if non_empty_str(object, "response")? || non_empty_str(object, "thinking")? {
        return Ok(Event::Content);
    }
    Ok(Event::Skip)
}

fn llamacpp_event(line: &[u8]) -> Result<Event, BenchError> {
    let Some(payload) = line.strip_prefix(b"data:") else {
        // Blank lines, `:` comments and other SSE fields (event:, id:) carry no data.
        return Ok(Event::Skip);
    };
    let payload = payload.strip_prefix(b" ").unwrap_or(payload);
    if payload == b"[DONE]" {
        return Ok(Event::Skip);
    }
    let value: Value = serde_json::from_slice(payload).map_err(|_| BenchError::Response)?;
    if let Some(error) = runtime_error(&value) {
        return Err(error);
    }
    let object = value.as_object().ok_or(BenchError::Response)?;
    if flag(object, "stop")? {
        let Some(timings) = object.get("timings").filter(|v| !v.is_null()) else {
            return Ok(Event::Final(Counters::default()));
        };
        let timings = timings.as_object().ok_or(BenchError::Response)?;
        let prompt_tokens = count(timings, "prompt_n")?;
        let generated_tokens = count(timings, "predicted_n")?;
        let draft_tokens = count(timings, "draft_n")?;
        let draft_accepted = count(timings, "draft_n_accepted")?;
        if let (Some(drafted), Some(accepted)) = (draft_tokens, draft_accepted)
            && accepted > drafted
        {
            return Err(BenchError::Response);
        }
        return Ok(Event::Final(Counters {
            prompt_tokens,
            generated_tokens,
            prefill_tps: rate(prompt_tokens, millis(timings, "prompt_ms")?),
            decode_tps: rate(generated_tokens, millis(timings, "predicted_ms")?),
            draft_tokens,
            draft_accepted,
        }));
    }
    if non_empty_str(object, "content")? {
        return Ok(Event::Content);
    }
    Ok(Event::Skip)
}

fn elapsed_ms(clock: &dyn Clock, start: Duration) -> f64 {
    clock.now().saturating_sub(start).as_secs_f64() * 1000.0
}

#[allow(clippy::too_many_arguments)]
async fn one_request(
    transport: &dyn Transport,
    clock: &dyn Clock,
    endpoint: &str,
    model: &str,
    protocol: Protocol,
    options: &BenchOptions,
    round: u32,
    slot: u8,
    cancel: &CancellationToken,
) -> Result<RunSample, BenchError> {
    let prompt = format!("{}{}", nonce(round, slot), options.fixture.text());
    let (path, body) = request_body(protocol, model, &prompt, options);
    let request = Request::new(endpoint, path, Some(body), None)?;
    let parse = match protocol {
        Protocol::Ollama => ollama_event,
        Protocol::LlamaCpp => llamacpp_event,
    };
    let operation = async {
        let start = clock.now();
        let response = transport.send(request).await?;
        if !(200..300).contains(&response.status) {
            return Err(BenchError::Status(response.status));
        }
        let mut reader = BufReader::new(response.body);
        let mut total = 0usize;
        let mut ttft_ms = None;
        loop {
            let mut line = Vec::new();
            let read = (&mut reader)
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut line)
                .await
                .map_err(|_| BenchError::Transport)?;
            if read == 0 {
                // EOF before the terminal event is a premature end of stream.
                return Err(BenchError::Response);
            }
            total += read;
            if read > MAX_LINE_BYTES || total > MAX_BODY_BYTES {
                return Err(BenchError::Limit);
            }
            let trimmed = line.trim_ascii();
            if trimmed.is_empty() {
                continue;
            }
            match parse(trimmed)? {
                Event::Skip => {}
                Event::Content => {
                    if ttft_ms.is_none() {
                        ttft_ms = Some(elapsed_ms(clock, start));
                    }
                }
                Event::Final(counters) => {
                    return Ok(RunSample {
                        ttft_ms,
                        wall_ms: elapsed_ms(clock, start),
                        prompt_tokens: counters.prompt_tokens,
                        generated_tokens: counters.generated_tokens,
                        prefill_tps: counters.prefill_tps,
                        decode_tps: counters.decode_tps,
                        draft_tokens: counters.draft_tokens,
                        draft_accepted: counters.draft_accepted,
                    });
                }
            }
        }
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(BenchError::Cancelled),
        result = tokio::time::timeout(REQUEST_TIMEOUT, operation) => {
            result.map_err(|_| BenchError::Timeout)?
        }
    }
}

/// Runs warmup rounds (excluded) then measured rounds against an already
/// running, identity-verified runtime. Each round issues `concurrency`
/// requests concurrently.
pub async fn run(
    transport: &dyn Transport,
    clock: &dyn Clock,
    endpoint: &str,
    model: &str,
    protocol: Protocol,
    options: &BenchOptions,
    cancel: &CancellationToken,
) -> Result<BenchReport, BenchError> {
    options.validate()?;
    if !valid_model(model) {
        return Err(BenchError::InvalidRequest);
    }
    Request::new(endpoint, "/", None, None)?;
    let rounds = u32::from(options.warmup) + u32::from(options.iterations);
    let mut samples =
        Vec::with_capacity(usize::from(options.iterations) * usize::from(options.concurrency));
    let mut round_rates = Vec::new();
    for round in 0..rounds {
        if cancel.is_cancelled() {
            return Err(BenchError::Cancelled);
        }
        let round_start = clock.now();
        let batch = try_join_all((0..options.concurrency).map(|slot| {
            one_request(
                transport, clock, endpoint, model, protocol, options, round, slot, cancel,
            )
        }))
        .await?;
        let round_seconds = clock.now().saturating_sub(round_start).as_secs_f64();
        if round < u32::from(options.warmup) {
            continue;
        }
        if options.concurrency > 1 {
            let generated: Option<u64> = batch.iter().map(|s| s.generated_tokens).sum();
            if let Some(rate) = rate(generated, Some(round_seconds)) {
                round_rates.push(rate);
            }
        }
        samples.extend(batch);
    }
    let collect = |pick: fn(&RunSample) -> Option<f64>| {
        Stat::from_samples(&samples.iter().filter_map(pick).collect::<Vec<_>>())
    };
    let (drafted, accepted) = samples
        .iter()
        .filter_map(|s| Some((s.draft_tokens?, s.draft_accepted?)))
        .fold((0u64, 0u64), |(d, a), (sd, sa)| {
            (d.saturating_add(sd), a.saturating_add(sa))
        });
    let draft_acceptance = (drafted > 0).then(|| accepted as f64 / drafted as f64);
    Ok(BenchReport {
        protocol,
        model: model.to_owned(),
        fixture: FixtureInfo {
            id: options.fixture.id(),
            version: FIXTURE_VERSION,
            sha256: options.fixture.sha256(),
        },
        options: options.clone(),
        ttft_ms: collect(|s| s.ttft_ms),
        prefill_tps: collect(|s| s.prefill_tps),
        decode_tps: collect(|s| s.decode_tps),
        draft_acceptance,
        aggregate_decode_tps: Stat::from_samples(&round_rates),
        samples,
    })
}

fn stat_line(out: &mut String, label: &str, stat: Option<&Stat>, unit: &str, missing: &str) {
    let _ = match stat {
        Some(stat) => writeln!(
            out,
            "{label:<22} {:.1} {unit} (p10 {:.1} · p90 {:.1}, n={})",
            stat.median, stat.p10, stat.p90, stat.n
        ),
        None => writeln!(out, "{label:<22} unknown ({missing})"),
    };
}

pub fn render_text(report: &BenchReport) -> String {
    let mut out = String::new();
    let options = &report.options;
    let _ = writeln!(out, "Runtime: {}", report.protocol.label());
    let _ = writeln!(out, "Model:   {}", sanitize(&report.model, 256));
    let _ = writeln!(
        out,
        "Fixture: {} v{} (sha256 {})",
        report.fixture.id,
        report.fixture.version,
        sanitize(&report.fixture.sha256, 64)
    );
    let _ = writeln!(
        out,
        "Runs:    {} measured after {} warmup · {} output tokens max · concurrency {}",
        options.iterations, options.warmup, options.output_tokens, options.concurrency
    );
    out.push('\n');
    stat_line(
        &mut out,
        "TTFT (wall clock):",
        report.ttft_ms.as_ref(),
        "ms",
        "runtime did not stream content",
    );
    stat_line(
        &mut out,
        "Prefill (runtime):",
        report.prefill_tps.as_ref(),
        "tok/s",
        "runtime did not report prompt timing",
    );
    stat_line(
        &mut out,
        "Decode (runtime):",
        report.decode_tps.as_ref(),
        "tok/s",
        "runtime did not report generation timing",
    );
    let _ = match report.draft_acceptance {
        Some(ratio) => writeln!(out, "{:<22} {:.1}%", "Draft acceptance:", ratio * 100.0),
        None => writeln!(
            out,
            "{:<22} unknown (runtime did not report speculative draft counters)",
            "Draft acceptance:"
        ),
    };
    if options.concurrency > 1 {
        stat_line(
            &mut out,
            "Aggregate (wall clock):",
            report.aggregate_decode_tps.as_ref(),
            "tok/s",
            "runtime did not report generated token counts",
        );
    }
    out
}
