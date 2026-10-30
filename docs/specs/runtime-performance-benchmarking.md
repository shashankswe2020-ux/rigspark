# Spec: Runtime Performance Benchmarking and Evidence-Driven Tuning

> Status: **Draft (v0.3) — architecture, security, and test-strategy feedback incorporated; pending human approval.**
> Last updated: 2026-10-30
> Implementation: active mode ships as `rigspark bench` (`crates/rigspark-runtime/src/bench.rs`) with `--iterations`, `--warmup`, `--output-tokens`, `--concurrency` and `--fixture short|long`. Ephemeral mode, memory sampling and regression baselines remain pending.
> Related: [rigspark.md](./rigspark.md),
> [hardware-advisor.md](./hardware-advisor.md),
> [context-window-sizing.md](./context-window-sizing.md), and
> [pluggable-inference-backends.md](./pluggable-inference-backends.md).
> Design inspiration: Jeff Dean and Sanjay Ghemawat,
> [Performance Hints](https://abseil.io/fast/hints.html) (last updated
> 2025-12-16), especially measurement, bulk APIs, compact memory representation,
> allocation reduction, avoiding unnecessary work, caching, and keeping logging
> off hot paths.

---

## 0. Assumptions and decisions proposed by this draft

This draft proceeds with the following assumptions. They become decisions only
when the spec is approved:

1. The first deliverable is **measurement**, not automatic runtime tuning.
   Optimizations without measured evidence are explicitly out of scope.
2. A new `benchmark <model>` command is a runtime command, not an advice command.
   It may inspect installed binaries and run real local inference, but it never
   changes deterministic `recommend`/`can-run` behavior.
3. Benchmark results do **not** automatically modify `data/perf.json` or
   `data/models.json`. Dataset curation remains reviewed, cited, offline work.
4. The command supports Ollama, llama.cpp, MLX, and future adapters through a
   backend benchmark contract. Unsupported metrics are `unknown`, not inferred.
5. Default execution uses an isolated, temporary, loopback-only server and
   preserves existing active state. An explicit `--mode active` benchmarks the
   currently active matching server without claiming or stopping it.
6. Pulling new weights is disabled by default. `--allow-pull` is required before
   the benchmark may acquire a missing model.
7. v1 measures single-request interactive performance. Concurrent throughput,
   parameter sweeps, and automatic launch-profile selection are follow-up phases.
8. P1 ships both Ollama and llama.cpp support. `ephemeral` is the default,
   measured runs default to 5 after 1 warmup, and verified process-tree memory is
   sampled for estimated output and early cancellation; admission-time reserve
   budgeting and platform hard containment bound the benchmark tree.
9. Exit code 2 is reserved for a conclusive comparable regression. P1 does not
   expose baseline flags or exit 2; P2 adds them without changing success/error
   exit codes.
10. Benchmark evidence is emitted to stdout; users retain it via explicit shell
    redirection. v1 creates no default benchmark-data directory and never writes
    repository datasets.
11. Ollama P1 requires an additive catalog-schema expansion for curated immutable
    manifest and complete blob-set digest/size evidence. Approving this spec
    approves that schema work; catalog execution remains read-only.

---

## 1. Objective

Add a reproducible, runtime-backed benchmark capability that answers:

- How long does this model/runtime take to become ready?
- What are time to first token (TTFT), prompt/prefill throughput, and decode
  throughput?
- How variable are warm runs?
- What memory footprint is observed while serving and generating?
- Did a code/runtime change cause a meaningful regression?
- Which phase is dominant: acquisition, load, prefill, decode, or memory?

The feature operationalizes the Abseil guidance to estimate first, measure the
real hot path, avoid unnecessary work, and optimize inside narrow module
boundaries. It does not attempt to optimize llama.cpp or MLX kernels from the
TypeScript CLI.

### Target users

- Maintainers validating Ollama, llama.cpp, MLX, or future adapters.
- Developers comparing runtimes on the same model and machine.
- Users diagnosing slow startup, slow prompt processing, or slow generation.
- Release engineers detecting performance regressions before publishing.

### Success looks like

```text
$ llmup benchmark qwen3:14b --backend llamacpp
Runtime: llama.cpp  b10090
Model:   qwen3:14b / Q4_K_M
Mode:    ephemeral (127.0.0.1:18180)

Acquire: cache hit, digest verified
Process → API ready: 410 ms
Cold model load:     2.43 s       (runtime reported)
Cold request total:  2.61 s
TTFT median:        118 ms       (p10 111 · p90 132)
Prefill median:     215.4 tok/s   (24 runtime-reported tokens)
Decode median:      31.8 tok/s    (128 requested, 128 generated)
Sampled peak memory: 10.7 GiB     (cgroup memory.current; 250 ms estimate)

Bottleneck: decode
Evidence: 5 measured runs after 1 warmup
```

If the runtime exposes no prefill timing, output is honest:

```text
Prefill: unknown (runtime did not report prompt timing)
```

### Non-goals for v1

- Changing `recommend` throughput from live benchmark results.
- Publishing a global leaderboard from one machine.
- Automatically selecting or applying runtime flags.
- Multi-user continuous-batching optimization.
- Kernel, quantization, or model conversion implementation.
- Benchmarking remote/cloud endpoints.
- Exposing unauthenticated servers beyond loopback.
- Claiming energy measurements without a sourced platform probe.

---

## 2. Design principles

### 2.1 Measure the phases separately

A single tok/s value is insufficient. v1 distinguishes:

1. acquisition/cache verification;
2. process start to backend readiness;
3. TTFT;
4. prompt/prefill evaluation;
5. autoregressive decode;
6. sampled process memory.

### 2.2 Runtime-reported counters are authoritative

Prompt tokens, generated tokens, and prefill/decode duration come from
Zod-validated backend responses. Wall-clock measurement is used for command-level
load and end-to-end latency, and as a cross-check—not to fabricate missing
runtime metrics.

### 2.3 Benchmark fixtures are fixed and versioned

The default prompts live in the repository and carry a fixture version. The
benchmark result records fixture id and SHA-256. This allows comparisons across
commits without embedding arbitrary user text or changing prompts silently.

### 2.4 Avoid unnecessary work

- Warmup runs are excluded from statistics.
- A verified cache hit performs no download.
- One temporary process serves all measured iterations.
- Model/tokenizer/process setup is not repeated per iteration.
- Metrics collection is bounded and sampled; no per-token logging.

### 2.5 Honest uncertainty

A metric is either:

- measured and labeled with its source;
- sampled and labeled as an estimate; or
- `unknown` with a reason.

No cross-runtime conversion factor is invented.

### 2.6 Runtime benchmarking is not offline advice

`benchmark` may probe binaries and run inference. `recommend` and `can-run`
remain deterministic, offline consumers of `data/models.json` and
`data/perf.json`.

---

## 3. Command definition

### 3.1 Command

```text
llmup benchmark <model> [options]
```

Description:

```text
Measure real local runtime load, TTFT, prefill, decode, and memory for <model>.
```

### 3.2 Options

| Option                             | Default                     | Contract                                                                                  |
| ---------------------------------- | --------------------------- | ----------------------------------------------------------------------------------------- |
| `--backend <name>`                 | create-selection precedence | Validated `BackendName`; selects runtime in ephemeral mode                                |
| `--mode <ephemeral\|active>`       | `ephemeral`                 | Ephemeral starts/stops an isolated process; active requires matching active model/backend |
| `--iterations <n>`                 | `5`                         | Integer `3..20`; measured runs                                                            |
| `--warmup <n>`                     | `1`                         | Integer `0..5`; excluded from aggregates                                                  |
| `--output-tokens <n>`              | `128`                       | Integer `16..2048`; maximum requested generation                                          |
| `--fixture <short\|medium\|long>`  | `short`                     | Versioned built-in prompt fixture                                                         |
| `--port <port>`                    | free high port              | Integer `1024..65535`; ephemeral only; must be loopback and free                          |
| `--allow-pull`                     | false                       | Permit verified acquisition when weights are not already cached                           |
| `--baseline <file>`                | none                        | **P2:** descriptor-safe, byte-capped, Zod-validated prior result                          |
| `--regression-threshold <percent>` | `10`                        | **P2:** number `1..100`; relative component of comparison threshold                       |
| `--json`                           | false                       | Emit canonical JSON only                                                                  |

### 3.3 Mode behavior

#### Ephemeral mode

1. Resolve model and quantization from the offline catalog.
2. Filter installed adapters by exact model source/format and mandatory benchmark
   support, then apply create-selection precedence. An explicit incompatible
   backend fails; it never falls through to another backend.
3. Determine the adapter's artifact family. For a self-managed adapter only,
   call offline `inspectArtifact()`; it performs no network request, process spawn,
   daemon request, or cache mutation. A daemon-managed adapter performs no cache
   inspection before its private daemon/store exists.
4. For self-managed artifacts, refuse missing/unverified evidence unless
   `--allow-pull` is present. Daemon-managed refusal/acquisition is deferred to
   step 6 and is scoped exclusively to the private store.
5. Select a candidate loopback port. OS-assigned port 0/socket activation is
   preferred where the runtime supports it. Otherwise the port probe is advisory
   and post-bind PID/listener ownership validation is authoritative.
6. Follow one normative backend flow:

- **Self-managed (llama.cpp/MLX):** inspect → optionally acquire with immutable
  source/digest → obtain verified artifact lease → `BenchmarkLeaseManager.serve()` with
  `existingListener:"reject"` and lease id → verify listener/runtime/artifact.
- **Daemon-managed (Ollama):** acquire a benchmark-private store lease, then
  call `BenchmarkLeaseManager.serve()` with `existingListener:"reject"`, its exact private-
  store lease, containment plan, and no artifact lease → inspect that store → optionally pull only through that
  owned endpoint → obtain/verify a daemon-store artifact lease →
  `loadBenchmarkModel()` through the same endpoint → verify served manifest/
  blob identity. Never use an unrelated active/default daemon or store.

7. Attached handles are invalid in ephemeral mode and cause failure before
   fixture transmission.
8. Measure API readiness, one excluded first request (cold/model-loading in
   ephemeral mode; priming-only in active mode), warmups, and measured inference
   as separate phases.
