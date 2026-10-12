# Spec: rigspark

> Status: **Draft (v0.1)** — pending human approval and sub-agent review.
> Last updated: 2026-08-04

## 1. Objective

### Accepted native extension: BitNet (2026-10-11)

The native Rust implementation will catalog `bitnet-b1.58:2b` and run the
official Microsoft I2_S GGUF through a distinct `bitnet` backend. The user
supplies a compiled BitNet server with an absolute `RIGSPARK_BITNET_SERVER`
path; RigSpark never substitutes stock llama.cpp or builds the runtime.
Existing GGUF integrity metadata and shared process/HTTP boundaries are reused,
without a catalog schema change or new Rust dependency.

Acceptance: pinned, digest-verified model acquisition; CPU-only, loopback-only
serving with custom ports and a maximum 4096-token context; lifecycle, chat,
doctor and CLI/GUI backend selection; ownership-safe stop; RAM-based fit advice
and unknown unsourced speed/geometry/quality. I2_S must never be advertised as
compatible with stock llama.cpp, Ollama, MLX or LM Studio.

Implementation order: core catalog/isolation tests, runtime lifecycle tests,
user-facing integration and documentation, then workspace tests/build/Clippy/
format/native-retirement checks. Tests inject network and process boundaries;
live inference requires a separately approved model download and supplied
BitNet executable. Automatic runtime building and GPU support are out of scope.

`rigspark` is a hardware-aware, single-command CLI (npm package) that lets a
user go from "nothing installed" to "a running local LLM server" in one line,
and to switch models — carrying their memory across — in one more line.

It solves four problems:

1. **Discovery** — "Which local LLM should I run on _this_ machine?" The tool
   inspects the hardware and prints a **ranked** list of open-weight models that
   will actually run well, each with a copy-pasteable install command.
2. **Install + serve** — One command downloads a model and spins up a local,
   OpenAI-compatible server.
3. **Memory portability** — One command migrates _all_ memory (conversation
   history, system prompt, saved facts, embeddings) from one model to another.
4. **Freshness** — A weekly GitHub Actions pipeline enriches the model catalog
   with newly released open-weight models so recommendations never go stale.

### Target users

- Developers and enthusiasts who want a local LLM without researching hardware
  requirements, quantization formats, or backend engines.
- Anyone switching models frequently who does not want to lose context/history.

### Success looks like

- A user on any of macOS (Apple Silicon + Intel), Linux, or Windows runs a
  single `npx` command and gets a correct, ranked recommendation in < 5s.
- `up` produces a reachable OpenAI-compatible endpoint with a passing health
  check.
- `migrate` moves memory such that the new model answers a "what did we talk
  about?" style prompt using pre-migration context.
- The catalog gains new open-weight models automatically each week via PR.

---

## 2. Tech Stack

- **Language:** TypeScript ~5.x, `strict: true`, **no `any`**.
- **Runtime:** Node.js ≥ 18 (native `fetch`), ESM (`"type": "module"`).
- **CLI parsing:** **`cac`** (tiny, typed, well-tested) for arg/subcommand
  parsing. Named exports only.
- **Validation:** **Zod** for all external inputs (CLI args, catalog JSON, API
  responses, config files).
- **Hardware detection:** `systeminformation` (cross-platform CPU/RAM/GPU/disk),
  with a `nvidia-smi` / WMI fallback for VRAM and a detection **timeout** that
  yields a safe default profile. Apple-silicon unified memory handled explicitly.
- **Inference backend:** **Ollama** is the sole v1 backend (spawned/managed as a
  child process, OpenAI-compatible API on `http://127.0.0.1:11434`). Backend is
  abstracted behind a `BackendAdapter` interface so MLX (Apple-silicon
  fast-follow), `llama.cpp`, and LM Studio can be added post-v1 without touching
  command code.
- **Embeddings:** `nomic-embed-text` served via the same Ollama backend; the
  embedding model id + dimension are recorded in each store's `meta.json`.
- **Testing:** **Vitest**. Mock all network, filesystem, and child-process
  boundaries — never download a real model or hit a real registry in tests.
- **Lint/format:** ESLint + Prettier.
- **Runtime deps kept minimal** — new deps require approval (see Boundaries).

---

