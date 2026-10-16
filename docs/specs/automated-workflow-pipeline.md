# Spec: Automated Generation Workflow Pipeline

> Status: **Draft for review**
> Last updated: 2026-10-16
> Related: [image-video-generation.md](./image-video-generation.md),
> [Ming Design workflow](../references/ming-design-workflow.md),
> [plan](../plans/automated-workflow-pipeline.md)

## 1. Objective

Adding a ComfyUI workflow currently requires hand-written Rust: a `Workflow` enum
variant, a `json!` graph builder, an output-node constant, a recipe, bespoke
preflight, device workarounds and tests. Ming Design took several PRs. That cannot
scale with a catalog of hundreds of image and video models.

Build a pipeline in which **adding a workflow requires no Rust code**:

1. CI discovers official ComfyUI templates, pins every weight, compiles the template
   into a runnable graph, validates it against a real (CPU-only) ComfyUI, and opens a
   PR containing a *candidate* workflow file plus fit-only catalog entries.
2. A maintainer runs one local command on real hardware. It generates media,
   validates the output, and writes an evidence file.
3. Promotion to runnable is a data change that CI only accepts when matching evidence
   exists.
4. One generic runtime executes every workflow file in the CLI, TUI and GUI.

### User stories

- As a maintainer, when Comfy-Org publishes `text_to_video_wan`, a PR appears with a
  pinned, validated candidate, without me writing graph code.
- As a maintainer with a GPU/Mac, I run `cargo workflow-verify <id>` and get a
  committed evidence file plus a promotion change.
- As a user, verified workflows appear as runnable; candidates show
  `workflow candidate (unverified)` and cannot run, even with memory bypass.

### Decisions (approved 2026-10-16)

| Decision | Choice |
|---|---|
| Verification | Local maintainer command on real hardware; self-hosted runner is out of scope |
| First scope | Prompt-only workflows (text-to-image, text-to-video). Image/video/audio inputs are later |
| Sources | Only `Comfy-Org/workflow_templates` at a pinned commit; `openSource: true`; no Partner API nodes |
| Existing workflows | Migrate all six (FLUX checkpoint/split, Wan 2.1, Wan 2.2 TI2V, Qwen-Image, Ming Design) to the data format |
| Dataset format | Approved: new workflow files and catalog fields |

## 2. Architecture

```
          weekly CI (no GPU)                          maintainer hardware
┌──────────┐ ┌──────┐ ┌─────────┐ ┌──────────┐ ┌──────────┐   ┌──────────┐ ┌─────────┐
│ discover │→│ pin  │→│ compile │→│ validate │→│ propose  │ → │  verify  │→│ promote │
│ index    │ │ HF   │ │ UI→API  │ │ CPU      │ │ PR       │   │ real run │ │ CI-gated│
└──────────┘ └──────┘ └─────────┘ │ ComfyUI  │ └──────────┘   └──────────┘ └─────────┘
                                  └──────────┘
                          runtime: one generic executor reads workflow files
```

### 2.1 Workflow file (new dataset)

`crates/rigspark-core/data/workflows/<id>.json`, bundled at build time:

```json
{
  "schemaVersion": 1,
  "id": "wan2.1-t2v-1.3b",
  "kind": "video",
  "status": "candidate",
  "provenance": {
    "source": "comfy-template",
    "repository": "Comfy-Org/workflow_templates",
    "commit": "99e3d43745926b78466d99f937c0cd2bb622423a",
    "template": "text_to_video_wan",
    "minComfyUIVersion": "0.3.60"
  },
  "graph": { "3": { "class_type": "KSampler", "inputs": { "seed": 0, "steps": 30 } } },
  "bindings": {
    "prompt": [{ "node": "6", "input": "text" }],
    "seed": [{ "node": "3", "input": "seed" }],
    "output": { "node": "28", "format": "mp4" }
  },
  "models": [
    { "node": "37", "input": "unet_name", "folder": "diffusion_models", "role": "diffusion" }
  ],
  "contract": { "KSampler": { "module": "nodes", "inputs": { "seed": "INT" } } },
  "overrides": [
    { "device": "mps", "node": "3", "input": "sampler_name", "value": "euler",
      "reason": "uni_pc diverges on Apple MPS" }
  ],
  "digest": "sha256 of canonical graph + bindings + models + overrides"
}
```

- `graph` is ComfyUI **API format** (what `/prompt` accepts), already compiled.
- `bindings` are the only values RigSpark substitutes at run time.
  Width, height, length and steps keep template values in this phase.
- `models` maps each loader input to a catalog file, so filenames are rewritten to
  RigSpark's pinned install paths.
- `contract` records the expected schema of every node class. It replaces
  Ming-specific preflight with generic preflight.
- `status`: `candidate` | `verified` | `withdrawn`.
- `digest` lets evidence bind to the exact graph that was verified.

### 2.2 Catalog changes (`generation.json`, schemaVersion 3)

- `workflow` becomes a string id validated against the bundled workflow registry,
  replacing the closed enum. Existing ids stay unchanged (`flux-checkpoint`,
  `flux-split`, `wan-t2v`, `wan22-ti2v`, `qwen-image`, `ming-design`).