9. Stop the complete verified process tree owned by this benchmark.
10. Verify all owned process instances and listeners are gone.

If another process owns or steals the requested port, fail closed. A successful
HTTP readiness response never establishes ownership. The adapter must prove that
the exact listener belongs to the spawned process instance (or verified
descendant) before sending fixture data.

Newly acquired ordinary self-managed cache weights remain after the benchmark.
An isolated daemon-private store is deleted only after final lease revalidation,
process-tree shutdown, and listener cleanup. The command reports the applicable
disposition explicitly. Partial/failed acquisitions are removed by the
acquisition path.

#### Active mode

1. Require non-null active state.
2. Require the requested model to equal `active.modelId`.
3. If `--backend` is supplied, require it to equal `active.backend`.
4. Verify endpoint/process/model identity through the active adapter.
5. Skip acquisition and load timing; report both as `not_applicable`. Execute an
   excluded first priming request, but do not label it cold or model-loading.
6. Never stop or mutate the active server.

Snapshot active state at start. Re-read and compare backend, model, endpoint,
ownership, PID, executable, and process-start identity immediately before and
after every first, warmup, and measured request. Abort on drift without restoring
stale state or stopping either old/new active process. Do not hold the global
state lock for the complete benchmark.

Immediately before and after every first, warmup, and measured request, active
mode also calls `inspectServedArtifact()` with the immutable catalog-derived
`ArtifactExpectation` and requires exact source identity, digest, and byte-size
equality. A drift, unsupported proof, transient inspection failure, or result
that relies only on a runtime's model label aborts and discards all samples.
These checks can detect drift but cannot exclude an A→B→A substitution during a
request. Unless a future adapter returns cryptographic artifact identity bound to
that exact response or holds a runtime-enforced request-scoped model lease,
active results record `requestBinding:"pre_post_only"` and are never baseline-
comparable. P1 adapters do not claim such binding. Ephemeral leased execution
records `requestBinding:"leased"`.

### 3.4 Exit-code contract

| Exit | Meaning                                                                                    |
| ---- | ------------------------------------------------------------------------------------------ |
| `0`  | Benchmark completed; no comparable metric exceeded threshold                               |
| `1`  | Validation, acquisition, runtime, identity, inference, or cleanup failure                  |
| `2`  | **P2 only:** benchmark completed and a conclusive comparable regression exceeded threshold |

A missing/unknown metric is not itself a regression. It is reported as unknown
and excluded from baseline comparison.

P1 does not register `--baseline` or `--regression-threshold`; passing either is
a CLI parse error. P2 adds both flags and exit 2 as an additive command feature.

---

## 4. Benchmark protocol

### 4.1 Fixed prompt fixtures

Add three UTF-8 fixtures:

- `short`: small interactive request; default.
- `medium`: enough text to make prefill measurable.
- `long`: fixed large-prefill diagnostic; it is not assumed to fit every model.

Requirements:

- no secrets, external URLs, or current facts;
- deterministic text bytes;
- fixture SHA-256 recorded in output;
- requested answer has a bounded deterministic format;
- runtime-reported prompt tokens recorded because tokenizer counts differ.

`short` and `medium` fixtures are byte-bounded so their maximum request plus
output is below the minimum supported context in the benchmarkable catalog; the
runtime-reported count is still recorded afterward. `long` requires an optional
side-effect-free `countTokensOffline()` adapter capability before process startup.
If prompt tokens plus requested output exceed the context limit, fail before
spawn; never truncate silently. A backend (including daemon-managed Ollama) that
cannot tokenize offline rejects `--fixture long` before acquisition/spawn.

### 4.2 Sampling sequence

For each run:

1. Confirm process identity and model identity.
2. Start the runner-owned monotonic end-to-end timer immediately before handing
   the complete request bytes to the adapter.
3. Submit non-streaming or streaming benchmark request through adapter benchmark
   support.
4. Stop the runner-owned timer after the adapter has consumed the terminal
   response event. Adapter samples do not contain a second end-to-end duration.
5. Reject malformed counters (negative, non-finite, inconsistent counts).
6. Between runs, ensure the prior request has completed; no overlapping work in
   v1.

Warmup runs execute the same path but are excluded from aggregates.

P1 execution uses the validated configured counts (defaults: one warmup followed
by five measurements) as strictly sequential requests in the same loaded runtime
session. Request bytes and generation settings are identical. No adaptive retry,
parallel request, sorting/trimming, replacement, or outlier removal is allowed.
Generation defaults are temperature 0, top-k 40, top-p 0.95, min-p 0,
repeat penalty 1, presence penalty 0, frequency penalty 0, seed 42, and no stop
sequences. Every sampling control and an explicit
`unsupported` marker for unavailable seed/determinism controls is fingerprinted.

Timing boundaries are normative:

- request start: monotonic timestamp immediately before dispatch of complete
  request bytes;
- TTFT start: adapter-owned monotonic timestamp at successful completion of
  request write; connection establishment/request upload are excluded;
- TTFT end: first qualifying decoded user-visible content bytes under §5.1;
- generation end: validated terminal event/EOF after all generated content;
- runner end-to-end: generation end minus request start;
- wall-clock decode cross-check: terminal timestamp minus first-content
  timestamp, with generated tokens after the first token as numerator;
- authoritative decode throughput: runtime-reported generated count divided by
  runtime-reported positive decode duration.

TTFT may be zero at clock resolution, but any duration used as a throughput
divisor must be finite and strictly positive. Synchronous first-token delivery,
first-token plus terminal event in one chunk, UTF-8 fragmentation, cancellation
before TTFT, and zero-duration runtime counters are explicit tests.

Warmup failure aborts the command. Any measured timeout, cancellation, malformed
counter, premature EOF, identity/state drift, or runtime failure aborts P1 and
makes P2 comparison inconclusive. Failed samples are never dropped, retried, or
replaced; e.g. failure of measured request 3 causes no request 4, 5, or 6 and no
partial success result.

### 4.3 Metrics

```ts
export type MetricSource = "wall_clock" | "runtime_reported" | "sampled_process" | "unavailable";

export interface MeasuredMetric {
  readonly known: boolean;
  readonly value: number | null;
  readonly unit: "ms" | "tok/s" | "bytes" | "tokens";
  readonly source: MetricSource;
  readonly reason: string | null;
}
```

Per-run measurements:

- `endToEndMs` — wall clock.
- `ttftMs` — first qualifying user-visible streamed content bytes under the
  normative §4.2/§5.1 boundary; otherwise unknown.
- `promptTokens` — runtime reported.
- `generatedTokens` — runtime reported.
- `prefillTokPerSec` — runtime-reported prompt duration/count.
- `decodeTokPerSec` — runtime-reported generation duration/count.
- `sampledPeakMemoryBytes` — sampled process-tree memory with explicit probe
  source, labeled estimate.

Lifecycle measurements:

- `artifactVerificationMs`, `cacheHit`, and integrity evidence.
- `processStartToApiReadyMs` in ephemeral mode.
- `modelLoadMs`, runtime-reported when available; never inferred from API
  readiness.
- `firstRequestEndToEndMs`, measured before warmups and labeled `cold` only in
  ephemeral mode; active mode labels it `priming`.
- runtime binary, version, canonical executable, start identity, endpoint.

The ephemeral order is: spawn → measure API readiness → execute one cold model-
loading request → record runtime model-load timing when provided → execute
configured warmups → execute measured iterations. Active mode replaces spawn/
readiness/cold-load with identity verification → one excluded priming request;
load remains `not_applicable`. Baseline comparisons never
compare metrics with different semantics or sources.

### 4.4 Aggregation

For known numeric metrics across measured iterations, report:

- minimum;
- median;
- arithmetic mean;
- p10;
- p90;
- maximum;
- sample count.

Percentiles use a documented deterministic nearest-rank implementation. No
outlier is silently dropped in v1. Raw samples remain in JSON.

For sorted `N > 0` samples and percentile `p` in `[0, 1]`, nearest-rank selects
zero-based index `clamp(ceil(p * N) - 1, 0, N - 1)`. `median` remains the middle
value for odd N and arithmetic mean of the two middle values for even N; it is
not defined as nearest-rank p50. All duration measurements use a monotonic clock;
only `generatedAt` uses wall time.

`--output-tokens` is a maximum. The result records requested and actual generated
tokens separately; early EOS is valid and never rewritten as the requested count.

### 4.5 Baseline comparison

A baseline is comparable only when all fingerprint fields match:

- result schema and benchmark protocol version;
- fixture id and fixture SHA-256;
- canonical model id, quantization, and exact served artifact digest;
- source revision/exact file or daemon manifest identity;
- tokenizer identity/version or tokenizer artifact digest;
- backend name, normalized runtime version, and canonical executable identity;
- normalized effective launch flags and minimal child environment fingerprint;
- context size, batch/micro-batch, threads, GPU offload, Flash Attention, and
  KV-cache type when applicable;
- complete generation options (temperature, top-k/top-p/min-p, penalties,
  stop sequences), seed value, or explicit unsupported determinism marker;
- OS, architecture, CPU/SoC identity, logical core count, physical memory, and
  accelerator identity/memory when used;
- hardware-probe implementation/version;
- output-token request;
- mode;
- metric source.

Additionally, both results must have `requestBinding:"leased"`; active P1 results
are `inconclusive` even when all fingerprint fields match.

These fields form a canonical `comparisonFingerprint` object serialized with
stable key ordering and hashed. A required field missing on either result makes
the comparison `inconclusive`; fields are never dropped merely because one run
failed to detect them. Optional fingerprint fields use an explicitly documented
symmetric rule and must be absent on both sides to remain comparable.

Default P2 regression conditions require equal known sample counts of at least
5 and both a relative and absolute breach:

- decode median decreases by more than `--regression-threshold` **and** at least
  1 tok/s; or
- TTFT median increases by more than `--regression-threshold` **and** at least
  10 ms.

Regression math uses finite, strictly positive baseline/current medians:

- throughput absolute drop = `baselineMedian - currentMedian`;
- throughput relative drop percent = `absoluteDrop / baselineMedian * 100`;
- latency absolute increase = `currentMedian - baselineMedian`;
- latency relative increase percent = `absoluteIncrease / baselineMedian * 100`.

