# Implementation Plan: Ming Design Workflow Support

Status: tested candidate implemented; production activation is gated.
Date: 2026-10-13
Tracking: [model request #317](https://github.com/shashankswe2020-ux/rigspark/issues/317)

## Current execution status

The user approved the serialized `ming-design` candidate value and then explicitly
chose to finish the tested candidate **without downloading weights**.

- T1: pinned source contract recorded in [the reference](../references/ming-design-workflow.md).
  Source inspection is complete; a minimum tested runtime/device remains unverified.
- T2-T3: typed workflow, authored direct-prompt graph, local-node enforcement, and
  bounded pre-download capability validation implemented with regression tests.
- T4: exact INT8 candidate fixture and reserved recipe added. The recipe remains
  `verified: false`, excluded from production admission; bundled data is unchanged.
- T5: mocked runtime execution and RGBA byte preservation tested. CLI/GUI regression
  tests enforce the fit-only release gate. An exposed runnable Ming browser/CLI path
  is deferred rather than adding a user-facing bypass solely for testing.
- T6-T8: blocked by the approved no-download decision. No real generation, catalog
  promotion, exact availability claim, or issue closure has been performed.
- T9: candidate quality checks/review apply to this increment; verified-feature
  release and production rollback exercises remain deferred.

Candidate verification completed: affected core/runtime generation and admission
suites, CLI/GUI release-gate regressions, request-routing regressions, full workspace
tests, build, strict Clippy, formatting, and `cargo native-retirement` all passed.
No live weights were downloaded, no ComfyUI process was started, and no production
catalog entry was changed. Mocked execution is not a real model smoke test.

## Goal and first release

Make one explicitly verified Ming Image 0.1 Design variant usable from the existing
CLI and GUI through a built-in local ComfyUI text-to-image workflow. Preserve PNG
alpha when the model produces transparent output.

Workflow code is shared by architecture, task, and compatible loader arrangement.
Compatibility is certified per exact model/encoder/VAE combination. Neither a family
name nor a shared publisher repository makes every variant runnable.

The first candidate is the official template's INT8 Design combination:

- `ming_image_0.1_design_int8_convrot.safetensors`
- `ming_image_0.1_ling_mini_2.0_int8_convrot.safetensors`
- `ming_image_vae_bf16.safetensors`

This is a candidate, not a declaration of support. Do not silently substitute BF16,
W4A8, or Design-Layer weights. BF16 and other precisions require separate verification.
Design-Layer, image editing, automatic prompt rewriting, custom nodes, downloaded
workflow execution, a new backend, and ComfyUI installation are out of scope.

## Inspected evidence and gaps

1. [Core workflow definitions](../../crates/rigspark-core/src/generation.rs) already
   support reusable typed workflows and exact file roles. Fit-only entries have no
   workflow. Keep that distinction.
2. [Generation admission](../../crates/rigspark-core/src/generation_admission.rs)
   already has explicit runnable recipes. Its fit-only fallback selects the smallest
   encoder/VAE; this is not proof that those companions execute the requested model.
   Existing Ming entries use W4A8, unlike the official INT8 template.
3. [Runtime generation](../../crates/rigspark-runtime/src/generation.rs) uses authored
   Rust graphs and a local-node allowlist. It currently installs weights before
   graph submission; Ming compatibility checks must happen before large downloads.
4. [Pinned official template](https://github.com/Comfy-Org/workflow_templates/blob/99e3d43745926b78466d99f937c0cd2bb622423a/templates/image_ming_image_01_design_t2i.json)
   includes the INT8 pairing, `CLIPLoader` with `qwen_image`, `ModelSamplingFlux`,
   `BasicScheduler`, `SamplerCustomAdvanced`, and VAE decoding. It also includes
   optional Qwen prompt enhancement, UI/subgraph machinery, and an advanced save
   node. Do not copy the whole template into runtime or allowlist every node it uses.
5. The pinned template uses Euler, a simple schedule, and 12 steps. It contains
   1536 dimensions, whereas the upstream native-inference model card recommends
   1024/2048 buckets. Resolve the ComfyUI-specific graph contract rather than mixing
   settings from different inference implementations.
6. The [publisher package](https://huggingface.co/Comfy-Org/Ming-Image)
   contains both Design and Design-Layer. Repository-level lineage cannot identify
   an individual variant. [Pinned package metadata](https://huggingface.co/api/models/Comfy-Org/Ming-Image/revision/53654871e47a5d2daed7b3a986cbf1010ef81c78?blobs=true)
   is available for integrity/lineage verification.
7. The [upstream model card](https://huggingface.co/inclusionAI/Ming-Image-0.1-Design)
   describes RGBA output and an 80-GiB CUDA validated configuration. This is neither
   a universal minimum nor evidence for Mac/MPS support. The planning machine has
   36 GiB unified memory; weight fit alone does not prove generation will fit.

Related specs:
[image/video generation](../specs/image-video-generation.md) and
[catalog requests](../specs/model-catalog-requests.md).

## Architecture decisions and gates

- Reuse the attach-only ComfyUI runtime, authenticated GUI API, weight installer,
  cancellation, output handling, and existing offline sizing.
- Author a minimal graph in Rust from verified node contracts. Use only reviewed
  local core nodes, not arbitrary JSON, remote APIs, or automatic custom-node installs.
- Start without a prompt-rewriting model. Prove direct user-prompt conditioning works;
  do not accidentally add the template's Qwen model to memory or download requirements.
- Prefer one new typed workflow identifier and an explicit recipe over a generic
  workflow/plugin registry. Confirm approval of the new serialized workflow value
  before implementation; do not silently change the dataset format or schema version.
- Use bounded, typed runtime capability queries. Node presence alone is insufficient:
  verify needed loader choices and inputs. Hardware execution remains a separate gate.
- Keep unknown performance and unverified device support unknown. No CUDA-to-MPS
  support claim based only on graph construction or mocked tests.
- Do not publish the runnable recipe/catalog promotion before the real smoke checkpoint.
  Candidate entries may exist in test fixtures and a review branch.
- Large downloads, launching/upgrading ComfyUI, or using another machine require
  separate approval. Planning and automated tests do not perform them.

## Tasks and dependency order

### T1. Establish the exact compatibility contract

Dependencies: none. Scope: small, documentation and source evidence.

- [ ] Pin the ComfyUI implementation/release and record required node input/output
  contracts, loader support for INT8 ConvRot, sampler settings, and resolution behavior.
- [ ] Prove that prompt enhancement is optional and determine how RGB/RGBA pixels
  reach the saved PNG. If core-only execution is unavailable, stop for a scope decision.
- [ ] Update the spec with the proposed identifier, device limitations, exact pairing,
  and acceptance evidence; obtain approval for the serialized workflow addition.

Likely files: image/video spec; this plan; a focused source reference if needed.
Verification: compare pinned template, core node implementations, and artifact metadata.
No weights or runtime installation required.

### T2. Implement the typed workflow and minimal graph

Dependencies: T1. Scope: medium, four source/test files.

- [x] Add the workflow's kind/file-role contract and a graph builder using exact installed
  filenames, the user prompt, bounded seed, and the verified generation parameters.
- [x] Select the correct output node; allowlist only newly required reviewed core nodes.
  Keep existing Flux, Wan, and Qwen graphs unchanged.
- [x] Add failing tests first for graph links, loader options, prompt/seed wiring, output
  selection, and rejection of unapproved nodes.

Files: core/runtime `src/generation.rs` and their `tests/generation.rs`.
Verification: affected core and runtime generation tests; existing graph regressions.
Do not enable a production catalog entry yet.

### T3. Fail early on incompatible ComfyUI installations

Dependencies: T2. Scope: small, runtime generation source/tests.

- [x] Query the required core-node schemas and loader capabilities with bounded
  responses, existing cancellation/timeouts, and typed validation.
- [x] Reject missing/incompatible capabilities with an actionable local error before
  any weight download or `/prompt` submission.
- [ ] Cover older servers, malformed/oversized responses, cancellation, unsupported
  devices where established by T1, and a compatible mocked server.

Verification: injected HTTP/download tests assert zero downloads and zero submissions
on preflight failure. Existing workflows retain their current behavior.

### Checkpoint A: Safe execution plumbing

- [x] T2 and T3 pass targeted tests and build/Clippy checks.
- [x] No Ming production entry has become runnable; no new runtime dependency.

### T4. Prepare an exact admission recipe and candidate fixture

Dependencies: T1, T2. Scope: medium, core/runtime admission tests and core recipe.

- [ ] Match only the named Design diffusion file and the verified INT8 encoder/VAE,
  with source-backed immutable revisions, SHA-256 digests, and exact byte counts.
- [ ] Missing companions or mismatched evidence never fall back to the smallest
  encoder while retaining a runnable workflow. Layer/W4A8/BF16 remain unpromoted.
- [ ] Validate candidate catalog shape and demonstrate byte-stable second-run admission;
  unrelated entries and existing default selections remain unchanged.

Files: core admission source/tests, existing recorded metadata fixture, runtime
admission tests. Keep any production recipe change unmerged until Checkpoint B.
Verification: core and runtime `generation_admission` suites, using recorded transport.

### T5. Verify the CLI and GUI path with injected generation

Dependencies: T2, T3, T4. Scope: medium, at most five existing test/fixture surfaces.

- [ ] The exact selected Ming candidate flows from CLI and GUI to the same runtime;
  preflight, integrity, cancellation, and execution errors remain visible.
- [ ] Mocked output tests preserve PNG bytes and alpha, validate the selected output,
  and keep safe file/download behavior.
- [ ] Browser fixture verifies Models selection, Image Chat generation, preview,
  Download, keyboard access, and no regressions to Text/Video.

Likely files: runtime generation tests, CLI `generation_cli` tests, GUI `generation_api`
tests, existing browser fixture and native journey harness.
Verification: focused suites plus the existing real-browser fixture journey.
No live endpoints or real weights in automated tests.

### T6. Perform the separately approved real generation smoke

Dependencies: T5. Scope: small, smoke procedure and evidence.

- [ ] Before running, agree on machine/device, ComfyUI version, disk/memory budget,
  exact download bytes, output location, and which newly downloaded files to retain.
- [ ] Verify pinned weights, submit a plain text-to-image prompt and a transparent-output
  prompt, and inspect successful PNGs including the alpha channel and GUI download.
- [ ] Record device/version, actual outcomes, unsupported combinations, and cleanup.
  Never label mock output or a queued job as successful model execution.

Verification: actual completion and decodable output through CLI and GUI. Test repeat
execution and safe cancellation without disrupting unrelated ComfyUI work.
If no suitable approved hardware is available, stop here with a draft implementation;
do not mark the candidate verified or close #317.

### Checkpoint B: Release eligibility

- [ ] Real smoke evidence exists for the exact candidate pairing and tested device.
- [ ] Core-only execution and output preservation are confirmed.
- [ ] Mac support is claimed only if independently exercised successfully on Mac.

### T7. Promote the verified catalog entry and preserve refresh behavior

Dependencies: T4, T6. Scope: small, catalog data and focused regression tests.

- [ ] Promote only the tested Design variant with its correct companions and workflow
  source; keep its stable ID and avoid changing default model selection.
- [ ] Catalog refresh preserves the exact verified recipe and does not promote Layer
  or unrelated precision variants.
- [ ] Recalculate fit using the actual companion sizes; do not inherit the smaller
  W4A8 estimate or invent generation throughput.

Verification: catalog validation, core/runtime admission tests, and deterministic
refresh/idempotence fixtures. No full unrelated catalog refresh as part of this change.

### T8. Reconcile model-request status with exact supported coverage

Dependencies: T7. Scope: medium, bounded request-routing source/test changes.

- [ ] Associate the requested canonical Design repository with the explicitly verified
  recipe and exact primary/companion artifacts, not broad package ancestry.
- [ ] Tests distinguish exact runnable Design coverage from fit-only Layer entries,
  wrong companions, mismatched digests, and unrelated base-model requests.
- [ ] The CLI report identifies the exact runnable Design variant and cites its source;
  retain safe blocked outcomes for ambiguous or unverified coverage.

Files: runtime request generation matcher/tests; core recipe metadata if necessary;
CLI request tests. Reuse explicit recipe facts rather than parallel name heuristics.
Verification: recorded runtime and CLI request regressions. Hosted verification follows
deployment in T9. Do not introduce automatic issue closure.

### T9. Review, document, and ship the verified slice

Dependencies: T7, T8. Scope: small, directly related docs and release verification.

- [ ] Document the exact supported variant, tested device matrix, ComfyUI requirements,
  deferred Layer/editing support, and troubleshooting.
- [ ] Complete correctness/readability/architecture/security/performance review and all
  native quality gates; open a review PR without automatic merge/publication. After
  maintainer merge, retry #317, verify the hosted status/cited ID, and close the issue
  manually only when T6 evidence and exact coverage reporting are both established.
- [ ] Keep rollback simple: remove the catalog workflow assignment and disable its
  admission recipe first, retaining the entry as fit-only and preserving user files.

Verification:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --test-threads=2
cargo build --workspace --locked
cargo native-retirement
```

Run the smallest affected suites after each increment and full workspace tests before
code commits. Keep behavior changes separate from unrelated formatting/refactoring.

## Open decisions before implementation or live execution

1. Approve the new serialized workflow identifier after T1 establishes the contract.
2. Confirm the minimum tested ComfyUI revision and whether all needed functionality is
   available in core. Custom-node requirements would need a new scope decision.
3. Choose the live-smoke machine and approve its downloads separately. The current
   Mac's capacity and available disk do not prove INT8/MPS compatibility.
4. Decide whether a CUDA-only first release is acceptable if Mac support cannot be
   verified. Do not silently substitute a different pairing to make the smoke pass.

## Definition of done

A user selects the verified Ming Design entry, submits a prompt, and receives a valid
downloadable PNG through the existing local runtime. Catalog integrity, runtime
compatibility, actual generation, transparent-output handling, and request status are
all verified. Unsupported variants remain honestly fit-only. A passing request Action
alone is not completion of workflow support.