## 3. CLI Surface

All commands runnable via `npx rigspark <command>` or, once installed
globally, `llmup <command>`.

| Command     | One-liner                                                  | Purpose                                                    |
| ----------- | ---------------------------------------------------------- | ---------------------------------------------------------- |
| `recommend` | `npx rigspark` (default) or `npx rigspark recommend` | Detect hardware, print ranked models + install commands.   |
| `up`        | `npx rigspark up <model>`                               | Install (if needed) + start local server for `<model>`.    |
| `chat`      | `npx rigspark chat [-m <model>]`                        | Interactive/piped chat that **records memory** (see §3.5). |
| `down`      | `npx rigspark down [model]`                             | Stop the local server owned by rigspark.                |
| `switch`    | `npx rigspark switch <model>`                           | Make `<model>` the active served model (no memory move).   |
| `migrate`   | `npx rigspark migrate --from <a> --to <b>`              | Move all memory from model `<a>` to `<b>`.                 |
| `ls`        | `npx rigspark ls`                                       | List installed models + which is active (from state).      |
| `catalog`   | `npx rigspark catalog [--refresh]`                      | Show/refresh the model catalog.                            |
| `doctor`    | `npx rigspark doctor`                                   | Diagnose hardware, backend, disk, ports, state.            |

> **Runtime state.** Commands that reason about "what is running" read/write a
> single state file `~/.rigspark/state.json` (validated by Zod, written
> atomically, guarded by a lock file — see §3.6). `ls`, `down`, `switch`,
> `doctor`, and `chat` all resolve the active model/endpoint through this state
> module rather than probing processes ad hoc.

### 3.1 `recommend` (the hardware-aware ranker)

**Behavior**

1. Detect hardware → `HardwareProfile`.
2. Load catalog (`models.json`), validate with Zod.
3. For each model+quantization, compute required memory and filter to those that
   fit within a safety headroom.
4. Score and rank the survivors.
5. Print a table: rank, model, params, quant, est. RAM/VRAM, license, why, and
   the exact `rigspark up <model>` command for #1.

**Ranking model (deterministic, documented, unit-tested)**

```
fits(model, hw)      = requiredMemory(model, quant) <= usableMemory(hw) * (1 - HEADROOM)
score(model, hw)     = wQ * qualityScore        // param-class + benchmarkProxy, normalized 0..1
                     + wF * fitScore             // rewards using hardware well, penalizes near-OOM
                     + wS * speedScore           // estimated tok/s heuristic (llmfit-style), 0..1
                     + wR * recencyScore         // newer open-weight releases rank higher
                     + wC * capabilityMatch      // chat/code/vision vs. requested --task
```

- **`speedScore`** is a **deterministic** estimated-throughput heuristic (no live
  benchmarking in v1): derived from `activeParams` (MoE) or total params (dense),
  the selected quant, and hardware class (GPU VRAM bandwidth vs. CPU/unified).
  Live on-device benchmarking is deferred (see Decisions).
- **Each model is ranked at its best-fitting quantization** (highest quality quant
  whose `requiredMemory` satisfies `fits()`); a model appears once, not once per
  quant.
- Only models where `fits() === true` are ranked; the rest are listed under a
  "won't fit" section with a typed reason (`ram-bound` | `vram-bound` |
  `disk-bound`).
- **Determinism rules (all unit-tested):**
  - `recencyScore` is computed against a **fixed reference date =
    `catalog.generatedAt`**, never the wall clock, so output cannot drift
    day-to-day. A future `releaseDate` clamps to `[0, 1]`.
  - Weights (`wQ/wF/wS/wR/wC`) and `HEADROOM` live in one `weights.ts` constants
    module; the five weights must sum to 1 (invariant asserted by a test).
  - **Tie-break** is explicit and stable: equal composite score → order by
    `benchmarkProxy` desc, then `releaseDate` desc, then `id` asc.
- `--task chat|code|vision` and `--json` flags supported. When `--task` is
  omitted, `capabilityMatch` is neutral (does not penalize any model). `--json`
  emits the ranked list for scripting.

> **Single source of truth for memory math.** `requiredMemory` (ranker) and the
> per-quant `minRamBytes`/`minVramBytes` written by the enrichment pipeline
> derive from the **same** `hardware/memory-math.ts` formula module. The pipeline
> imports it so `fits()` filtering can never diverge from catalog values.