Both absolute and relative comparisons use strict `>` (not `>=`) and are joined
with logical AND. Zero, negative, NaN, or infinite inputs make the metric
inconclusive. Tests cover one representable value below, exactly at, and above
every threshold for latency and throughput.

Noise is evaluated before regression. For finite positive samples, define
`relativeMAD = median(abs(sample - median)) / abs(median)`. A metric is noisy
when `relativeMAD > 0.10`; exactly 0.10 is accepted. A zero median is accepted
only when every sample is zero, otherwise inconclusive; non-finite samples are
invalid. Noisy metrics are inconclusive and never produce exit 2. Fixed-vector
tests cover clean, exact-limit, over-limit, zero-median, duplicates, and invalid
samples.

Record power mode and thermal state as `measured`, `unsupported`, or
`probe_failed`. Matching `unsupported` on both runs is comparable; `probe_failed`,
one-sided support, differing probe name/version, or differing measured states is
inconclusive. The text output identifies the exact
metric, old/new median, absolute/percent delta, and thresholds. Unknown,
incomparable, or insufficient evidence never produces exit 2.

---

## 5. Backend contract

### 5.0 Artifact and lifecycle prerequisites

Benchmark orchestration requires an offline artifact inspection boundary:

```ts
export type ArtifactIntegrity =
  "digest_verified" | "size_floor_verified" | "unverified" | "not_applicable";

export interface ArtifactStatus {
  readonly present: boolean;
  readonly integrity: ArtifactIntegrity;
  readonly observedBytes: number | null;
  readonly expectedSha256: string | null;
  readonly observedSha256: string | null;
  readonly artifactIdentity: string | null;
  readonly observedSourceIdentity: string | null;
  readonly manifestSha256: string | null;
  readonly blobs: readonly {
    readonly sha256: string;
    readonly observedBytes: number;
  }[];
  readonly modelPath: string | null;
}

export type ArtifactExpectation =
  | {
      readonly family: "self_managed";
      readonly modelId: string;
      readonly sourceIdentity: string;
      readonly expectedSha256: string;
      readonly expectedBytes: number;
    }
  | {
      readonly family: "daemon_managed";
      readonly modelId: string;
      readonly sourceIdentity: string;
      readonly expectedManifestSha256: string;
      readonly expectedBlobSetSha256: string;
      readonly expectedBlobs: readonly {
        readonly sha256: string;
        readonly expectedBytes: number;
      }[];
    };

export interface VerifiedArtifactLease {
  readonly leaseId: string;
  readonly backend: BackendName;
  readonly artifactIdentity: string;
  readonly sourceIdentity: string;
  readonly observedSha256: string;
  readonly observedBytes: number;
  readonly manifestSha256: string | null;
  readonly blobs: readonly {
    readonly sha256: string;
    readonly observedBytes: number;
  }[];
  readonly disposition: "descriptor" | "immutable_snapshot" | "daemon_store";
}

export interface BackendAdapter {
  inspectArtifact?(options: PullOptions): Promise<ArtifactStatus>;
  inspectServedArtifact?(request: {
    readonly endpoint: string;
    readonly model: string;
    readonly expected: ArtifactExpectation;
  }): Promise<ArtifactStatus>;
  loadBenchmarkModel?(request: {
    readonly endpoint: string;
    readonly model: string;
    readonly lease: VerifiedArtifactLease;
    readonly signal?: AbortSignal;
  }): Promise<void>;
  countTokensOffline?(request: {
    readonly model: string;
    readonly fixtureBytes: Uint8Array;
  }): Promise<number>;
}

export interface BenchmarkContainmentPlan {
  readonly kind: "systemd_user_scope_v1";
  readonly identity: string;
  readonly systemdVersion: string;
  readonly cgroupVersion: 2;
  readonly hardLimitBytes: number;
  readonly tasksMax: number;
}

export interface BenchmarkServeOptions extends ServeOptions {
  readonly existingListener: "reject";
  readonly artifactLeaseId: string | null;
  readonly privateStoreLeaseId: string | null;
  readonly containment: BenchmarkContainmentPlan;
}

export interface BenchmarkLeaseManager {
  acquirePrivateStore(backend: BackendName): Promise<string>;
  acquireArtifact(request: {
    readonly backend: BackendName;
    readonly expected: ArtifactExpectation;
    readonly allowPull: boolean;
    readonly endpoint: string | null;
    readonly signal?: AbortSignal;
  }): Promise<VerifiedArtifactLease>;
  serve(request: {
    readonly adapter: BackendAdapter;
    readonly options: BenchmarkServeOptions;
  }): Promise<ServeHandle>;
  revalidate(lease: VerifiedArtifactLease): Promise<void>;
  release(leaseId: string): Promise<void>;
  releaseAll(): Promise<void>;
}
```

Active mode requires `inspectServedArtifact()` to return `present:true`, exact
served source/artifact identity, and `digest_verified`, and the runner requires
exact family-specific `ArtifactExpectation` equality. Self-managed proof matches
source, SHA-256, and bytes. Daemon-managed proof matches source, manifest SHA-256,
and every expected blob SHA-256/size plus the canonical blob-set SHA-256.
Null, malformed, runtime-self-asserted-only, or mismatched evidence rejects P1.
The operation revalidates listener/runtime/model identity and does not trust model
ids alone.

To prevent post-verification substitution, ephemeral mode obtains a typed
`VerifiedArtifactLease` from `BenchmarkLeaseManager.acquireArtifact()`. The manager opens a regular file with
no-follow semantics, verifies ownership/permissions/digest through that
descriptor, records stable device/inode/size/mtime metadata, and keeps the lease
through final process-tree shutdown. `serve()` receives the lease id and must
load from a benchmark-private immutable snapshot created atomically on the same
trusted filesystem, or from descriptor-backed storage only when a documented
platform primitive seals content against in-place writes for the entire lease.
Read-only permission bits alone are insufficient. The adapter rehashes leased
content through held descriptors immediately before and after every request;
absence of enforceable immutability rejects P1 before inference. Daemon-managed
stores use an isolated store lease that binds exact
manifest bytes and every referenced blob digest/size through held descriptors or
an immutable private snapshot; validating only the manifest name or top-level
model label is insufficient. Revalidation proves the manifest and complete blob
set still resolve to the leased objects, including runtimes that lazily mmap or
read weights after readiness. `BenchmarkLeaseManager.revalidate()` is required before
spawn/model load, before and after every first/warmup/measured request, and during
final cleanup. The lease remains held through shutdown and is released exactly
once in `finally`; release is idempotent and removes benchmark-private snapshots/
stores but retains ordinary verified cache artifacts. Replacement causes failure
before accepting results. Backends without a substitution-resistant lease
primitive are ineligible for P1. Tests replace the cache pathname and daemon
manifest/blob between checkpoints and require fail-closed behavior.

Daemon blob entries are sorted by SHA-256 then byte size, canonicalized as the
complete array with RFC 8785, and SHA-256 hashed to form `blobSetSha256`; missing,
extra, or duplicate entries reject. Expected daemon manifest/blob evidence is
curated offline catalog data, never learned from the runtime being benchmarked.

`inspectArtifact()` is offline/read-only. For self-managed weights, only
`digest_verified` is benchmarkable. For daemon-managed stores, P1 also requires
`digest_verified`; size-floor fallback remains valid for normal product behavior
but not for benchmark evidence. Expected/observed evidence is included in JSON,
while local artifact paths are redacted from default output.

`BenchmarkLeaseManager` is the sole serving/resource-resolution boundary and a
stateful, process-local, per-command resource owner; adapters remain stateless.
`serve()` resolves opaque IDs inside the manager and passes only already-resolved,
immutable spawn resources to a non-public backend launch helper—an adapter never
looks up an ID, derives a path from one, or accesses process-global lease state.
The manager owns open descriptors, immutable snapshots, private
stores, and opaque unguessable lease-id mappings. IDs are valid only in the
creating process/session. Unknown, cross-session, released, or duplicate IDs fail
closed. Concurrent operations are serialized per lease; `release()` and
`releaseAll()` are idempotent, and startup scavenges only cryptographically named
dead-owner private resources under the benchmark root. Crash-safe cleanup and
ownership metadata are tested.

Benchmark ephemeral mode always uses `reject`. Returning `ownedByUs:false` is a
contract violation and no inference/stop call is made on that handle.

### 5.1 Additive optional benchmark interface

Do not overload normal user-facing `chat()` with benchmark-only response fields.
Add an optional capability method:

```ts
export interface RequestedGenerationConfiguration {
  readonly temperature: 0;
  readonly topK: number;
  readonly topP: number;
  readonly minP: number;
  readonly repeatPenalty: number;
  readonly presencePenalty: number;
  readonly frequencyPenalty: number;
  readonly seed: 42;
  readonly stopSequences: readonly string[];
}

export type EffectiveControl<T> =
  | { readonly status: "applied"; readonly value: T }
  | { readonly status: "unsupported"; readonly value: null };

export interface EffectiveGenerationConfiguration {
  readonly maxOutputTokens: EffectiveControl<number>;
  readonly temperature: EffectiveControl<number>;
  readonly topK: EffectiveControl<number>;
  readonly topP: EffectiveControl<number>;
  readonly minP: EffectiveControl<number>;
  readonly repeatPenalty: EffectiveControl<number>;
  readonly presencePenalty: EffectiveControl<number>;
  readonly frequencyPenalty: EffectiveControl<number>;
  readonly seed: EffectiveControl<number>;
  readonly stopSequences: EffectiveControl<readonly string[]>;
}

export interface RuntimeBenchmarkRequest {
  readonly endpoint: string;
  readonly model: string;
  readonly messages: readonly ChatMessage[];
  readonly maxOutputTokens: number;
  readonly fixtureId: string;
  readonly generation: RequestedGenerationConfiguration;
  readonly signal?: AbortSignal;
}

export interface RuntimeBenchmarkSample {
  readonly content: string;
  readonly ttftMs: number | null;
  readonly promptTokens: number | null;
  readonly generatedTokens: number | null;
  readonly promptDurationMs: number | null;
  readonly decodeDurationMs: number | null;
  readonly metricSource: "ollama_native" | "llamacpp_native" | "mlx_native";
  readonly effectiveGeneration: EffectiveGenerationConfiguration;
}

export interface BackendAdapter {
  // Existing methods unchanged.
  benchmark?(request: RuntimeBenchmarkRequest): Promise<RuntimeBenchmarkSample>;
}
```