- `files[]` may contain multiple files per role (Wan 2.2 high/low-noise) and a new
  `lora` role. Each file is still pinned by repo, revision, SHA-256 and size.
- An entry is runnable only if its workflow file is `verified` **and** has evidence
  covering the entry's exact files.
- The generation catalog stays bundled-only, so older binaries never see new ids.

### 2.3 Evidence file

`crates/rigspark-core/data/workflow-evidence/<id>/<device-class>.json`, written by
`cargo workflow-verify`:

- workflow digest, catalog entry id, every weight's SHA-256 and size
- ComfyUI version and commit, Python and Torch versions, device class
  (`apple-mps` | `nvidia-cuda` | `cpu`), OS, total RAM/VRAM
- prompt and seed used, output SHA-256, decoded format, dimensions, frame count and
  duration (video)
- wall-clock queue-to-output time (observational; **not** a throughput estimate)
- RigSpark version and commit

Only facts the run observed are recorded. Unknowns are omitted, never estimated.

## 3. Pipeline stages

### 3.1 Discover (CI)

- Resolve `Comfy-Org/workflow_templates` HEAD to a commit; fetch `index.json` at that
  commit, bounded in size.
- Keep templates with `openSource: true` and `mediaType` `image` or `video`.
- Reject with a recorded reason: `api_*`/Partner nodes, any `Load*` media input
  (later phase), custom (non-core) nodes, or non-Hugging Face model URLs.
- Templates already represented by a workflow file at the same template revision are
  skipped. Changed templates produce a new candidate revision; verified ones are kept.

### 3.2 Pin (CI)

- Every model URL must be `https://huggingface.co/<owner>/<repo>/resolve/<ref>/<path>`.
- Resolve `<ref>` to a commit via the HF API and read LFS SHA-256 and size.
  Gated, private, missing or mismatched files reject the template.
- Only files referenced by loader nodes in the compiled graph are pinned; unused
  alternatives in the template metadata are ignored.

### 3.3 Compile (CI, Rust)

Converts the template's UI format into API format using ComfyUI's live `object_info`:

- Flatten subgraphs; resolve links through reroutes, subgraph inputs and outputs.
- Drop frontend-only nodes (`Note`, `MarkdownNote`, `Reroute`, legacy
  `PrimitiveNode`, by folding their values into targets) and muted or bypassed nodes,
  handling bypass pass-through.
- Map `widgets_values` to input names using `object_info` order. Skip frontend-only
  widgets such as `control_after_generate` after seed inputs.
- Detect bindings deterministically:
  - prompt = the text input of the encoder feeding the sampler's positive conditioning
  - seed = sampler `seed` or `noise_seed`
  - output = exactly one `SaveImage`, `SaveAnimatedWEBP` or `SaveVideo`
- Ambiguity (zero or several candidates) rejects with a reason. Nothing is guessed.
- Emit `contract` from `object_info` for every class used.

### 3.4 Validate (CI, no GPU)

- Start pinned ComfyUI on CPU, loopback-only, custom and API nodes disabled.
- Create zero-byte placeholder files with the pinned names in a temporary
  `models/` directory.
- Submit the compiled graph to `/prompt`. ComfyUI's own validation must return no
  `node_errors`. Then immediately interrupt and clear the queue. The placeholders are
  never loaded.
- Run RigSpark's generic preflight against the same `object_info`.

### 3.5 Propose (CI)

- Write candidate workflow files and fit-only catalog entries with hardware-fit
  verdicts; generation speed stays `unknown`.
- Open at most one pending PR, as the enrichment workflows do. Each run is capped at
  10 new candidates.
- Record rejections with reasons in `docs/references/workflow-harvest-report.json`.

### 3.6 Verify (maintainer hardware)

`cargo workflow-verify <id> --entry <catalog-id> --comfyui-dir ~/ComfyUI`:

- Uses an already-running loopback ComfyUI, or starts an owned one with custom and
  API nodes disabled and separate user/output directories. Never replaces an
  unowned server.
- Runs generic preflight, downloads with fail-closed integrity, and generates with the
  workflow's fixed verification prompt and seed.
- Validates output: decodes PNG/WEBP/MP4 headers, checks dimensions and frame count
  against the graph, and rejects empty or truncated media.
- Writes the evidence file and prints the promotion command. Writes nothing on failure.

### 3.7 Promote (CI-gated data change)

`cargo workflow-promote <id>` sets `status: verified` only when evidence exists whose
digest and weight hashes match. A required CI test enforces the same rule for every
bundled workflow, so a hand edit cannot bypass it.

## 4. Generic runtime

- `rigspark-runtime` loads the workflow file, applies bindings, rewrites loader
  filenames and applies device overrides. It runs generic preflight, then the
  existing submit/poll/download path.
- Node policy:
  - allow core classes whose `python_module` is `nodes` or `comfy_extras.*`
  - deny `comfy_api_nodes`, custom nodes, and network-fetching or upload nodes
- Output formats: PNG, animated WEBP, and **MP4** (new; GUI uses `<video>`).
  Media is saved byte-for-byte with bounded size.