**`usableMemory` rules**

- Apple Silicon: unified memory → usable = total RAM − OS reserve.
- Discrete NVIDIA/AMD GPU: usable = VRAM for GPU-offloaded quants; falls back to
  free system RAM for CPU inference with a quality/speed penalty in `fitScore`.
- Integrated GPU sharing system RAM: treated as CPU/shared-memory path — shared
  memory is **not** double-counted as both VRAM and RAM.
- Multi-GPU: v1 uses the single largest VRAM pool (models rarely span GPUs);
  aggregation is deferred (see Decisions).
- CPU-only: usable = **free** system RAM − reserve (use free, not total, to avoid
  recommending models that OOM under real load).

### 3.2 `up` (install + serve, one line)

1. Resolve `<model>` against catalog (fuzzy: `llama3.1`, `llama3.1:8b`,
   `llama3.1:8b-q4_K_M`). Ambiguity → error listing candidates. The resolved id
   is validated against `^[a-z0-9._:\/-]+$` before any use.
2. **Disk preflight** — compare the resolved quant `diskBytes` against free disk;
   abort with a clear error if insufficient.
3. Ensure backend present. If Ollama missing: print the OS-specific install
   command and exit non-zero (do **not** silently install system software —
   see Boundaries). `--install-backend` opt-in flag allowed.
4. **Daemon ownership.** Ollama is a single shared daemon serving all pulled
   models on one port. rigspark checks state/health: if a daemon is already
   running it **attaches** (records `ownedByUs: false`); otherwise it **spawns**
   one (`ownedByUs: true`) and records pid/port/endpoint in `state.json`.
   `down` only stops daemons rigspark started.
5. Pull the model via the backend adapter with `spawn(..., { shell: false })`,
   every argument a discrete array element (no shell string). Stream progress.
6. **Integrity check** — verify the pulled quant against the catalog’s recorded
   SHA-256 digest; fail closed on mismatch.
7. Ensure the server is bound to `127.0.0.1` (never `0.0.0.0` without `--host`).
8. Health-check the OpenAI-compatible readiness probe (`GET /v1/models`, with
   `/api/tags` fallback) with bounded retries/backoff until ready or timeout.
   On serve/health failure, clean up any process rigspark spawned (no orphans).
9. Update `state.json`; print the endpoint, model id, and an example OpenAI-
   compatible `curl`.

### 3.3 `migrate` (memory portability, one line)

`npx rigspark migrate --from llama3.1:8b --to qwen2.5:14b`

**What "memory" means** — the rigspark-managed store per model
(`~/.rigspark/memory/<model-id>/`):

- `conversation.jsonl` — chat history (role/content/timestamps).
- `system.md` — active system prompt / persona.
- `facts.json` — durable user facts ("memories").
- `embeddings/` — optional vector store (index + vectors + source chunks) with
  the embedding model + dimension recorded in `meta.json`.

**Migration steps**

1. Load source memory; validate with Zod (memory files carry their own
   `schemaVersion` for forward-compatible layout changes).
2. **Context-window remap** — if target context window is smaller, summarize/
   truncate oldest turns (strategy: keep system prompt + facts, summarize old
   history into a compact "prior summary" turn). Never drop `facts.json`.
   Summarization uses the **target model if it is running**, else a
   deterministic rule-based truncation fallback; the chosen strategy is recorded
   in the run summary. `migrate.ts` reaches the backend only through the
   `BackendAdapter` interface.
3. **Re-embed if needed** — if the target's embedding model/dimension differs,
   re-embed source chunks with the target embedding model; otherwise copy the
   index as-is. (Trigger matrix: same model+dim → reuse; any difference → re-embed.)
4. Write to target memory dir atomically: stage in a temp dir **inside
   `~/.rigspark` with `0700`** (never shared `/tmp`, and same filesystem so
   `rename` is atomic), then rename. Source is left intact unless `--move` is
   passed; with `--move`, source is deleted **only after** the target write is
   fully committed — on any failure the source is preserved (no data loss).
5. Print a summary: turns carried, turns summarized, vectors re-embedded,
   summarization strategy used.