`capabilities.openAiCompatible` does not imply that detailed timing fields are
available. P1 requires `benchmark()` for selected adapters; there is no silent
`chat()` fallback. A backend without the method is rejected before acquisition
or spawn. A future explicit `--end-to-end-only` mode requires a separate spec.

TTFT uses only the adapter's monotonic clock and is elapsed time from successful
completion of request write to the first non-empty **user-visible generated
content bytes**. The runner never computes TTFT. Metadata-only events, role markers, empty deltas, transport
framing, and hidden reasoning are excluded. UTF-8 fragments count only after
forming at least one valid content byte sequence. The result records whether
connection establishment is excluded (`ttftIncludesConnectionSetup:false` is a
required fingerprint/result field).

### 5.2 Adapter requirements

- Validate all external response payloads with Zod.
- Use active/ephemeral endpoint passed in request; no hardcoded port.
- Revalidate listener/process/runtime/model identity immediately before sending
  benchmark content.
- Apply internal timeout and caller cancellation.
- Bound response bytes and token counts.
- Refuse redirects.
- Emit no per-token logs.

### 5.3 Runtime mapping

`applied` is not accepted merely because the adapter requested a value. Each
adapter maintains a version-pinned capability map verified against official
runtime documentation and real smoke evidence. Unit tests assert exact wire
fields; malformed/rejected options fail the request. If support cannot be proven,
report `unsupported`. Maximum output tokens and temperature are required for P1;
a runtime that cannot prove both is not benchmarkable. Other unsupported controls
remain explicit and participate in the fingerprint.

#### Ollama

Normalized controls map to `/api/chat` `options`: `temperature`, `top_k`,
`top_p`, `min_p`, `repeat_penalty`, `presence_penalty`, `frequency_penalty`,
`seed`, `stop`, and `num_predict`. Support is keyed to the normalized Ollama
version and validated by adapter fixtures plus production smoke.

Source fields, when available:

- `load_duration`;
- `prompt_eval_count`, `prompt_eval_duration`;
- `eval_count`, `eval_duration`;
- streaming first-chunk wall clock for TTFT.

#### llama.cpp

Normalized controls map to the pinned `llama-server` OpenAI/native request
fields. Standard OpenAI fields are used where defined; llama.cpp extensions such
as top-k/min-p are sent only when supported by the pinned build capability map.
The response's accepted timing/count contract and real smoke establish support.

Source fields, when available:

- `timings.prompt_n`, `timings.prompt_ms`;
- `timings.predicted_n`, `timings.predicted_ms`;
- streaming first-chunk wall clock for TTFT.

#### MLX

Use only fields exposed by the targeted `mlx_lm.server` version. If OpenAI usage
contains counts but no phase durations, report token counts while phase
throughput remains unknown. Do not apply Ollama/llama.cpp efficiency assumptions.
Generation-control support must be derived from the pinned MLX server API; absent
or silently ignored controls are `unsupported`, never `applied`.

---

## 6. Memory and process measurement

P1 uses the process-listener identity boundary plus verified parent/descendant
relationships to identify the complete benchmark-owned process tree. Process
memory sampling must:

- sample only verified process instances and revalidate PID/start/executable
  identity before every sample;
- in ephemeral Linux mode, sample the verified scope's cgroup-v2
  `memory.current` as aggregate process-tree resident/accounted memory and label
  it `cgroup_memory_current`, not RSS;
- in active mode, when attribution is possible, use the arithmetic sum of RSS
  across unique verified process instances and explicitly label it
  `summed_process_rss_estimate` because shared pages may be double-counted;
- treat unattributable workers/attached processes as unknown rather than adding
  them heuristically;
- use a documented fixed interval (default 250 ms);
- stop sampling in all success/failure paths;
- cap retained samples;
- report interval and count;
- label peak memory as sampled/estimated with its exact source;
- return unknown when the platform probe cannot source it in active mode;
  ephemeral P1 treats an unsupported/failing memory probe as a pre-spawn error.

Fresh listener identity can validate active inference, but does not establish an
owned process tree. Therefore active-mode RSS is unknown unless every included
process can be safely attributed without signaling or state mutation.

Ephemeral mode requires an inherited process-tree-wide hard memory containment
limit supported by the platform/runtime; otherwise it fails before spawn. The
limit is set below the host fit budget by the reserve/headroom in §13.1. RSS
polling is measurement and secondary early cancellation only; the hard limit
bounds benchmark-tree allocation after an admission-time host-availability check.
It does not control unrelated workloads. Default sample interval is 250 ms and retained samples
are capped at 4800. A sampled breach cancels work and terminates only the
revalidated owned process tree. Sampling overhead is measured in a bounded test;
if the sampler exceeds 1% CPU in its fixture, the implementation must increase
the interval before release.

Containment is an injectable platform boundary, separate from backend protocol
logic. It must prove before spawn that the byte ceiling applies to the root and
all descendants, return immutable spawn configuration/containment identity, and
verify after spawn that the owned process tree joined that exact containment.
Merely observing RSS, setting a listener-process-only limit, or trusting a
runtime flag does not satisfy support. No shell command construction, global OS
limit mutation, privilege escalation, or signaling of unrelated processes is
permitted. Unsupported platforms fail with a typed pre-spawn error and no
benchmark result; the CLI never describes sampled polling as enforcement.

P1 support is explicit:

| Platform | Ephemeral P1             | Active P1 | Required primitive                                                                   |
| -------- | ------------------------ | --------- | ------------------------------------------------------------------------------------ |
| Linux    | Supported when available | Supported | systemd user transient scope backed by cgroup v2, `MemoryMax`, and `MemorySwapMax=0` |
| macOS    | Unsupported              | Supported | no qualifying unprivileged aggregate process-tree primitive is specified for P1      |
| Windows  | Unsupported              | Supported | Windows Job Object support is deferred                                               |

Linux containment launches through a uniquely named systemd user transient scope
using canonical, version-checked `systemd-run`/`systemctl` executables with
argument arrays and `shell:false`. The scope sets `MemoryMax=hardLimitBytes` and
`MemorySwapMax=0` plus `TasksMax=256` before runtime execution. The runner
verifies the exact unit,
`ControlGroup`, effective properties, cgroup-v2 controller files, runtime PID,
`pids.max`, and every descendant's cgroup membership before fixture transmission and before/
after every request. Unexpected migration aborts; cleanup stops only the unique
verified unit and proves unit/cgroup/listener removal. A host without systemd user
scope access and cgroup v2 fails before runtime spawn. P1 real ephemeral smoke
therefore requires such a Linux host; macOS/Windows P1 smoke uses active mode
only. P3 MLX is active-only unless a later approved spec names an enforceable
macOS primitive.

Because `ephemeral` remains the cross-platform CLI default, unsupported hosts
fail during side-effect-free preflight with exit 1 and guidance to start the
matching model through `llmup up` and rerun with `--mode active`. There is no
implicit fallback to or mutation of an active server.

The sampler must not become the dominant benchmark cost. Sampling overhead is
measured in a microbenchmark or bounded integration test before enablement.

---

## 7. Text output

Text output includes:

1. runtime/model/quant/environment identity;
2. acquisition and load information;
3. aggregate metric table;
4. bottleneck interpretation based only on known metrics;
5. baseline comparison when supplied;
6. cleanup result for ephemeral mode;
7. explicit unknown reasons.

No ANSI/control bytes from runtime/model responses may reach output without
`stripControl()`.

---

## 8. JSON output schema

Normative metric/lifecycle primitives (validated before serialization):

```ts
export type UnknownReasonCode =
  | "runtime_unsupported"
  | "probe_failed"
  | "active_pid_untrusted"
  | "insufficient_samples"
  | "active_request_artifact_unbound"
  | "not_comparable";

export type MetricObservation =
  | {
      readonly known: true;
      readonly value: number;
      readonly unit: "ms" | "tok/s" | "bytes" | "tokens";
      readonly source: Exclude<MetricSource, "unavailable">;
      readonly reasonCode: null;
    }
  | {
      readonly known: false;
      readonly value: null;
      readonly unit: "ms" | "tok/s" | "bytes" | "tokens";
      readonly source: "unavailable";
      readonly reasonCode: UnknownReasonCode;
    };

export type LifecycleMetric =
  | { readonly status: "measured"; readonly valueMs: number }
  | { readonly status: "not_applicable"; readonly valueMs: null }
  | { readonly status: "unknown"; readonly valueMs: null; readonly reasonCode: UnknownReasonCode };
```

