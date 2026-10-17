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
| 4 | CI validation and proposals | Weekly workflow; workerless CPU schema validation; candidate PRs | 3 |
| 5 | Verify, promote, MP4 | Evidence files, CI gate, MP4 output in CLI and GUI | 1, 3 |
| 6 | Real proof | Re-verify the six on MPS, remove legacy allowance, promote one new workflow | 4, 5 |

Phases 4 and 5 can proceed in parallel once phase 3 lands.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| UI→API conversion is subtle (widget order, `control_after_generate`, subgraphs, bypass) | Wrong graphs | Frozen golden fixtures from real templates; ComfyUI's own workerless `validate_prompt` in CI; ambiguous cases fail closed |
| Migration changes behavior of shipped workflows | Regressions | Byte-identical golden tests against the existing builders before deletion |
| Six shipped workflows lack evidence files | Promotion gate would block them | Temporary `legacy-builtin` allowance for those six ids at frozen digests, removed in phase 6 after MPS re-verification |
| Template churn upstream | Noisy PRs | Pinned commit, skip unchanged templates, cap 10 candidates, one pending PR |
| Large models exceed local hardware | Cannot verify | Evidence is per device class; unverified stays fit-only; never claim CUDA from MPS |
| CPU ComfyUI install in CI is slow or flaky | CI noise | Harvest workflow only; cached venv; pinned ComfyUI commit and Torch CPU wheel |

## Phase 1 status (2026-10-16)

Done: 1.1, 1.2, 1.3, 1.5 and 1.6. The six workflows are bundled data files under
`crates/rigspark-core/data/workflows.json`. The runtime builds every graph through
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

## Phase 2 status (2026-10-17)

Done. `rigspark_runtime::workflow_compile::compile(template, ui_json, object_info)`
converts official templates into API-format graphs with prompt, seed, output and model
bindings. It handles:

- subgraph flattening, with ids `parent:child` as the ComfyUI frontend exports
- both instance value styles: `widgets_values(_named)` on the instance, and
  `proxyWidgets` where inner nodes keep their own values
- reroute, bypass pass-through, mute and notes
- positional widgets mapped through `input_order`, with `control_after_generate`
  only when the schema declares it
- V3 dynamic combos (`format`, `format.codec`, `sampling_mode.*`) and autogrow sockets
  (`values.a`)
- switch folding through primitive constants, and pruning to the single media save node
- link type checks, combo and range checks, and model files required to appear in
  the template manifest

Order-sensitive schema parsing uses an order-preserving map, so no workspace-wide
`serde_json` feature change was needed.

Fixtures: four templates at `99e3d437` and `object_info` for the 37 classes used,
captured from ComfyUI 0.38.0 (`f1072eb`) on Apple Silicon in CPU mode. 14 tests cover
every compiled template and the fail-closed cases.

**Earlier smoke:** ComfyUI 0.38.0's own `/prompt` validator accepted all five compiled
graphs (Wan 2.1, Qwen-Image, Wan 2.2 14B, Ming with and without prompt enhancement)
with zero `node_errors`, using zero-byte placeholder weights in a temporary
`--base-directory`. The queue started executing before it was cleared, and the
placeholders failed harmlessly at load. This method was rejected for CI; phase 4
validates directly without starting a server or worker.

Findings that change expectations:

1. **Templates are not the shipped graphs.** Template defaults differ: Wan 2.1 uses
   33 frames and MP4, Qwen-Image uses CFG 4, and Ming enables an LLM prompt enhancer
   that needs a 27B Qwen 3.8 encoder. The compiler is faithful; reducing a template is
   a maintainer decision recorded in the workflow file, never inferred.
2. **Some templates are hardware-specific.** The official Ming template selects
   `comfy kitchen attention` (Nvidia/AMD only). On this Mac, ComfyUI offers only
   `pytorch attention`, so the unmodified template is correctly refused. Phase 3 must
   compile against the target device class's `object_info` and record which device
   classes a candidate is valid for.
3. **Multi-file models are real:** the Wan 2.2 14B template needs two diffusion files.
   This confirms the phase 3 catalog change.

## Phase 3 status (2026-10-17)

Done, in four commits:

1. **String workflow ids.** `WorkflowId` replaces the closed enum. The bundled registry
   (`crates/rigspark-core/data/workflows.json`, kept in canonical form by a test)
   decides which workflows exist, their kind and slots, and whether they are verified.
   Catalog entries run only through a verified registry workflow. Adds the `lora` role.