`--dry-run` prints the plan without writing.

### 3.5 `chat` (the memory-capture path)

`migrate` is only meaningful if something _writes_ the memory store. `chat` is
that producer: rigspark sits in the request path as a thin recorder.

1. Resolve the active model/endpoint from `state.json` (or `-m <model>`).
2. Forward each user turn to the backend's OpenAI-compatible endpoint and stream
   the response back.
3. Append `{role, content, ts}` turns to `conversation.jsonl`; extract durable
   user facts into `facts.json` (rule-based v1; model-assisted extraction
   deferred — see Decisions); optionally chunk + embed content into `embeddings/`
   with the embedding model + dimension recorded in `meta.json`.
4. All registry/model-sourced strings shown in the terminal are stripped of
   control/ANSI sequences before display.

External OpenAI-compatible clients may still talk to the backend directly, but
memory is only captured for traffic that flows through `chat`. This is stated
explicitly so users understand what `migrate` can and cannot carry.

### 3.6 Runtime state & concurrency

- `~/.rigspark/state.json` — Zod-validated record of the active model,
  backend endpoint, daemon pid/port, and `ownedByUs`. Written atomically.
- A **lock file** (`~/.rigspark/lock`) serializes mutating commands
  (`up`/`down`/`switch`/`migrate`) so concurrent invocations cannot corrupt the
  memory store or state.

### 3.4 Catalog & the weekly enrichment pipeline

**Catalog** — `data/models.json`, versioned in the repo, Zod-validated. The
catalog is **exhaustive across open-weight families** — comparable in breadth to
[`AlexsJones/llmfit`](https://github.com/AlexsJones/llmfit) (hundreds of models,
its `data/` catalog serves as a cross-check reference), scored across quality /
fit / recency / capability dimensions.

**v1 ships a complete catalog of every open-weight model released to date;** the
weekly pipeline thereafter only **adds newly released** models (and updates
derived fields), never re-seeds. The full initial catalog is produced by a
one-time **bootstrap** run of the same enrichment logic in "backfill" mode
(§3.4 pipeline), then committed.

**Seed families (the bootstrap must cover all of these and more).** At minimum,
every family below across its common param sizes and quantizations:

- **Kimi (Moonshot AI)** — `kimi-k2` (K2-Instruct / K2-Base, MoE ~1T total /
  ~32B active, Modified-MIT), `kimi-k2-thinking`, `kimi-vl` / `kimi-vl-a3b`
  (vision MoE), `kimi-dev-72b`, `kimi-linear`. _(Explicitly required.)_
- **Llama** (Meta) — 3.1 / 3.2 / 3.3, 1B–70B.
- **Qwen** (Alibaba) — Qwen2.5 / Qwen3 dense + MoE, 0.5B–72B, incl. Coder.
- **DeepSeek** — V2/V3 (MoE), R1 + distills.
- **Mistral / Mixtral** — 7B, Nemo, Small, Mixtral 8x7B/8x22B (MoE).
- **Gemma** (Google) — 2 / 3, 2B–27B.
- **Phi** (Microsoft) — 3 / 3.5 / 4.
- **GLM** (Zhipu), **Yi**, **Command-R** (Cohere, open-weight tiers),
  **StableLM**, **SmolLM**, **OLMo**, **Granite** (IBM).

Only **open-weight** licenses are admitted (see the license gate). Non-open
(closed API) models are out of scope.