```ts
export interface BenchmarkSampleV1 {
  readonly index: number;
  readonly startOffsetMs: number;
  readonly endOffsetMs: number;
  readonly endToEndMs: MetricObservation;
  readonly ttftMs: MetricObservation;
  readonly promptTokens: MetricObservation;
  readonly generatedTokens: MetricObservation;
  readonly prefillTokPerSec: MetricObservation;
  readonly decodeTokPerSec: MetricObservation;
  readonly sampledPeakMemoryBytes: MetricObservation;
  readonly effectiveGenerationConfigurationHash: string;
}

export type AggregateV1 =
  | {
      readonly known: true;
      readonly source: Exclude<MetricSource, "unavailable">;
      readonly count: number;
      readonly min: number;
      readonly median: number;
      readonly mean: number;
      readonly p10: number;
      readonly p90: number;
      readonly max: number;
      readonly samples: readonly number[];
      readonly reasonCode: null;
    }
  | {
      readonly known: false;
      readonly source: "unavailable";
      readonly count: 0;
      readonly min: null;
      readonly median: null;
      readonly mean: null;
      readonly p10: null;
      readonly p90: null;
      readonly max: null;
      readonly samples: readonly [];
      readonly reasonCode: UnknownReasonCode;
    };

export interface PowerModeValue {
  readonly source: "ac" | "battery";
  readonly lowPowerMode: boolean;
  readonly charging: boolean | null;
}

export type ThermalStateValue = "nominal" | "fair" | "serious" | "critical";

export type EnvironmentObservation<T> =
  | {
      readonly status: "measured";
      readonly value: T;
      readonly probeName: string;
      readonly probeVersion: string;
      readonly reasonCode: null;
    }
  | {
      readonly status: "unsupported";
      readonly value: null;
      readonly probeName: string;
      readonly probeVersion: string;
      readonly reasonCode: "platform_unsupported";
    }
  | {
      readonly status: "probe_failed";
      readonly value: null;
      readonly probeName: string;
      readonly probeVersion: string;
      readonly reasonCode: "permission_denied" | "timeout" | "malformed_output";
    };

export interface NormalizedLaunchConfigurationV1 {
  readonly host: "127.0.0.1" | "::1";
  readonly port: number;
  readonly contextTokens: number;
  readonly batchSize: number | null;
  readonly microBatchSize: number | null;
  readonly threads: number | null;
  readonly gpuLayers: number | null;
  readonly flashAttention: boolean | null;
  readonly kvCacheType: string | null;
  readonly modelAlias: string | null;
  readonly environment: readonly {
    readonly name: string;
    readonly valueSha256: string;
  }[];
}

export type ComparisonLaunchConfigurationV1 = Omit<
  NormalizedLaunchConfigurationV1,
  "host" | "port"
>;

export type ArtifactFingerprintV1 =
  | {
      readonly family: "self_managed";
      readonly sourceIdentity: string;
      readonly sha256: string;
      readonly bytes: number;
    }
  | {
      readonly family: "daemon_managed";
      readonly sourceIdentity: string;
      readonly manifestSha256: string;
      readonly blobSetSha256: string;
      readonly blobs: readonly {
        readonly sha256: string;
        readonly bytes: number;
      }[];
    };

export interface CanonicalLoopbackEndpointV1 {
  readonly protocol: "http:";
  readonly host: "127.0.0.1" | "::1";
  readonly port: number;
}

export interface ComparisonFingerprintV1 {
  readonly protocolVersion: 1;
  readonly resultSchemaVersion: 1;
  readonly fixtureId: string;
  readonly fixtureSha256: string;
  readonly modelId: string;
  readonly quantization: string;
  readonly artifact: ArtifactFingerprintV1;
  readonly tokenizerIdentity: string;
  readonly backend: BackendName;
  readonly runtimeVersion: string;
  readonly executableIdentityHash: string;
  readonly comparisonLaunchConfiguration: ComparisonLaunchConfigurationV1;
  readonly comparisonLaunchConfigurationHash: string;
  readonly contextTokens: number;
  readonly requestedOutputTokens: number;
  readonly requestedGenerationConfiguration: RequestedGenerationConfiguration;
  readonly requestedGenerationConfigurationHash: string;
  readonly appliedGenerationConfiguration: EffectiveGenerationConfiguration;
  readonly appliedGenerationConfigurationHash: string;
  readonly determinismSupport: "seeded" | "unsupported";
  readonly platform: Platform;
  readonly arch: Arch;
  readonly cpuOrSocIdentity: string;
  readonly logicalCores: number;
  readonly physicalMemoryBytes: number;
  readonly acceleratorIdentity: string | null;
  readonly acceleratorMemoryBytes: number | null;
  readonly hardwareProbeVersion: string;
  readonly powerMode: EnvironmentObservation<PowerModeValue>;
  readonly thermalState: EnvironmentObservation<ThermalStateValue>;
  readonly mode: "ephemeral" | "active";
  readonly metricSource: "ollama_native" | "llamacpp_native" | "mlx_native";
  readonly ttftIncludesConnectionSetup: false;
  readonly requestBinding: "leased" | "pre_post_only";
  readonly sampler: {
    readonly probeName: string;
    readonly probeVersion: string;
    readonly intervalMs: number | null;
    readonly memorySource: "cgroup_memory_current" | "summed_process_rss_estimate" | "unavailable";
  };
  readonly containment:
    | {
        readonly kind: "systemd_user_scope_v1";
        readonly systemdVersion: string;
        readonly cgroupVersion: 2;
        readonly hardLimitBytes: number;
        readonly tasksMax: number;
      }
    | { readonly kind: "not_applicable" };
}

export type BaselineComparisonV1 =
  | { readonly status: "not_requested" }
  | { readonly status: "inconclusive"; readonly reasonCode: UnknownReasonCode }
  | {
      readonly status: "pass" | "regression";
      readonly thresholdPercent: number;
      readonly metrics: readonly {
        readonly name: "ttftMs" | "decodeTokPerSec";
        readonly baselineMedian: number;
        readonly currentMedian: number;
        readonly absoluteDelta: number;
        readonly relativeDeltaPercent: number;
        readonly regressed: boolean;
      }[];
    };

export interface BenchmarkResultV1 {
  readonly schemaVersion: 1;
  readonly generatedAt: string;
  readonly fixture: { readonly id: string; readonly sha256: string };
  readonly fingerprint: ComparisonFingerprintV1;
  readonly comparisonFingerprintHash: string;
  readonly runtime: {
    readonly backend: BackendName;
    readonly version: string;
    readonly mode: "ephemeral" | "active";
    readonly endpoint: CanonicalLoopbackEndpointV1;
    readonly executableIdentityHash: string;
    readonly processStartedAt: string;
    readonly listenerPid: number;
    readonly verifiedProcessTreeSize: number;
    readonly requestBinding: "leased" | "pre_post_only";
  };
  readonly integrity: {
    readonly status: "digest_verified";
    readonly artifactIdentity: string;
    readonly expected: ArtifactFingerprintV1;
    readonly observed: ArtifactFingerprintV1;
  };
  readonly cache:
    | {
        readonly status: "observed";
        readonly beforePresent: boolean;
        readonly beforeIntegrity: ArtifactIntegrity;
        readonly acquisitionPerformed: boolean;
        readonly cacheHit: boolean;
        readonly readyPresent: true;
        readonly postCleanupPresent: boolean;
        readonly retainedAfterBenchmark: boolean;
        readonly temporaryArtifactsRemaining: 0;
        readonly locksRemaining: 0;
      }
    | {
        readonly status: "not_applicable";
        readonly beforePresent: null;
        readonly beforeIntegrity: "not_applicable";
        readonly acquisitionPerformed: false;
        readonly cacheHit: null;
        readonly readyPresent: null;
        readonly postCleanupPresent: null;
        readonly retainedAfterBenchmark: null;
        readonly temporaryArtifactsRemaining: null;
        readonly locksRemaining: null;
      };
  readonly lifecycle: {
    readonly artifactVerification: LifecycleMetric;
    readonly processStartToApiReady: LifecycleMetric;
    readonly modelLoad: LifecycleMetric;
    readonly firstRequestEndToEnd: LifecycleMetric;
  };
  readonly configuration: {
    readonly iterations: number;
    readonly warmups: number;
    readonly requestedOutputTokens: number;
    readonly launch: NormalizedLaunchConfigurationV1;
    readonly requestedGeneration: RequestedGenerationConfiguration;
    readonly appliedGeneration: EffectiveGenerationConfiguration;
    readonly requestGenerationEvidence: readonly {
      readonly phase: "first" | "warmup" | "measured";
      readonly index: number;
      readonly effectiveGenerationConfigurationHash: string;
    }[];
  };
  readonly sampling:
    | {
        readonly probeName: string;
        readonly probeVersion: string;
        readonly memoryStatus: "measured";
        readonly memorySource: "cgroup_memory_current" | "summed_process_rss_estimate";
        readonly intervalMs: number;
        readonly retainedSampleCount: number;
        readonly maxRetainedSamples: number;
        readonly memorySamples: readonly {
          readonly offsetMs: number;
          readonly memoryBytes: number;
        }[];
        readonly sampledPeakMemoryBytes: number;
        readonly safetyLimitBytes: number | null;
        readonly safetyHeadroomBytes: number | null;
        readonly watchdogMode: "hard_limit_plus_sampling" | "sampling_only";
        readonly containmentIdentityHash: string | null;
        readonly containmentVerified: boolean;
        readonly watchdogStatus: "not_breached";
        readonly breachSampleBytes: null;
        readonly cancellationVerified: false;
      }
    | {
        readonly probeName: string;
        readonly probeVersion: string;
        readonly memoryStatus: "unsupported";
        readonly memorySource: "unavailable";
        readonly intervalMs: null;
        readonly retainedSampleCount: 0;
        readonly maxRetainedSamples: 0;
        readonly memorySamples: readonly [];
        readonly sampledPeakMemoryBytes: null;
        readonly safetyLimitBytes: null;
        readonly safetyHeadroomBytes: null;
        readonly watchdogMode: "not_applicable";
        readonly containmentIdentityHash: null;
        readonly containmentVerified: false;
        readonly watchdogStatus: "not_applicable";
        readonly breachSampleBytes: null;
        readonly cancellationVerified: false;
      };
  readonly samples: readonly BenchmarkSampleV1[];
  readonly aggregates: {
    readonly endToEndMs: AggregateV1;
    readonly ttftMs: AggregateV1;
    readonly prefillTokPerSec: AggregateV1;
    readonly decodeTokPerSec: AggregateV1;
    readonly sampledPeakMemoryBytes: AggregateV1;
  };
  readonly baseline: BaselineComparisonV1;
  readonly cleanup: {
    readonly required: boolean;
    readonly verified: boolean;
    readonly ownedProcessCount: number;
    readonly remainingProcessCount: 0;
    readonly remainingListenerCount: 0;
    readonly remainingTemporaryArtifactCount: 0;
    readonly remainingContainmentCount: 0;
  };
  readonly unknowns: readonly {
    readonly metric: string;
    readonly reasonCode: UnknownReasonCode;
  }[];
}
```

Canonical hashes use RFC 8785 JSON Canonicalization Scheme over the complete
unhashed object, UTF-8 encoding, and SHA-256 rendered as lowercase 64-hex. Arrays
whose order is semantically irrelevant (environment entries) are sorted by
documented key/value byte order before canonicalization; request-message and stop-
sequence order remains unchanged. Null means not applicable; `unsupported` and
`probe_failed` remain explicit discriminants and are never collapsed. Each hash
excludes its own hash field; `comparisonFingerprintHash` covers the complete
unhashed `fingerprint`. The result stores both canonical objects and hashes so
completeness is reviewable. Table
tests verify semantically identical objects hash equally and each field mutation
changes the fingerprint.