- Per-workflow Rust code is removed after migration. Remaining Rust is generic.

## 5. Commands

```bash
cargo workflow-harvest --dry-run                 # discover + pin + compile, no writes
cargo workflow-harvest --object-info <file>      # CI: compile against live CPU ComfyUI
cargo workflow-validate --endpoint http://127.0.0.1:8188   # CI validation stage
cargo workflow-verify <id> --entry <catalog-id> --comfyui-dir ~/ComfyUI
cargo workflow-promote <id>
cargo test --workspace --locked -- --test-threads=2
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo native-retirement
```

## 6. Project structure

```
crates/rigspark-core/src/workflow/        # file model, digest, bindings, contract, registry
crates/rigspark-core/data/workflows/      # bundled workflow files
crates/rigspark-core/data/workflow-evidence/
crates/rigspark-runtime/src/workflow/     # compiler (UI→API), harvest, pin, validate, verify
crates/rigspark-runtime/src/generation.rs # generic executor (per-workflow builders removed)
crates/rigspark-cli/src/workflow_*.rs     # maintenance binaries
.github/workflows/workflow-harvest.yml    # weekly + dispatch
crates/*/tests/workflow_*.rs              # recorded templates/object_info fixtures
```

## 7. Code style

Follow existing conventions: typed `serde` models with `deny_unknown_fields`,
`thiserror` errors, bounded network reads, no `unsafe`, and pure decision functions
extracted for testing.

```rust
pub enum BindingError {
    #[error("{template}: expected one positive prompt input, found {found}")]
    AmbiguousPrompt { template: String, found: usize },
}
```

## 8. Testing strategy

- **Golden migration:** for every existing workflow, the data-driven graph equals the
  current `json!` builder output for the same seed, prompt, device and filenames.
  This runs before any builder is deleted.
- **Compiler:** recorded templates (`text_to_video_wan`, `image_qwen_image`,
  `image_ming_image_01_design_t2i`, `video_wan2_2_14B_t2v`) plus recorded
  `object_info`. Expected API graphs are reviewed once, then frozen. Covers
  subgraphs, bypass, reroutes, primitives, `control_after_generate`, and ambiguity
  rejection.
- **Pinning:** recorded HF responses; gated, missing and mismatched cases fail closed.
- **Promotion gate:** verified without evidence, with stale digest, or with mismatched
  hashes fails.
- **Runtime:** mocked ComfyUI transport for bindings, overrides, node policy, MP4
  output and bounds. No real ComfyUI, downloads or network in tests.
- **CI validation:** CPU ComfyUI runs only in the harvest workflow, not in unit tests.
- **Real hardware:** the verify command is the real smoke test. Re-verify the six
  migrated workflows on Apple MPS before deleting their builders.

## 9. Boundaries

- **Always:** pin every weight by commit and SHA-256; fail closed; keep loopback-only;
  keep speed `unknown` without measurement; keep candidates non-runnable; record
  rejection reasons.
- **Ask first:** new output formats beyond PNG/WEBP/MP4; media-input workflows;
  non-Hugging Face weight hosts; community sources; self-hosted runners; changing the
  default model; new runtime dependencies (e.g. an MP4 parser crate).
- **Never:** run Partner/API or custom nodes; guess a binding; mark verified without
  matching evidence; download weights or start ComfyUI in tests; reintroduce
  Node/TypeScript; expose servers beyond 127.0.0.1.

## 10. Success criteria

1. All six existing workflows run from data files with byte-identical graphs (golden)
   and pass re-verification on Apple MPS; per-workflow Rust builders are deleted.
2. `cargo workflow-harvest --dry-run` against the pinned templates commit compiles
   `text_to_video_wan` and `image_qwen_image` into graphs that ComfyUI's own `/prompt`
   validation accepts. It reports a reason for each rejected template; for example,
   the official Ming template needs an Nvidia/AMD attention backend. Differences from
   hand-authored graphs are expected and recorded, not hidden.
3. The CI harvest workflow opens a candidate PR, with fit-only entries, that passes
   CPU ComfyUI validation.
4. `cargo workflow-verify` on this Mac promotes one **new** workflow end to end with
   no workflow-specific Rust (target: Wan 2.1 T2V-14B or a Wan 2.2 14B T2V variant if
   it fits; otherwise the smallest new prompt-only template that fits).
5. CI rejects `verified` without matching evidence.
6. fmt, Clippy (`-D warnings`), full tests, build and `cargo native-retirement` pass.

## 11. Open questions

1. **CI Python:** CPU ComfyUI validation installs Python, Torch CPU and pinned
   ComfyUI in the harvest workflow only. Node/TypeScript stay banned. Approve?
2. **MP4 output:** validating MP4 needs a header/box parser. Options are a small
   in-house bounded parser or a vetted crate (dependency, so ask first).
3. **Verification prompt:** use one fixed prompt per kind, or the template's own
   default prompt? Proposal: the template default, recorded in evidence.
4. **Template updates:** when a verified template changes upstream, should the old
   verified workflow keep running until the new candidate is verified? Proposal: yes.