```jsonc
{
  "schemaVersion": 2,
  "generatedAt": "2026-08-04T00:00:00Z",
  "models": [
    {
      "id": "llama3.1:8b",
      "family": "llama3.1",
      "params": "8B",
      "architecture": "dense", // "dense" | "moe"
      "license": "llama-3.1-community", // must be open-weight allow-listed
      "openWeight": true,
      "contextLength": 131072,
      "capabilities": ["chat", "code"],
      "releaseDate": "2024-07-23",
      "source": { "ollama": "llama3.1:8b", "hf": "meta-llama/Llama-3.1-8B" },
      "quantizations": [
        {
          "name": "Q4_K_M",
          "diskBytes": 4900000000,
          "minRamBytes": 6500000000,
          "minVramBytes": 6000000000,
          "sha256": "<hex>",
        },
      ],
      "benchmarkProxy": 0.71, // 0..1 normalized quality proxy
    },
    {
      "id": "kimi-k2:instruct",
      "family": "kimi-k2",
      "params": "1T", // total parameters
      "architecture": "moe",
      "activeParams": "32B", // MoE: params active per token (drives speed, not footprint)
      "license": "modified-mit",
      "openWeight": true,
      "contextLength": 131072,
      "capabilities": ["chat", "code"],
      "releaseDate": "2025-07-11",
      "source": { "ollama": "kimi-k2", "hf": "moonshotai/Kimi-K2-Instruct" },
      "quantizations": [
        {
          "name": "Q4_K_M",
          "diskBytes": 620000000000,
          "minRamBytes": 640000000000,
          "minVramBytes": 640000000000,
          "sha256": "<hex>",
        },
      ],
      "benchmarkProxy": 0.93,
    },
  ],
}
```

> **MoE memory math.** For MoE models, `requiredMemory` is driven by **total**
> parameters (all experts must be resident), while `activeParams` feeds the
> speed/quality signals only. This is why a model like `kimi-k2` correctly lands
> in the "won't fit" section on consumer hardware with a `ram-bound` reason,
> rather than being mis-ranked as an 8B-class model. `memory-math.ts` has an
> explicit `dense` vs `moe` branch, unit-tested for both.

**Pipeline (`.github/workflows/catalog-refresh.yml`)** — runs weekly (cron)
and on manual `workflow_dispatch`. It has two modes sharing one code path:
**backfill** (one-time bootstrap that builds the complete v1 catalog) and
**incremental** (the weekly default — fetch only releases newer than the
catalog's newest `releaseDate`/known ids, add/update them, and never re-seed
existing entries):

1. Fetch candidate open-weight releases from Hugging Face API (filter by
   allow-listed license + text-generation/vision tasks) and the Ollama library.
   In **incremental** mode, only releases **newer than the catalog** are
   considered (across the seed families **plus any newly trending open-weight
   org/model** — e.g. new Moonshot/Kimi, Qwen, DeepSeek releases). In
   **backfill** mode, the full open-weight set is swept. All fetch hosts are
   **allow-listed** (`huggingface.co`, the Ollama registry); non-HTTPS or
   private/loopback/link-local targets are rejected (anti-SSRF).
2. Enrich each: architecture (dense/MoE + activeParams), quant disk sizes and
   min RAM/VRAM **via the shared `memory-math.ts` formula**, capabilities,
   context length, release date, SHA-256 digest.
3. **License gate** — the open-weight allow-list is enforced **in the Zod schema
   itself**, so both CI enrichment _and_ the runtime catalog loader reject any
   non-allow-listed license. A bad merge cannot smuggle a closed model past the
   runtime.
4. **Merge reconciliation (merge-by-`id`)** — registry data updates only
   derived/technical fields; hand-curated fields (e.g. `benchmarkProxy`) are
   preserved. New quants are added; upstream-removed entries are flagged, not
   silently dropped. Enrichment is **idempotent** (re-running on the same inputs
   produces no diff). A partial upstream failure degrades gracefully and never
   wipes the existing catalog. A catalog size cap keeps `models.json` bounded.
5. Validate the merged catalog with the Zod schema; run `npm test`.
6. Open a **pull request** with the catalog diff (labeled `catalog`,
   `automated`) for **human review** — the pipeline never pushes to `main`
   directly.

**Workflow hardening:** explicit least-privilege `permissions:`
(`contents: read`, `pull-requests: write`); all actions pinned to full commit
SHAs; triggers limited to `schedule` + `workflow_dispatch` (never
`pull_request_target`); any HF token is a masked secret and never echoed;
enrichment output is **data-only** (no code executed from fetched content).

---

## 4. Project Structure