`NormalizedLaunchConfigurationV1.environment` contains only documented,
allowlisted, non-sensitive runtime-tuning variables. Values are represented by
SHA-256 rather than emitted raw. Variables containing paths, usernames, cache/
home locations, endpoints, credentials, proxies, loader settings, or telemetry
identifiers are omitted entirely—not hashed—and their functional configuration
is represented by dedicated normalized fields where relevant.

Known values are finite and nonnegative; durations used as divisors are positive.
Unknown values always carry a stable reason code. Aggregates include source,
known sample count, min/median/mean/p10/p90/max, and raw numeric samples. Samples
contain no prompt or generated content. The result includes a complete canonical
comparison fingerprint, sampling metadata, runtime executable identity hash,
effective launch configuration, integrity discriminant/evidence, and cleanup
status. `.strict()` schemas reject unknown keys at every trusted boundary.
Root-schema `superRefine` requires the host/port-omitting projection of root launch
configuration plus requested/applied objects to equal their fingerprint copies,
runtime identity to match fingerprint runtime
fields, and `integrity.expected`, `integrity.observed`, and fingerprint artifact
objects to be deeply equal after canonical blob ordering. Cache evidence must
match those artifact fields. Any
divergence is schema-invalid; table tests mutate each duplicate independently.
It requires exactly `1 + warmups + iterations` ordered request-generation
evidence entries; every hash equals the root applied-generation hash, and each
measured sample carries that same hash. Per-run sampled memory peaks are derived
only from raw samples timestamped inside that request's start/end window; their
aggregate and the command-wide sampled peak must recompute exactly from retained
raw evidence.
It also enforces mode invariants: successful ephemeral results use observed cache
evidence, measured process-tree memory, `hard_limit_plus_sampling`, non-null limit/headroom/
containment identity, `containmentVerified:true`, and verified required cleanup;
active results use `sampling_only` with null containment fields or the unsupported
sampling branch, `cache.status:"not_applicable"`, and `cleanup.required:false`.
Ephemeral measured memory source is exactly `cgroup_memory_current`; active
measured source is exactly `summed_process_rss_estimate`; unsupported active
source is `unavailable`. Root sampling source/probe/version/interval must equal
the sampler fingerprint.
For observed cache evidence, `retainedAfterBenchmark` equals
`postCleanupPresent`: an ordinary self-managed verified cache entry is retained,
whereas the isolated daemon-private store is absent after successful cleanup.
A memory-limit or sampled-watchdog breach is an operational failure, so it emits
no `BenchmarkResultV1`; cancellation proof belongs in the sanitized error path
and tests, not a success document.
Metric refinements require exact field semantics: end-to-end and TTFT are
`wall_clock`/`ms`; prompt/generated counts are `runtime_reported`/`tokens`;
prefill/decode rates are `runtime_reported`/`tok/s`; sampled peak memory is
`sampled_process`/`bytes`. Unknown variants retain the same field unit with
`source:"unavailable"`; no other source/unit pairing validates.

Raw runtime payloads are not emitted by default because they may be large,
unstable, or contain echoed prompt content.

---

## 9. Data boundaries

### 9.1 Offline datasets

- `data/models.json`: read-only model/source/quant metadata.
- `data/perf.json`: read-only estimated-throughput calibration.
- Benchmark execution never writes either file.
- P1 catalog authoring adds validated daemon manifest/blob expectations; changing
  their format again remains ask-first.

### 9.2 Promotion into curated performance data

A benchmark result may become evidence for a future dataset update only through a
separate maintainer review that:

1. verifies artifact/runtime/hardware fingerprints;
2. retains raw JSON evidence outside runtime package data;
3. cites reproducible commands and versions;
4. uses multiple runs/machines where the dataset claim is broader than one host;
5. updates provenance and tests explicitly.

One local benchmark never silently becomes a global efficiency scalar.

---

## 10. Project structure

Proposed files:

```text
src/benchmark/
  fixtures.ts          Versioned prompt fixtures + SHA-256
  schema.ts            CLI/result/baseline Zod schemas
  stats.ts             Pure deterministic aggregation/percentiles
  runner.ts            Lifecycle + iteration orchestration
  resource-containment.ts  Hard process-tree memory-limit boundary
  process-memory-sampler.ts   Bounded process-tree memory sampling
  compare.ts           Comparable fingerprint + regression logic
src/commands/
  benchmark.ts         Command boundary and rendering orchestration
src/backend/
  adapter.ts           Optional benchmark request/sample contract
  ollama.ts            Ollama timing/stream adapter
  llamacpp.ts          llama.cpp timing/stream adapter
  mlx.ts               MLX metrics when implemented
tests/benchmark/
  fixtures.test.ts
  schema.test.ts
  stats.test.ts
  runner.test.ts
  resource-containment.test.ts
  process-memory-sampler.test.ts
  compare.test.ts
tests/commands/
  benchmark.test.ts
tests/backend/
  adapter-contract.test.ts
  <backend benchmark tests>
```

One file per module; tests mirror source structure.

---

## 11. Code conventions

- TypeScript strict mode; no `any`.
- ESM `.js` import paths.
- Named exports only.
- Explicit return types on exported functions.
- Files `kebab-case.ts`; types `PascalCase`; functions `camelCase`; constants
  `SCREAMING_SNAKE_CASE`.
- Zod-validates CLI input, baseline JSON, runtime responses, and result JSON.
- Typed errors; no numeric error returns from command modules.
- All backend-specific timing parsing stays behind `BackendAdapter`.
- No new runtime dependency without approval.

Illustrative pure aggregation style:

```ts
export function median(samples: readonly number[]): number {
  if (samples.length === 0) {
    throw new ValidationError("median requires at least one sample");
  }
  const sorted = [...samples].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1
    ? (sorted[middle] as number)
    : ((sorted[middle - 1] as number) + (sorted[middle] as number)) / 2;
}
```

---

## 12. Testing strategy

### 12.1 TDD and test hierarchy

- Write failing tests before behavior changes.
- Unit-test pure fixture, stats, schema, and comparison logic.
- Integration-test benchmark orchestration with fake clocks and fake adapters.
- Contract-test every registered adapter's benchmark behavior.
- Run real runtime smoke tests only through
  `.github/skills/runtime-production-smoke-test/SKILL.md`, never from Vitest.

### 12.2 Mock boundaries

All automated tests mock with `vi.fn()`:

- network/fetch;
- filesystem and baseline reads;
- child-process/runtime operations;
- process sampling;
- wall and monotonic clocks;
- sleep/backoff.

No test spawns Ollama, llama.cpp, or MLX or downloads weights.

### 12.3 Required cases

- CLI validation boundaries for every flag.
- Fixed fixture bytes and SHA-256 stability.
- Median/percentile aggregation, odd/even counts, no mutation.
- Percentile property/table tests: selected value belongs to input set,
  permutation invariance, monotonicity in percentile, p=0/p=1 clamping,
  singleton, duplicates, and extreme finite values. Median tests include
  permutation invariance, duplicates, signed zero, and overflow-safe middle-pair
  averaging.
- Unknown metric propagation.
- Runtime counter schema rejection: negative, non-finite, inconsistent.
- Warmup excluded from aggregates.
- Active mode never stops or writes state.
- Ephemeral mode always stops owned process on success/failure.
- Separate event-order tests require the exact self-managed and daemon-managed
  sequences from §3.3 and fail on every adjacent transposition or skipped gate.
- Existing listener/active state preserved.
- Port conflict fails before spawn.
- Missing cache refuses without `--allow-pull`.
- Offline artifact miss without `--allow-pull` produces zero network, daemon,
  cache-mutation, and spawn calls.
- Daemon-managed and self-managed acquisition ordering is asserted separately.
- Effective generation controls/support markers match requested configuration
  and remain identical across first/warmup/measured requests.
- Per-backend wire-payload tests prove every generation control is either sent
  under the documented runtime field and confirmed effective or marked
  unsupported; omission/default inference cannot produce `applied`.
- Digest mismatch, oversized transfer, redirect violation, cache symlink,
  concurrent pull, and interrupted-partial cleanup fail closed.
- Listener-steal race between candidate-port selection, spawn, and readiness
  fails before fixture transmission and cleans only verified owned processes.
- Spawned-child PID mismatch, daemonization, worker descendants, parent exit,
  PID reuse, partial cleanup, and cancellation during startup are covered.
- Artifact pathname/inode replacement after digest verification but before spawn
  and before model-ready identity is rejected while the verified lease remains
  held.
- Lease tests obtain, revalidate before/after every request, retain through
  shutdown, and release exactly once; release remains safe when `finally` is
  entered repeatedly after partial failures. Daemon tests substitute both the
  manifest and an individual referenced blob at every checkpoint.
- Baseline fingerprint mismatch is incomparable, never regression.
- Baseline reads reject symlink/FIFO/device/oversize/schema-poisoning inputs and
  use complete required fingerprints.
- Exit 2 only for comparable threshold breach.
- Relative-only noise, absolute-only noise, thermal/power unknown, unequal or
  fewer-than-five samples produce `inconclusive`, not exit 2.
- Output control-character sanitization.
- Incremental decoded response bytes/chunks/events/strings/nesting and request
  byte/token bounds, including endless stream cancellation.
- Independent acquisition/start/request/shutdown/global deadlines actively
  cancel sockets, streams, sampling, and owned process trees.
- Automated timing tests inject monotonic clock, timer scheduler, transport,
  process, filesystem, and port allocator; use no real sleep/socket/daemon/wall
  clock. Tests advance exactly below/at/above each deadline, prove phase deadlines
  never reset/extend the global deadline, and run late queued callbacks to assert
  no post-result writes/state mutations.
- Disk temporary-copy margin, memory reserve, and memory-watchdog breaches fail
  before host exhaustion.
- Active state is revalidated before every request; concurrent switch/down
  aborts without restoring stale state or stopping either process.
