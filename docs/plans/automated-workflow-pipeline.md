# Implementation Plan: Automated Generation Workflow Pipeline

Spec: [automated-workflow-pipeline.md](../specs/automated-workflow-pipeline.md)
Status: approved; phase 1 implemented (see below)
Date: 2026-10-16

## Approved answers to the spec's open questions

- CPU ComfyUI (Python and CPU-only Torch) may run in the harvest CI job only.
- MP4 validation uses a small in-house bounded box parser; no new dependency.
- Verification uses the template's own default prompt, recorded in evidence.
- A verified workflow stays runnable while its updated template is still a candidate.

## Phases and delivery

Each phase is one PR that leaves `main` releasable. Every phase must pass fmt,
Clippy (`-D warnings`), full tests, build and `cargo native-retirement`.

| Phase | PR | Outcome | Depends on |
|---|---|---|---|
| 1 | Data-driven runtime | Six workflows run from files; per-workflow Rust removed | none |
| 2 | Template compiler | UI→API compiler with binding detection on recorded templates | 1 |
| 3 | Harvest and pin | `cargo workflow-harvest --dry-run`, multi-file roles, LoRA, report | 2 |
| 4 | CI validation and proposals | Weekly workflow; CPU ComfyUI `/prompt` validation; candidate PRs | 3 |
| 5 | Verify, promote, MP4 | Evidence files, CI gate, MP4 output in CLI and GUI | 1, 3 |
| 6 | Real proof | Re-verify the six on MPS, remove legacy allowance, promote one new workflow | 4, 5 |

Phases 4 and 5 can proceed in parallel once phase 3 lands.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| UI→API conversion is subtle (widget order, `control_after_generate`, subgraphs, bypass) | Wrong graphs | Frozen golden fixtures from real templates; ComfyUI's own `/prompt` validation in CI; ambiguous cases fail closed |
| Migration changes behavior of shipped workflows | Regressions | Byte-identical golden tests against the existing builders before deletion |
| Six shipped workflows lack evidence files | Promotion gate would block them | Temporary `legacy-builtin` allowance for those six ids at frozen digests, removed in phase 6 after MPS re-verification |
| Template churn upstream | Noisy PRs | Pinned commit, skip unchanged templates, cap 10 candidates, one pending PR |
| Large models exceed local hardware | Cannot verify | Evidence is per device class; unverified stays fit-only; never claim CUDA from MPS |
| CPU ComfyUI install in CI is slow or flaky | CI noise | Harvest workflow only; cached venv; pinned ComfyUI commit and Torch CPU wheel |

## Phase 1 status (2026-10-16)

Done: 1.1, 1.2, 1.3, 1.5 and 1.6. The six workflows are bundled data files under
`crates/rigspark-core/data/workflows/`. The runtime builds every graph through
`WorkflowFile::apply`. Device overrides, output nodes and native-capability preflight
markers are data. Builders, `save_node`, the Wan sampler helper and the Ming-only
preflight are removed. Graphs match the frozen legacy output byte-for-byte for all
six workflows on MPS, CUDA and no-device runs.

Deferred: **1.4 (catalog schemaVersion 3, string workflow ids) moves to phase 3**,
where the first non-built-in id appears. Until then the `Workflow` enum stays the id
type, and the registry enforces exactly one file per id. This avoids a schema change
with no consumer.

Real-hardware checkpoint (36 GiB Apple Silicon, ComfyUI 0.38.0, owned loopback server):

- Ming Design: complete real generation through the data-driven path, including the
  generic native-capability preflight. Output: 1024x1024 RGBA PNG.
- Wan 2.1 1.3B: ComfyUI accepted the data-driven graph, and the running queue showed
  the MPS `euler` override, the bound prompt and seed, and the pinned FP16 weights at
  832x480x49. Sampling ran at about 243-333 s/step under memory pressure. The run was
  stopped by decision after 2 of 30 steps, so **completion was not observed**. The
  graph is byte-identical to the previously shipped builder.