```
data/
  models.json               → Versioned model catalog (CI-enriched)
src/
  cli.ts                    → Entry point / arg dispatch (named exports)
  commands/                 → One file per command (handler + Zod schema)
    recommend.ts
    up.ts
    chat.ts
    down.ts
    switch.ts
    migrate.ts
    ls.ts
    catalog.ts
    doctor.ts
  hardware/
    detect.ts               → HardwareProfile detection (systeminformation)
    memory-math.ts          → usableMemory / requiredMemory — shared by ranker + pipeline
  ranking/
    rank.ts                 → Deterministic scoring + filtering + tie-break
    weights.ts              → Scoring weights (sum to 1) + HEADROOM constants
  catalog/
    schema.ts               → Zod schema for models.json (dense/MoE, license allow-list gate)
    load.ts                 → Load + validate catalog
    enrich.ts               → Pipeline enrichment logic (HF/Ollama fetch + merge)
  backend/
    adapter.ts              → BackendAdapter interface
    ollama.ts               → Ollama adapter (spawn shell:false, pull, serve, health)
  memory/
    store.ts                → Read/write per-model memory store (schemaVersion'd)
    migrate.ts              → Context remap + re-embed logic
    capture.ts              → Records turns/facts/embeddings for `chat`
  state/
    state.ts                → state.json read/write (atomic) + lock file
  config.ts                 → Paths (~/.rigspark), env, defaults
  errors.ts                 → Typed error classes
tests/                      → Mirrors src/ (Vitest)
.github/workflows/
  ci.yml                    → lint + typecheck + test + build
  catalog-refresh.yml       → Weekly catalog enrichment → PR
docs/specs/rigspark.md   → This spec
```

Conventions: **one command per file**, handler + Zod input schema co-located.

---

## 5. Code Style

```ts
// src/hardware/memory-math.ts
import { z } from "zod";

export const HardwareProfileSchema = z.object({
  arch: z.enum(["x64", "arm64"]),
  platform: z.enum(["darwin", "linux", "win32"]),
  totalRamBytes: z.number().int().positive(),
  freeRamBytes: z.number().int().positive(),
  gpu: z
    .object({
      vendor: z.enum(["apple", "nvidia", "amd", "intel", "none"]),
      vramBytes: z.number().int().nonnegative(),
    })
    .array(),
  freeDiskBytes: z.number().int().nonnegative(),
});
export type HardwareProfile = z.infer<typeof HardwareProfileSchema>;

const OS_RESERVE_BYTES = 2 * 1024 ** 3;

/** Memory the machine can realistically give an inference process. */
export function usableMemoryBytes(hw: HardwareProfile): number {
  const gpuVram = hw.gpu.reduce((max, g) => Math.max(max, g.vramBytes), 0);
  const unified = hw.arch === "arm64" && hw.platform === "darwin";
  if (unified) return Math.max(0, hw.totalRamBytes - OS_RESERVE_BYTES);
  if (gpuVram === 0) return Math.max(0, hw.freeRamBytes - OS_RESERVE_BYTES); // CPU-only: use free RAM
  return gpuVram; // discrete GPU: single largest VRAM pool (v1)
}
```

Conventions:

- Files `kebab-case.ts`; Types `PascalCase`; functions `camelCase`; constants
  `SCREAMING_SNAKE_CASE`; CLI subcommands `kebab-case`.
- Named exports only (no default exports).
- Explicit return types on all exported functions.
- Errors **throw** typed errors from `errors.ts`; never return error codes.
- All external input parsed through Zod at the boundary.

---

## 6. Testing Strategy

- **Framework:** Vitest. Tests mirror `src/` under `tests/`.
- **TDD / Prove-It:** write a failing test before code; for bugs, write the
  failing test first, then fix.
- **Mock every boundary:** `fetch` (HF/Ollama registries), `child_process`
  (backend spawn/pull), and filesystem (memory store). No real downloads, no
  real servers, no network in tests.
- **Test levels:** prefer the lowest level that captures the behavior.
  - Unit: ranking math + tie-break, memory-math, context-remap, Zod schemas,
    license gate, path-slug/traversal, state read/write.
  - Integration: `recommend` end-to-end with a fixture catalog + fake hardware;
    `migrate` with a fixture memory store; **atomic-write & `--move` rollback
    against a real tmpdir** (a mock cannot prove atomicity); backend lifecycle
    with mocked `child_process`.
  - e2e (opt-in, not in CI default): real `up` against a locally installed
    Ollama, gated behind an env flag.
- **Determinism harness:** a shared fixture freezes time (`vi.setSystemTime`)
  and pins `catalog.generatedAt` so recency-scoring and cron tests are stable.