- Active served source/digest/size is independently inspected before and after
  every request; any changed artifact or failed proof discards all samples.
- Ephemeral startup refuses before spawn when process-tree hard containment is
  unsupported; platform-boundary tests prove the installed byte limit is below
  the host reserve budget and that a limit breach cancels and cleans owned work.
- Mocked containment tests cover systemd/cgroup discovery and malformed read-back;
  the required Linux smoke reads kernel cgroup membership/limits for the real
  runtime and descendants and retains sanitized gate evidence. A skipped smoke
  is `not_run`, never passed.
- Containment tests verify effective `MemoryMax`, `MemorySwapMax`, `TasksMax`,
  `memory.max`, `memory.swap.max`, and `pids.max`, plus migration/escape detection
  and idempotent unit/cgroup removal.
- Runtime log tests prove stdio is not inherited, discard mode is constant-space,
  bounded capture cancels on byte/line overflow, and diagnostics expose no raw
  line, path, content, or control bytes.
- Active P1 baseline input is always inconclusive with
  `active_request_artifact_unbound`; no matching pre/post evidence can produce
  exit 2.
- Daemon catalog/result fixtures cover manifest changes and missing, extra,
  duplicate, reordered, wrong-size, and wrong-digest blob entries.
- Strict result-schema tests independently mutate every duplicated root and
  fingerprint field and require cross-field refinement failure.
- Success emits one JSON document; any cleanup/operational failure emits no
  success stdout document.
- Advice commands remain network/probe-free.

### 12.4 Verification commands

```bash
npm test
npm run typecheck
npm run lint
npm run build
```

Runtime release verification additionally invokes:

```text
/runtime-production-smoke-test <backend> benchmark
```

The retained opt-in smoke evidence matrix records backend/runtime version,
canonical executable identity hash, model/artifact digest, platform/hardware
fingerprint, exact command/configuration, fixture hashes, validated counters,
and cleanup result. A smoke passes only when all measured samples complete,
counters/timings are finite and valid, output retains no prompt/response content,
and no owned process, listener, temporary artifact, or lock remains. MLX is not
complete until evidence from this same matrix is retained for a real Apple
Silicon run—not merely an attempted invocation.

---

## 13. Security, integrity, and lifecycle

- Loopback-only endpoint; no non-loopback benchmark opt-in in v1.
- Parse endpoints with `URL` and accept only `http:`, exact normalized host
  `127.0.0.1` or `::1`, and an explicit canonical port; reject credentials,
  hostname/DNS resolution, IPv4-mapped or unusual numeric forms, query, fragment,
  and paths other than `/`. Correlate each connection with the exact listener
  address/port and verified process before and after every request.
- Refuse redirects for inference.
- Revalidate listener address, PID, canonical executable, process start identity,
  backend identity, and model identity immediately before benchmark content.
- Verify weights before serving; self-managed backends require catalog digest.
- Byte-cap all runtime responses and baseline files.
- Bound iterations, warmups, output tokens, prompt bytes, samples, and total
  benchmark duration.
- Fixed benchmark prompts contain no user memory or private data.
- Ephemeral mode never writes active state.
- Cleanup runs in `finally`; cleanup failure makes the command fail.
- Never signal a PID without verified ownership.
- Text/JSON output excludes raw prompts and model responses by default.

### 13.1 Enforceable resource budgets

One parent cancellation signal covers all phases and is propagated to artifact
inspection/acquisition, spawn, readiness, inference, and sampling.
P1 uses fixed constants; changing or exposing them requires a follow-up spec:

| Budget                              |   P1 value |
| ----------------------------------- | ---------: |
| acquisition                         |     30 min |
| process/API readiness               |      5 min |
| first/warm/measured request         | 2 min each |
| graceful shutdown                   |       10 s |
| total command excluding acquisition |     20 min |

Before pull, require known artifact bytes, free disk for final artifact + full
temporary copy + 10% safety margin, and existing hardware fit with a documented
host-memory reserve. Unknown artifact size refuses `--allow-pull`.

Containment uses one normative checked-integer formula:

```text
reserveBytes = max(2 GiB, ceil(totalHostMemoryBytes / 10))
availableBudgetBytes = availableHostMemoryAtStartBytes - reserveBytes
runtimeNeedBytes = advisorEstimatedHostServingBytes + backendDaemonOverheadBytes
runtimeHeadroomBytes = max(512 MiB, ceil(runtimeNeedBytes / 10))
requestedCeilingBytes = runtimeNeedBytes + runtimeHeadroomBytes
hardLimitBytes = min(availableBudgetBytes, requestedCeilingBytes)
```

All inputs must be sourced and known; subtraction underflow, addition overflow,
non-safe conversion, `availableBudgetBytes <= 0`, or
`hardLimitBytes < runtimeNeedBytes` rejects before spawn. Byte arithmetic uses
checked `bigint` until bounded serialization. `advisorEstimatedHostServingBytes`
includes model, KV cache at effective context, runtime workspace, and applicable
unified-memory accelerator allocation. Discrete accelerator memory is validated
separately and is not double-counted as host RSS. The backend's conservative,
version-pinned daemon overhead is included exactly once; unknown overhead rejects
ephemeral P1. The reserve is subtracted exactly once by
`availableBudgetBytes`—never again from `hardLimitBytes`.

Before spawn, P1 installs the resulting process-tree hard memory limit; unsupported
containment fails ephemeral mode. RSS sampling may trigger earlier cancellation
but does not claim to prevent allocation spikes. Timeout abort
actively closes sockets/streams and terminates owned work; it does not merely
reject a promise.

Acquisition follows at most five manually validated HTTPS redirects. Every hop
and final origin must match the catalog source allowlist; HTTPS downgrade,
credentials, cross-origin authorization forwarding, and loopback/private/
link-local destinations are rejected. Redirect bodies are cancelled, and final
size/digest verification remains mandatory.

Cleanup is triggered by parent cancellation but runs under a fresh independent
shutdown controller that is not already aborted. Its deadline is the shutdown
budget above. Parent cancellation cannot skip cleanup; cleanup cannot extend the
global command indefinitely. Tests assert cleanup runs after caller cancellation
and global timeout.

### 13.2 Bounded runtime responses

Adapters incrementally enforce decoded (post-decompression) limits before JSON
parsing:

- 16 MiB total decoded bytes per response;
- 4096 stream events/chunks;
- 1 MiB per event/line/string;
- bounded arrays/nesting defined in strict Zod schemas;
- generated-token count no greater than requested maximum plus explicitly
  documented protocol framing allowance;
- finite safe integer counts and finite positive durations.

Compressed responses are refused unless decoded bytes are capped during
decompression. Any cap/timeout cancels and destroys the body. Validation errors
never quote raw bodies. Runtime-provided labels cannot determine metric trust or
appear unsanitized in output.

### 13.3 Process-tree ownership and cleanup

Ownership is a process-instance tuple: PID, start identity, canonical executable,
listener address/port, process group/session where available, and verified
parent/descendant relationships. Revalidate immediately before every signal.
Graceful shutdown is bounded; forced termination applies only to still-verified
owned instances. Cleanup verifies all owned descendants/listeners are gone and
is idempotent after startup, readiness, inference, timeout, cancellation, output,
and partial-cleanup failures. A runtime that daemonizes outside a verifiable tree
fails before fixture transmission.

### 13.4 Baseline file boundary (P2)

Open a baseline once with no-follow semantics and validate/read through that same
descriptor. Reject symlinks, directories, FIFOs, sockets, devices, non-regular
files, ownership/permission violations, and descriptor metadata changes. Enforce
a 4 MiB streaming byte cap and read deadline. The strict schema bounds strings,
arrays, nesting, samples, and numbers; it never follows paths embedded in the
document. Missing required fingerprint fields means `inconclusive`. Sanitize
baseline-originated strings and never echo document contents in errors.

### 13.5 Child environment and retained data

Spawn with argument arrays and `shell:false`. Child processes receive a minimal
allowlisted environment; remove proxy, credential, dynamic-loader, unrelated
runtime-endpoint, and telemetry variables unless the runtime explicitly requires
one. Fixtures are repository-owned only—never user memory, chat history,
clipboard, environment values, arbitrary files, or free-form prompt CLI input.
Child stdout/stderr are never inherited by CLI stdout. Persistent runtime logs
are discarded by default; an implementation that needs readiness logs captures
at most 1 MiB/4096 lines per stream in a bounded ring, strips control bytes and
paths, never emits raw lines in success output/errors, and cancels on overflow.
Discard generated content after bounded protocol validation. Output identity uses
runtime basename/version and a stable process-identity hash; full local paths are
opt-in diagnostics only. No telemetry/evidence upload occurs without separate
approval.

### 13.6 Atomic output on failure

With `--json`, success/regression writes exactly one validated JSON document to
stdout. In text mode, render the complete report into a bounded in-memory string
and write it once only after cleanup verification. Any operational or cleanup
failure writes no success JSON/text report; emit one sanitized stderr diagnostic
and exit 1. Machine-readable failure envelopes are out of P1 scope and require a
versioned schema.

---

## 14. Performance interpretation

The command may label a likely dominant phase using only measured metrics:

- `load`: runtime-reported `modelLoadMs` is at least 60% of the same cold
  request's end-to-end interval.
- `prefill`: median prompt duration is at least 60% of median
  `promptDuration + decodeDuration`.
- `decode`: median decode duration is at least 60% of that same partition.
- `mixed`: both prompt/decode durations are known but neither reaches 60%.
- `memory_capacity`: verified process-tree sampled peak memory is at least 90% of the
  documented serving-memory pool after host reserve; unavailable in P1.
- `unknown`: required metrics do not partition the same interval or are unknown.

Load and request labels are reported separately because one-time startup and
steady-state request costs have different semantics. It must not claim “memory
bandwidth bound” from decode speed alone. That requires
hardware counters or a documented roofline comparison.

---

## 15. Phased delivery

### Phase P1 — Measurement foundation

- fixture/schema/stats modules;
- optional backend benchmark method;
- Ollama and llama.cpp timing parsers;
- ephemeral/active runner;
- text/JSON output;
- real-runtime smoke.

### Phase P2 — Regression comparison