## Phase 1 tasks: data-driven runtime

- [ ] **1.1 Workflow file model and registry (core).** Add `crates/rigspark-core/src/workflow/`
  with typed `WorkflowFile`, `Binding`, `ModelSlot`, `Override`, `Contract`, `Status`, and
  a canonical digest. Load the bundled registry from `data/workflows/*.json` via
  `include_str!`. Validate ids, bounds, binding targets and digest.
  - Acceptance: unknown fields rejected; digest stable across key order; dangling bindings rejected.
  - Verify: `cargo test -p rigspark-core --test workflow_file`.
  - Files: `core/src/workflow/{mod,file,digest}.rs`, `core/src/lib.rs`, `core/tests/workflow_file.rs`.
- [ ] **1.2 Encode the six workflows as data files.** Generate each file from the current
  `json!` builder with placeholder bindings. Record the Wan MPS sampler override and Wan
  negative prompt as data.
  - Acceptance: six files present; registry loads them.
  - Verify: core registry test.
  - Files: `core/data/workflows/*.json` (6).
- [ ] **1.3 Golden equivalence.** For each workflow and for MPS and non-MPS devices,
  `apply(file, prompt, seed, names, devices)` equals the legacy builder output exactly.
  - Acceptance: all twelve cases are byte-identical.
  - Verify: `cargo test -p rigspark-runtime --test workflow_golden`.
  - Files: `runtime/src/workflow/apply.rs`, `runtime/tests/workflow_golden.rs`.
- [ ] **1.4 Catalog schemaVersion 3.** Workflow becomes a string id checked against the
  registry; v2 files still parse. Runnable requires a verified file or the frozen
  `legacy-builtin` allowance.
  - Acceptance: unknown id rejected; candidate is non-runnable even with bypass.
  - Verify: core generation tests and CLI/GUI fit-only regressions.
  - Files: `core/src/generation.rs`, `core/data/generation.json`, related tests.
- [ ] **1.5 Generic executor and preflight.** Route `generate` through `apply`. Derive the
  node allowlist and output node from the file. Generalize preflight from Ming to any
  `contract`, with the legacy allowance for files that have no contract yet.
  - Acceptance: existing runtime, CLI and GUI generation suites pass unchanged.
  - Verify: `cargo test -p rigspark-runtime --test generation`, GUI `generation_api`, CLI `generation_cli`.
  - Files: `runtime/src/generation.rs`, `runtime/src/generation/preflight.rs`.
- [ ] **1.6 Remove per-workflow Rust.** Delete the builders, the `Workflow` enum branches
  and `save_node`. Keep golden expectations as frozen JSON fixtures.
  - Acceptance: no workflow-specific graph code remains; full workspace passes.
  - Verify: full gates.
  - Files: `runtime/src/generation.rs`, `core/src/generation.rs`, `core/src/generation_admission.rs`.

## Phase 2 tasks: template compiler (outline)

- 2.1 Record fixtures: four templates at the pinned commit, plus `object_info` for the
  classes used, captured from local ComfyUI 0.38.0.
- 2.2 Parser: bounded UI-format model (nodes, links, subgraphs, modes).
- 2.3 Flattening: subgraphs, reroutes, bypass pass-through, muted nodes, frontend-only nodes.
- 2.4 Widget mapping using `object_info` order, including `control_after_generate`.
- 2.5 Binding detection (prompt, seed, output) with explicit ambiguity errors.
- 2.6 Equivalence tests: the compiled Wan 2.1, Qwen-Image and Ming graphs match the
  phase-1 data files modulo documented differences.

## Phases 3-6

Detailed task lists are written when the preceding phase merges, using what it
teaches us, as was done for the Ming plan.

## Checkpoints

- After phase 1: run a real Ming and Wan 1.3B generation from data files on this Mac
  before deleting builders (task 1.6).
- After phase 2: show the compiler output for `text_to_video_wan` next to the shipped
  graph.
- After phase 6: a new workflow reaches the GUI with no workflow-specific Rust.