- **Coverage targets:** > 80% (**line and branch**) on `src/ranking/`,
  `src/hardware/`, `src/catalog/`, `src/memory/`, **`src/backend/`, and
  `src/state/`** (backend owns process lifecycle + the loopback-binding
  property, so it is in the high tier); > 70% overall.
- Failure paths are first-class: every command tests its error/rollback branch,
  not just the happy path.
- Run `npm test` after every change.

---

## 7. Commands

```bash
npm run build       # tsc
npm test            # Vitest
npm run test:cov    # Vitest --coverage
npm run lint        # ESLint
npm run lint:fix    # ESLint --fix
npm run format      # Prettier
npm run typecheck   # tsc --noEmit
npm run dev         # tsx src/cli.ts
```

---

## 8. Security & Privacy

- **Local-first:** server binds to `127.0.0.1` by default; `--host` requires an
  explicit `--allow-unauthenticated` acknowledgement + warning to expose beyond
  loopback (the endpoint is an unauthenticated model + memory store on a LAN).
- **No system installs without consent:** the tool prints backend-install
  commands but never installs system software silently; `--install-backend` is
  explicit opt-in.
- **No command injection:** the backend adapter spawns with `shell: false` and
  passes every argument as a discrete array element; model ids are validated
  against `^[a-z0-9._:\/-]+$` before use.
- **Registry input is untrusted:** all HF/Ollama API responses validated with
  Zod before use; fetch hosts are allow-listed and private/loopback/non-HTTPS
  targets rejected (anti-SSRF); enrichment runs in CI, output gated behind a
  human-reviewed PR.
- **Weight integrity:** each downloaded quant is verified against the catalog's
  SHA-256 digest; the backend is documented as the weight-parsing trust boundary.
- **License gate in the schema:** only allow-listed open-weight licenses pass the
  Zod schema, enforced in both CI and the runtime loader.
- **Memory store perms:** `~/.rigspark/` created `0700`, files `0600`; on
  startup its ownership (uid) is checked and it must not be a symlink. Atomic
  migrate staging happens inside this dir, never in shared `/tmp`.
- **Path safety:** model ids are mapped to a deterministic safe slug, then the
  resolved path is `realpath`-checked to be inside `~/.rigspark/memory/`
  (defends legit `/`+`:` ids and `..` traversal alike).
- **Terminal safety:** control/ANSI sequences stripped from all registry- and
  model-sourced strings before display (and in `--json`).
- **No secrets in repo:** HF token (if used for rate limits) comes from CI
  secret / env only, masked, never echoed; lockfile committed, `npm audit` in CI.

---

## 9. Boundaries

- **Always:** validate input with Zod, run tests before commits, bind to
  loopback by default, spawn backends with `shell: false`, verify weight
  digests, keep ranking deterministic + tested (tie-break + pinned recency),
  restrict memory-dir permissions, serialize mutating commands via lock file.
- **Ask first:** adding a runtime dependency, adding a new inference backend,
  changing the catalog schema or memory-store `schemaVersion`, changing the
  ranking weights/algorithm, changing state-file layout, CI changes.
- **Never:** install system software silently, add non-open-weight models to the
  catalog, push catalog changes to `main` without a PR, use a shell string for
  child processes, stage migrate writes in shared `/tmp`, commit secrets or
  model weights, use `any`, hit real registries/download real models in tests.

---

## 10. Success Criteria (verifiable)

1. `recommend` on a fixture `HardwareProfile` produces a deterministic ranked
   order (stable tie-break; recency pinned to `catalog.generatedAt`) asserted by
   a unit test; models exceeding usable memory are excluded with a typed reason.
   Empty-catalog and all-models-too-big cases return cleanly (no throw). A large
   MoE model (e.g. `kimi-k2`, 1T total) is sized by **total** params and lands
   in "won't fit" as `ram-bound` on consumer hardware.
   _(Verify: `npm test tests/ranking`)_
2. `usableMemoryBytes` returns unified memory on Apple Silicon, largest VRAM on
   discrete-GPU, and **free** RAM on CPU-only/integrated-GPU profiles without
   double-counting shared memory. _(Verify: `npm test tests/hardware`)_