2. **Candidate workflow files.** These add template provenance (pinned commit),
   validated device classes, MP4 via `SaveVideo`, PNG via `SaveImageAdvanced`, 1-8 seed
   bindings, and slots that pin exact weights (allowing repeated diffusion slots and
   LoRAs). Repeated-role and MP4 workflows fail closed at run time until phase 5.
3. **Harvester** (`rigspark_runtime::workflow_harvest`). It resolves `main` through
   GitHub's `vnd.github.sha` media type, then fetches the index and templates as raw
   files at that pinned commit. Only these exact URLs are allow-listed. It compiles per
   device class, pins every manifest weight to an exact Hugging Face commit, SHA-256
   and size, and links candidates to catalog entries by primary-weight digest. It
   rejects Partner, media-input, unpinnable and incompatible templates with reasons,
   skips unchanged templates, caps candidates per run, and marks the run incomplete on
   fetch failures.
4. **`cargo workflow-harvest`.** Repeatable `--object-info <cpu|mps|cuda>=<file>`,
   `--commit`, `--limit`, `--dry-run`, and write mode, which appends candidates
   canonically and is idempotent.

**Live dry run** (2026-10-17, templates `04a508b`, ComfyUI 0.38.0 CPU `object_info`
from an owned server):

- 41 candidates; 17 link to existing catalog entries. Linked examples include Wan 2.1
  1.3B, Wan 2.2 5B and 14B (both fit-only 14B entries), Qwen-Image and 2512, Z-Image
  and its INT8 variant, Chroma, LongCat and Ovis. New families include HiDream I1,
  Hunyuan Video 1.5, FLUX.2, SDXL and SD3.5.
- 649 rejections:

  | Count | Reason |
  |---:|---|
  | 389 | Partner API |
  | 206 | needs a media input |
  | 22 | no single media save node (audio, 3D, multi-output) |
  | 16 | prompt not identifiable (for example FLUX schnell's split `clip_l`/`t5xxl` encoder) |
  | 15 | version skew or a missing node |
  | 1 | gated repository |

- 1 failure: `Comfy-Org/flux2-klein` metadata request refused (a redirect outside the
  policy). The run is honestly marked incomplete.

The live run found and fixed two compiler bugs, each with a regression fixture:

- FLUX dev's `ConditioningZeroOut(positive)` negative was mistaken for a shared prompt.
- `PreviewAny` display values were treated as widget values.

Media-input templates are now detected before compiling, so their reason is accurate.

A first attempt also exposed an environment hazard: an unowned ComfyUI already held
port 8188, so a capture that assumed ownership read the wrong server. Phase 4 avoids
server ports entirely by capturing `object_info` in-process from pinned ComfyUI.

## Phase 4 status (2026-10-17)

Done: the scheduled/manual workflow checks for a pending proposal before doing work,
fetches only the triggering `main` revision, uses ComfyUI commit
`f1072eb0350638a3390ddb6afbcaa8c6b237c6fd`, and pins Python 3.12 and CPU Torch.
`scripts/workflow_validate.py` captures CPU schemas and validates up to ten harvested
candidates with ComfyUI's direct `execution.validate_prompt`. It never starts a
server/worker, calls `/prompt`, loads weights, or executes a graph; model placeholders
are empty files in a fresh temporary base. It fails closed on incomplete harvests,
non-CPU candidates, unpinned or unsafe model paths, native-node violations, partial
output errors, and modifications/removals of existing verified workflows. The
machine-readable report says `validation-only-no-worker` and `executedGraphs: 0`.

Only the workflow registry plus harvest and validation reports can reach the proposal
PR; model catalog entries are not added. Candidates stay unverified, no PR is opened
for zero candidates, and the automation does not merge. The commit/PR text carries the
requested Claude watermark and co-author trailer.

Verification: seven standard-library validator tests, the CLI/core/runtime workflow
test slices, `cargo native-retirement`, `cargo fmt --check`, and `actionlint` passed.
A real local CPU smoke harvested one template at source commit `04a508b`, then
validated `image-capybara-v0-1-text-to-image-04a508b`: valid, no node errors, output
`102`, with `executedGraphs: 0`. This proves schema compatibility only, not that its
36 GB weights fit or can generate on CPU.

### Phase 2 tasks: template compiler (outline)

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