- explicit baseline input;
- comparable fingerprint;
- exit-2 threshold contract;
- CI artifact workflow (without runtime expectation on generic CI hosts).

### Phase P3 — MLX support

- MLX timing/count fields supported by pinned runtime version;
- Apple Silicon process/RSS verification;
- unknown metrics remain unknown;
- real MLX runtime smoke.

### Phase P4 — Evidence-driven tuning experiments (follow-up spec/plan)

- controlled matrix over runtime flags such as context, batch/micro-batch,
  Flash Attention, GPU offload, and KV-cache type;
- one variable changed per experiment unless factorial design is explicit;
- no auto-apply until gains are reproducible and safety/fit remain valid.

---

## 16. Acceptance criteria

Unless explicitly labeled **P2** or **P3**, a criterion gates P1. P2/P3 criteria
gate only their named delivery phase and do not block P1 completion.

### Command and output

- **AC1:** `llmup benchmark <model>` resolves a model and emits text containing
  runtime/model identity, iteration counts, lifecycle metrics, known metric
  aggregates, unknown reasons, and cleanup result.
- **AC2:** `--json` validates against schema v1 and contains raw samples plus
  deterministic aggregates; it contains no raw prompt/response text.
- **AC3:** Invalid flag ranges fail before backend, filesystem, or network calls.

### Measurement correctness

- **AC4:** Warmups are excluded; all measured runs are retained; aggregate tests
  cover odd/even sample counts and deterministic percentile rules.
- **AC5:** Runtime timing/count payloads are Zod-validated; malformed values fail
  closed and never become metrics.
- **AC6:** Missing runtime phase timing is emitted as `unknown`, not wall-clock
  phase inference.
- **AC7:** Fixed fixture hashes are asserted so prompt drift fails tests.

### Lifecycle and safety

- **AC8:** Ephemeral mode refuses occupied ports, writes no state, and always
  uses `existingListener:"reject"`, proves the spawned listener/process tree,
  and stops verified owned instances; failure to clean up makes exit code 1.
- **AC9:** Active mode requires matching model/backend, never pulls, writes state,
  or stops the active process.
- **AC10:** Both modes revalidate loopback listener/process/runtime/model identity
  immediately before sending benchmark content.
- **AC11:** Missing self-managed weights cause a pre-spawn failure unless
  `--allow-pull` is explicit. A missing daemon-managed artifact is refused after
  private-daemon/store inspection but before pull/load unless `--allow-pull` is
  explicit. All pulls require exact immutable source and SHA-256 (never
  size-only), bounded atomic acquisition, and backend-specific ordering.

### Baselines and determinism

- **AC12 (P2):** Comparable baseline regressions produce exit 2 only when a known
  median crosses the configured threshold.
- **AC13 (P2):** Fingerprint mismatch yields `inconclusive` with exit 0, never a false
  regression.
- **AC14:** Benchmark execution does not modify `data/models.json`,
  `data/perf.json`, active state, or user memory.
- **AC15:** `recommend` and `can-run` remain deterministic and make no runtime or
  network calls.

### Backend coverage

- **AC16:** Shared contract tests cover every registered adapter's benchmark
  support or explicit unsupported behavior.
- **AC17:** Ollama and llama.cpp pass real production smoke for load, inference,
  timing extraction, and process identity in active mode on each supported OS;
  ephemeral lifecycle/containment/cleanup additionally passes on delegated-cgroup
  Linux.
- **AC18 (P3):** MLX cannot be marked complete until its real Apple Silicon smoke test
  passes; unsupported phase metrics remain unknown.

### Quality gates

- **AC19:** `npm test`, `npm run typecheck`, changed-file lint, and
  `npm run build` pass.
- **AC20:** Code and security review have no unresolved Critical/Important or
  Critical/High/Medium findings respectively.

### Adversarial and resource acceptance

- **AC21:** A listener stealing the selected port before readiness is detected by
  PID/executable/start/address validation; no fixture is sent and only the
  verified spawned process tree is cleaned up.
- **AC22:** Every runtime request revalidates listener/process/model identity
  before send and after response; active-state drift aborts without state writes
  or stopping unrelated processes.
- **AC23:** Runtime responses are incrementally decoded under byte/event/string/
  nesting/token limits; endless/oversized streams are actively cancelled and
  retained output remains bounded.
- **AC24:** Acquisition, readiness, request, shutdown, and global deadlines are
  tested independently with fake monotonic clocks and active cancellation.
- **AC25:** Pull refuses unknown size, insufficient temporary-copy disk margin,
  digest mismatch, unsafe redirects, symlinks, concurrent promotion races, and
  stale/partial cache artifacts.
- **AC26:** Process-tree cleanup tests cover workers, parent exit, daemonization,
  PID reuse, cancellation, and partial shutdown; no signal uses name/port alone.
- **AC27 (P2):** Baseline reads accept only descriptor-stable regular files under
  the byte/deadline/schema limits and require a complete matching fingerprint;
  otherwise comparison is `inconclusive` with exit 0.
- **AC28 (P2):** Regression exit 2 requires at least five equal known samples and
  both relative and absolute threshold breaches; noisy/incomplete evidence is
  inconclusive.
- **AC29:** Child environment is allowlisted, proxy/credential/loader variables
  are removed, spawn uses arg arrays with `shell:false`, and prompts/results/
  local paths are absent from outputs, logs, errors, and baselines.
- **AC30:** On operational or cleanup failure, stdout contains no success JSON or
  text report; stderr contains one sanitized diagnostic and exit code is 1.
- **AC31:** Fake-monotonic-clock tests assert exact request-start, TTFT,
  generation-end, and decode intervals for synchronous/chunked/fragmented/
  cancelled/zero-duration event sequences; invalid rates are never serialized.
- **AC32:** Warmup or measured-sample failure aborts immediately with no retry,
  replacement, later request, partial aggregate, or success output.
- **AC33 (P2):** Regression/noise tests cover strict threshold boundaries and fixed
  formulas; fingerprint table tests mutate every required field independently
  and stable key ordering makes insignificant JSON whitespace irrelevant.
- **AC34:** P1 uses the validated configured counts (default one warmup + five
  measured) as sequential identical requests in one loaded session with pinned
  generation controls; unsupported seed/control values are explicit fingerprint
  fields.
- **AC35:** Real smoke evidence follows the normative matrix and passes only with
  all samples, finite validated counters, content-free retained output, verified
  cleanup, and no remaining temporary resources.
- **AC36:** Every adapter reports normalized effective generation controls (or
  explicit unsupported markers); requested/effective controls are fingerprinted
  and identical across measured requests.
- **AC37:** Ephemeral execution holds a descriptor/no-follow verified artifact
  lease through every request and shutdown; pathname/inode, manifest, or blob
  substitution at any checkpoint aborts and discards all samples.
- **AC38:** Active mode cryptographically resolves served artifact identity before
  and after every request; missing/mismatched proof rejects P1, while pre/post-
  only proof is explicitly request-unbound and baseline-incomparable.
- **AC39:** Ephemeral P1 admits work only after reserve budgeting, then bounds its
  process tree with a hard memory limit plus sampled memory watchdog; sampled/
  limit breach cancels inference
  and cleans only owned instances, while sampler overhead remains within its
  tested budget.
- **AC40:** Lease release occurs exactly once from `finally`, is idempotent after
  partial failure, removes private snapshots/stores, and leaves verified ordinary
  cache artifacts according to the reported cache disposition.
- **AC41:** Active mode proves exact served source/digest/size before and after
  every first, warmup, and measured request; failed or changed proof aborts without
  stopping or mutating the active instance.
- **AC42:** Daemon-managed ephemeral execution starts an isolated private daemon/
  store before inspect/pull, sends acquisition only to that exact endpoint,
  leases the complete manifest/blob set, and never reads a default/user daemon.
- **AC43:** Backend payload tests prove each effective generation control, and
  strict result refinement rejects any disagreement among root configuration,
  fingerprint, runtime, integrity, or cache evidence.
- **AC44:** Daemon-managed catalog entries provide immutable expected manifest and
  complete blob-set evidence; missing/extra/duplicate/mismatched blobs reject
  acquisition, serving, request checkpoints, result validation, and comparison.
- **AC45 (P2):** A result without request-scoped leased/cryptographic artifact
  binding—including every P1 active result—is `inconclusive` and cannot exit 2.
- **AC46:** A per-command lease manager and typed benchmark serve options carry
  private-store, artifact, and containment resources without stateful adapters;
  unknown/cross-session/released IDs and crash leftovers fail closed or are
  ownership-safely scavenged.

---

## 17. Boundaries

### Always

- Measure before optimizing.
- Use fixed/versioned fixtures and record fingerprints.
- Keep advice deterministic/offline.
- Verify weights and process/model identity.
- Bind loopback and clean up owned processes.
- Report unknown instead of inventing a metric.
- Run mocked tests before real runtime smoke.

### Ask first

- New runtime dependencies.
- Changing catalog/performance dataset schemas beyond the additive daemon
  evidence approved with this spec.
- Persisting benchmark data under the rigspark home layout.
- Automatically applying runtime tuning flags.
- Adding non-loopback or remote benchmarking.
- Sending benchmark evidence to any external service.

### Never

- Train/calibrate global advice automatically from one user's run.
- Include user memory, conversation history, secrets, or arbitrary files in
  benchmark prompts.
- Disable integrity, identity, timeout, or cleanup checks for speed.
- Kill a process by name/port without verified ownership.
- Run real runtimes or network downloads inside Vitest.
- Fabricate unsupported metrics.

---

## 18. Draft decisions requiring human approval

- `ephemeral` is the default; `active` is explicit.
- P1 uses exit 0/1 only. P2 adds exit 2 for conclusive regression.
- Defaults are 5 measured runs after 1 warmup.
- P1 ephemeral mode requires verified process-tree hard memory containment;
  bounded-overhead process-memory sampling is secondary and output labels it as an
  estimate.
- P1 includes Ollama and llama.cpp through one shared contract.
- Benchmark evidence has no implicit repository/home storage location; users
  save JSON explicitly and curated promotion is a separate reviewed workflow.

Implementation must not begin until this specification and these draft decisions
are approved.
