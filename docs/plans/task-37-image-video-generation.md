# Task 37: Local Image and Video Generation (ComfyUI)

> Spec: [image-video-generation.md](../specs/image-video-generation.md)
> Status: **T1–T10 complete**
> Last updated: 2026-10-04

## Architecture

```
rigspark-core     data/generation.json ─► generation.rs (schema, validate, resolve, fit, text)
                                              │
rigspark-runtime  acquire.rs (+comfyui, timeout) ◄─ generation.rs (ComfyUI client, workflows,
                  http.rs (+query)              ◄─   allowlist, install, submit/poll/fetch)
                                              │
rigspark-cli      native.rs: `catalog --generation`, `generate <kind|id> --prompt …`
```

Dependency order: T1 → T2 → T3 → T4 → T5. T2 helpers are independent of T1.

## Tasks

- [x] **T1 — Generation catalog (core)**
  - Acceptance: `generation.json` with FLUX.1 schnell fp8 + Wan 2.1 T2V 1.3B; typed
    validation of every spec §3 rule; `resolve`, `fit` (spec §4), `catalog_text`.
  - Verify: `cargo test -p rigspark-core --locked --test generation`
  - Files: `crates/rigspark-core/data/generation.json`, `src/generation.rs`, `src/lib.rs`,
    `tests/generation.rs`

- [x] **T2 — Runtime primitives**
  - Acceptance: `Artifact` accepts backend `comfyui`; `Acquisition::with_timeout`;
    `http::Request::with_query` (encoded pairs, still loopback-validated).
  - Verify: `cargo test -p rigspark-runtime --locked --lib` + existing acquire tests
  - Files: `crates/rigspark-runtime/src/acquire.rs`, `src/http.rs`

- [x] **T3 — ComfyUI client + workflows (runtime)**
  - Acceptance: spec §5 steps 1–7 with injected transports; `ensure_local_only`
    allowlist; typed `GenerationError`; output persisted without clobbering.
  - Verify: `cargo test -p rigspark-runtime --locked --test generation`
  - Files: `crates/rigspark-runtime/src/generation.rs`, `src/lib.rs`, `tests/generation.rs`

- [x] **T4 — CLI surface**
  - Acceptance: `generate` command + `--prompt/--output/--seed/--comfyui-dir`,
    `catalog --generation`; validation before any network/state access; help fixtures and
    command-list contract tests updated.
  - Verify: `cargo test -p rigspark-cli --locked` (contract + new `generation_cli`)
  - Files: `crates/rigspark-cli/src/native.rs`, `src/native_args.rs`,
    `tests/generation_cli.rs`, existing contract tests/fixtures

- [x] **T5 — Docs + full verification**
  - Acceptance: README usage section; spec checklist ticked; fmt, Clippy, workspace tests,
    `cargo native-retirement` pass.
  - Files: `README.md`, this plan, spec

## v1.1 tasks (TUI + GUI + live verification)

- [x] **T6 — Shared native entry point**: `prepare`/`run_native`/`Generator` in runtime; CLI refactored onto it.
- [x] **T7 — Terminal UI**: `tui_generate.rs` form + progress loop; `tests/tui_generate.rs` (TestBackend + fake generator).
- [x] **T8 — Browser GUI**: `rigspark-gui/src/generation.rs` job API + Create view (`generate.js`); `tests/generation_api.rs`.
- [x] **T9 — Resumable downloads**: `DownloadTransport::get_from` + Range resume in `Acquisition`; tests in `tests/acquire.rs`.
- [x] **T10 — Live verification** (ComfyUI 0.38.0, torch 2.14.1/MPS, Apple M4 Max 36 GB):

  | Surface | Image (FLUX.1 schnell) | Video (Wan 2.1 1.3B) |
  | --- | --- | --- |
  | CLI | ✅ coherent 1024² PNG (seed 42) | ⚠️ 832×480×33 WebP produced; content corrupt |
  | TUI | ✅ coherent PNG (seed 1234) | ⚠️ produced; content corrupt |
  | GUI | ✅ coherent PNG previewed inline (seed 99); live cancel verified | ⚠️ produced, previewed; content corrupt |

  Findings:
  1. A CDN drop discarded 60% of a 16 GiB download, which led to resumable downloads (T9).
  2. Wan on MPS produced corrupt video at the official settings. Bisection on the same graph:
     results were unchanged under `--force-upcast-attention --fp32-vae`, `--bf16-unet`, `--use-pytorch-cross-attention`,
     and `--force-fp32`, so precision and the attention kernel are ruled out. The variable is the **sampler**:
     `uni_pc` diverged as step count rose, while `euler` at the full official 832×480×33, 30 steps, was coherent
     (same prompt/seed that failed). **Fix:** when ComfyUI's `/system_stats` reports an `mps` device, the
     Wan workflow uses `euler`. Other devices keep the official `uni_pc`. Size, length, and steps are unchanged.
     (A 9-frame fallback was considered and rejected: it failed its 30-step validation.)
  3. MPS lacks fp8: the 16.1 GiB FLUX fp8 checkpoint loads as 22.7 GB bf16, so file-size fit is optimistic on MPS.

## v1.2 — generation from chat

- [x] **T11 — Output history**: GUI serves the last 64 finished outputs by job id (`earlier_outputs_stay_viewable_after_later_jobs_finish`).
- [x] **T12 — Chat toggle**: Text · Image · Video radiogroup in the composer; capture-phase submit routing
  in `generate.js`; inline progress → media bubble; Stop cancels. Live: "create mount fuji" → inline
  FLUX image with 0 `/api/chat` requests.
- [ ] **T13 — Live chat video**: "create video of running horse" via the Video toggle. First attempt
  sampled ~250 s/step (vs ~60–77) because FLUX stayed resident after a chat image: ComfyUI reported
  4.9 GB free and 6 GB swap on 36 GB unified memory. Chat Stop verified live (`execution_interrupted`).
- [x] **T14 — Make room between workflows**: before submitting a different workflow than ComfyUI's last
  prompt (`/history?max_items=1`), call ComfyUI's core `/free`, always on Apple MPS (shared memory) and
  on discrete GPUs only when reported free memory is below the model's weights. A first, weights-only
  threshold did not trigger live (ComfyUI reported ≥ 9.2 GiB free) yet Wan still sampled at ~200 s/step
  beside FLUX, so activations and upcast encoders dominate. Same-model repeats keep their warm cache.

## Risks

| Risk | Mitigation |
| --- | --- |
| ComfyUI node/input names drift | Graphs copied from official example metadata; node errors surfaced verbatim (sanitised) |
| Large downloads exceed the 30 min acquisition default | `with_timeout` raises the limit for generation weights only |
| User points `--comfyui-dir` at a different install than the running server | `/models/<folder>` visibility check fails closed with a clear message |
| Weight-only fit is optimistic for activations | Verdict text states it is memory-of-weights only; speed stays `unknown` |
