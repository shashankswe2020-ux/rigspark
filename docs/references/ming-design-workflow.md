# Ming Design candidate workflow contract

Date: 2026-10-13. Status: implemented candidate, **not live-verified or released**.

## Sources

- [Official template, pinned revision](https://github.com/Comfy-Org/workflow_templates/blob/99e3d43745926b78466d99f937c0cd2bb622423a/templates/image_ming_image_01_design_t2i.json).
- [Native Ming architecture and model detection](https://github.com/Comfy-Org/ComfyUI/blob/8010b48d2c1be986613f8e8c761527d205bd756f/comfy/supported_models.py).
- [Native Ming node extension](https://github.com/Comfy-Org/ComfyUI/blob/8010b48d2c1be986613f8e8c761527d205bd756f/comfy_extras/nodes_ming.py).
- [Core nodes, including loaders and PNG saving](https://github.com/Comfy-Org/ComfyUI/blob/8010b48d2c1be986613f8e8c761527d205bd756f/nodes.py).
- [Per-node object-info API](https://github.com/Comfy-Org/ComfyUI/blob/8010b48d2c1be986613f8e8c761527d205bd756f/server.py).
- [Pinned publisher metadata](https://huggingface.co/api/models/Comfy-Org/Ming-Image/revision/53654871e47a5d2daed7b3a986cbf1010ef81c78?blobs=true).
- [Model card](https://huggingface.co/inclusionAI/Ming-Image-0.1-Design).

The ComfyUI revision is the inspected source baseline, not a certified minimum
version or proof that a particular installed build/device can execute INT8 weights.

## Exact candidate pairing

All three artifacts use publisher `Comfy-Org/Ming-Image` and revision
`53654871e47a5d2daed7b3a986cbf1010ef81c78`.

| Role | File basename | Bytes |
| --- | --- | ---: |
| Diffusion | `ming_image_0.1_design_int8_convrot.safetensors` | 6,175,239,953 |
| Text encoder | `ming_image_0.1_ling_mini_2.0_int8_convrot.safetensors` | 19,507,670,525 |
| VAE | `ming_image_vae_bf16.safetensors` | 253,816,696 |

Total: **25,936,727,174 bytes**, about 24.2 GiB of weights before runtime overhead.
Exact SHA-256 values are in the [candidate fixture](../../crates/rigspark-core/fixtures/ming-design-candidate.json);
tests compare them to the recorded publisher response, not to fabricated weights.
Runtime tests separately replace these with tiny integrity-pinned stand-ins.

The fixture is a standalone test catalog. Its `default: true` satisfies that catalog's
validation and does not change any bundled default. It is not an installable user
catalog or a supported GUI model selection.

## Authored minimal graph

The `ming-design` graph uses the direct prompt with the native Ling loader's
`qwen_image` choice, BasicGuider, Euler, a simple 12-step schedule, full denoise,
and ModelSamplingFlux with max/base shifts 1.15/0.5. Latent and sampling dimensions
are both fixed to 1024 x 1024 with batch size one.

The official template connects resolution inputs to both latent creation and sampling;
its disconnected widget values are not authoritative for an API graph. The candidate
uses the 1024 bucket also described by the model card rather than mixing dimensions.

Deliberate reductions:

- Direct prompt conditioning instead of the optional prompt-enhancement branch, so no
  Qwen language model, account, remote API, or extra encoder is required.
- Default core attention rather than selecting the template's optimized attention
  backend. Device/kernel compatibility and image quality still require live verification.
- Standard `SaveImage` instead of dynamic advanced output formats. The pinned core node
  constructs the PNG from all decoded channels without forcing RGB. Runtime copies the
  PNG bytes unchanged; a valid RGBA fixture tests byte preservation.
- No UI/subgraph/preview-only nodes or downloaded workflow JSON.

## Preflight and failure behavior

Only the Ming candidate takes the new preflight path. Before installing weights:

1. Validate the built-in graph against the existing local-node allowlist.
2. Inspect each distinct graph class and `TextEncodeMingImageEdit` through bounded
   `/object_info/<class>` requests. The latter is a native-Ming capability marker,
   not an editing node submitted in the text-to-image graph.
3. Require local core module declarations and reject API nodes; verify the marker
   comes from `comfy_extras.nodes_ming`.
4. Validate required/optional inputs, loader choices, numeric bounds, linked output
   types, and the PNG output-node flag.
5. Defer only installed weight filename membership until the existing post-install
   `/models/...` resolution step. Empty pre-install file inventories are expected.

Each node response is limited to 1 MiB and uses existing HTTP timeouts/cancellation.
Missing, malformed, oversized, or incompatible responses produce an explicit error,
with no downloads or prompt submission. Cancellation remains cancellation.

These checks are compatibility screening, not proof of weight decoding, kernel support,
memory sufficiency, actual image generation, or the trustworthiness of a local server.
No CUDA or Mac/MPS execution claim follows from an object-info response.

## Release gate

The reserved admission recipe has `verified: false`; production recipe lookup excludes
it. Bundled Ming entries remain unchanged and fit-only. Both native preparation and
direct runtime execution reject fit-only entries before contacting ComfyUI; GUI job
creation cannot bypass this with the memory-fit bypass flag.

The user explicitly chose **no weight downloads and no real smoke test** for this
increment. Do not flip the recipe flag, promote catalog entries, mark #317 available,
or claim CLI/GUI Ming generation works until the exact pairing passes the approved
real-runtime checkpoint in the [implementation plan](../plans/ming-design-workflow-support.md).

## First approved live attempt (2026-10-14)

Result: **PARTIAL, acquisition failure; no image generated**.

- Candidate code: `a08ff8c`, plus the live-discovered COMBO compatibility fix.
- Existing ComfyUI 0.38.0 at `f1072eb0350638a3390ddb6afbcaa8c6b237c6fd`;
  Python 3.12.13, Torch 2.14.1, Apple MPS available, 36 GiB unified RAM.
- Started an owned server on `127.0.0.1:49322`, with custom and partner nodes
  disabled and separate smoke user/output directories. No existing server was replaced.
- Live immutable metadata matched all three candidate file sizes and SHA-256 digests.
  Production hardware sizing returned `yes` (about 28.9 GiB weight budget); this
  does not prove inference peak-memory requirements.
- The first preflight rejected a valid modern node schema: native ComfyUI reports
  some choices as `["COMBO", {"options": [...], "multiselect": false}]`, not only
  legacy nested arrays. A failing regression reproduced this; validation now accepts
  both single-choice formats while rejecting missing/malformed choices, unsupported
  values, and multiselect inputs. The real preflight then passed.
- Production acquisition downloaded and verified the 6,175,239,953-byte diffusion
  artifact. Independent SHA-256 verification matched the catalog, with mode `0600`.
- The 19,507,670,525-byte encoder transfer reached at least 95% before failing with
  `error decoding response body; download transport failed`. No prompt was submitted.
  The installer removed its incomplete file and lock; VAE acquisition was not reached.
- The owned ComfyUI process was stopped and its listener released. The verified
  diffusion cache was retained for a later retry; no inference RAM remains allocated
  by this test. No production catalog data or ComfyUI checkout files changed.
- Regression tests, full workspace tests, build, strict Clippy, fmt and the native
  retirement gate passed for the schema fix. Real acquisition completion, cache-hit
  execution, actual PNG/alpha generation, and CUDA/MPS certification remain unproven.

The attempt used a temporary Rust executable calling production hardware sizing,
`ComfyUi::generate`, `NativeTransport`, and `HfTransport` with the candidate fixture.
The executable source was removed after cleanup; it did not enable a user-facing
catalog bypass. Retrying requires the encoder download again, since failed unverified
bytes were deliberately not promoted or retained.