3. `up <model>` with a mocked backend calls (disk-preflight →) pull → digest-
   verify → serve → health-check in order, binds `127.0.0.1`, and cleans up any
   spawned process on serve/health failure. _(Verify: `npm test tests/commands/up`)_
4. `migrate --from a --to b` on a fixture store: carries `facts.json` unchanged,
   summarizes overflow turns when target context is smaller (asserting the
   summarizer is _called with_ overflow turns), re-embeds only when the embedding
   model/dimension differs, writes atomically, and on `--move` preserves the
   source if the target write fails. _(Verify: `npm test tests/memory`)_
5. Model ids attempting path traversal (`../`, absolute, encoded) are rejected/
   slugged and resolve inside `~/.rigspark/memory/`. _(Verify: `npm test tests/memory`)_
6. Catalog Zod schema rejects a non-open-weight license, missing/negative quant
   bytes, and malformed entries; requires `activeParams` when
   `architecture: "moe"`; `enrich` merge is idempotent and preserves curated
   fields. _(Verify: `npm test tests/catalog`)_
7. `catalog-refresh.yml` runs on the weekly cron with least-privilege
   `permissions:` and SHA-pinned actions, produces a validated catalog diff, and
   opens a PR (never commits to `main`). _(Verify: workflow lint + a dry-run of
   `enrich.ts` against recorded fixtures.)_
8. `npm run build`, `npm run typecheck`, `npm run lint`, `npm test` all pass.

---

## 11. Decisions (recommended route)

The former open questions are resolved as follows for v1:

1. **CLI dep:** **`cac`** — tiny, typed, well-maintained. (Adopted in Tech Stack.)
2. **Backend (v1):** **Ollama only.** Simplest UX and OpenAI-compatible out of
   the box; other engines slot in later via `BackendAdapter`. (Ruled out
   `node-llama-cpp` for v1 to avoid native-build maintenance.)
3. **Embeddings:** **`nomic-embed-text` via Ollama**; model id + dimension
   recorded in `meta.json` so `migrate` re-embeds only on a real change.
4. **Fact extraction:** **rule-based heuristics in v1**; model-assisted
   extraction deferred.
5. **Benchmark proxy:** **frozen at enrich time** — a normalized 0..1 score
   derived from param-class + a pinned public-leaderboard snapshot, written into
   the catalog so ranking is reproducible in CI (never fetched at runtime).
6. **Windows/GPU detection:** `systeminformation` with `nvidia-smi`/WMI fallback
   and a detection **timeout → safe default profile**.
7. **Multi-GPU:** **single largest VRAM pool** in v1; cross-GPU spanning
   deferred.
8. **Weight digests:** prefer the registry-supplied SHA-256; when unavailable,
   fall back to a size check and mark the quant `digestVerified: false` (surfaced
   in `doctor`).
9. **Additional backends:** **MLX (Apple-silicon) is the first fast-follow**
   after v1, then `llama.cpp` and LM Studio — all behind `BackendAdapter`.
10. **Speed estimation:** **deterministic tok/s heuristic ships in v1** as the
    `speedScore` ranking dimension; **live on-device benchmarking is deferred**
    to a post-v1 release.

**Deferred to post-v1 (revisit):** MLX/llama.cpp/LM Studio adapters, live
benchmarking, model-assisted fact extraction, multi-GPU spanning.

---

## 12. Phased Plan (for reference — implement after approval)

1. **Foundations:** scaffold, config/paths, errors, Zod catalog schema (with
   license gate) + loader, state module + lock, fixture `models.json`.
2. **Hardware + ranking:** `detect.ts`, `memory-math.ts` (shared), `rank.ts`,
   `weights.ts` + `recommend` command. _(Ships the headline feature first.)_
3. **Backend + serve:** `BackendAdapter`, Ollama adapter (shell:false, digest
   verify, daemon attach/spawn), `up`/`down`/`switch`/`ls`/`doctor`.
4. **Memory + migrate:** memory store (schemaVersion'd), `chat` capture path,
   context remap, re-embed, `migrate`.
5. **Pipeline:** `enrich.ts` (merge + host allow-list) + `catalog-refresh.yml`
   least-privilege weekly PR flow.

Each phase: implement → test → verify → commit, per
`incremental-implementation` and `test-driven-development`.
