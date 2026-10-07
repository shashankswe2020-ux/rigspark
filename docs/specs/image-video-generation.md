# Spec: Local Image and Video Generation (ComfyUI)

> Status: **Approved for implementation (v1.2: multimodal model selection and catalog enrichment)**
> Last updated: 2026-10-05
> Plan: [task-37-image-video-generation.md](../plans/task-37-image-video-generation.md)
> Related: [pluggable-inference-backends.md](./pluggable-inference-backends.md),
> [hardware-advisor.md](./hardware-advisor.md)

## 1. Objective

Extend rigspark beyond text LLMs so a user can discover, verify, install, and run
**open-weight image and video generation models fully locally and for free**.

- `rigspark catalog --generation` lists curated image/video models with an honest
  memory-fit verdict for this machine.
- `rigspark generate image --prompt "…"` / `rigspark generate video --prompt "…"`
  installs pinned, SHA-256-verified weights into an existing ComfyUI installation,
  submits a built-in local workflow to the loopback ComfyUI server, and saves the
  result to disk.

### Decisions (confirmed with the user)

| Question | Decision |
| --- | --- |
| Runtime | **ComfyUI** (GPL-3.0, free, local) for both image and video |
| Install model | **Attach-only**: user installs and starts ComfyUI; rigspark manages workflows + weights |
| CLI surface | New `generate image\|video` command |
| Initial catalog | One image model + one video model, both Apache-2.0, with verified workflows |
| Model views | Models has Text, Image, and Video views with one active model per kind |
| GUI creation | Chat uses the active model for its selected Text, Image, or Video mode; the separate Create view is removed |
| Enrichment | Dedicated image and video pipelines update the shared generation catalog |

### Free and local guarantees (non-negotiable)

ComfyUI Partner Nodes call paid, closed cloud APIs and require a Comfy account with
prepaid credits ([docs](https://docs.comfy.org/tutorials/partner-nodes/overview)).
rigspark must never use them:

1. rigspark only submits **built-in workflows** generated in Rust; it never accepts
   user-supplied or downloaded workflow JSON.
2. Every workflow is checked against a **fail-closed allowlist** of core local node
   classes before submission; any other `class_type` is rejected.
3. rigspark never reads, stores, or forwards a Comfy account/API key; requests carry
   no credentials.
4. ComfyUI endpoints must be **loopback** (`127.0.0.1`, `::1`, `localhost`).
5. Only licenses already on the catalog allowlist are accepted (v1 entries are
   Apache-2.0). ComfyUI is driven over HTTP as a separate process; no ComfyUI code
   or example files are vendored (GPL-3.0 isolation; workflows are authored here).

### Non-goals (v1.2)

- Installing, spawning, or upgrading ComfyUI or its Python environment.
- Image-to-video, inpainting, LoRAs, upscalers, or custom nodes.
- Throughput estimates for generation (no sourced dataset → `unknown`).
- Tauri-specific surfaces, `recommend`/`can-run` integration, or signed remote updates
  of the generation catalog (the enrichment pipelines update the reviewed bundled dataset).
- Adding metadata-only Hugging Face entries that cannot run through a built-in workflow.
- Automatically adding a new workflow family or accepting downloaded workflow JSON.

### v1.2 success criteria

1. Models exposes keyboard-accessible Text, Image, and Video tabs. Text preserves the
   existing runtime controls; Image and Video list only their matching generation entries.
2. Starting a text model makes it the active Text model. Selecting an image or video
   card makes it the active model for that kind without starting ComfyUI.
3. The active-model summary always names the current Text, Image, and Video selections;
   an unavailable text runtime is shown honestly as not running.
4. Chat Text, Image, and Video modes use their matching active selection. Image/video
   generation automatically uses the detected ComfyUI installation and default loopback
   port; runtime configuration is not exposed in the minimalist composer.
5. Create is absent from navigation and markup. Existing generation API and CLI
   behavior remain supported.
6. Two independently runnable GitHub Actions workflows enrich image and video entries
   in the shared generation catalog. They open review PRs and never push catalog
   changes directly to the protected branch.
7. Enrichment accepts typed Hugging Face metadata only for an allowlisted built-in
   workflow family, requires an allowlisted open-weight license, pins an immutable
   revision, records exact byte sizes and SHA-256 digests, validates the complete
   catalog, and reports every rejected/unknown candidate.
8. Advice stays deterministic and offline. Network access exists only in the explicit
   maintenance pipeline, never in Models, Chat, `recommend`, or `can-run`.
9. A successful image/video reply exposes a direct Download action. If ComfyUI is not
   detected or the selected model does not fit, Chat fails visibly and directs the user
   to the CLI/environment configuration rather than silently bypassing safety checks.

## 2. Curated models (v1)

| id | kind | weights (pinned) | license | workflow source |
| --- | --- | --- | --- | --- |
| `flux1-schnell:fp8` | image | `Comfy-Org/flux1-schnell@c2b683e…` `flux1-schnell-fp8.safetensors` → `checkpoints` (17.2 GB) | apache-2.0 | [ComfyUI Flux examples](https://comfyanonymous.github.io/ComfyUI_examples/flux/) |
| `flux1-schnell:fp16` | image | `flux1-schnell.safetensors` → `diffusion_models` (23.8 GB), T5 fp8 + CLIP-L → `text_encoders` (5.4 GB), AE → `vae` (0.34 GB) | apache-2.0 | [ComfyUI Flux examples](https://comfyanonymous.github.io/ComfyUI_examples/flux/) |
| `wan2.1-t2v:1.3b` | video | `Comfy-Org/Wan_2.1_ComfyUI_repackaged@123acf1…` diffusion model → `diffusion_models` (2.8 GB), umt5 fp8 → `text_encoders` (6.7 GB), VAE → `vae` (0.25 GB) | apache-2.0 | [ComfyUI Wan examples](https://comfyanonymous.github.io/ComfyUI_examples/wan/) |
| `wan2.1-t2v:1.3b-bf16` | video | BF16 diffusion model → `diffusion_models` (2.8 GB), shared umt5 fp8 + VAE | apache-2.0 | [ComfyUI Wan examples](https://comfyanonymous.github.io/ComfyUI_examples/wan/) |

Revisions, sizes, and SHA-256 digests come from the Hugging Face API at the pinned
commit. Workflow node graphs and parameters (FLUX: 1024², 4 steps, cfg 1.0, euler/simple;
Wan: 832×480, 49 frames, shift 8, 30 steps, cfg 6, uni_pc/simple, 16 fps animated WebP)
are taken from the API-format graphs embedded in the official example outputs.

## 3. Data model

New bundled dataset `crates/rigspark-core/data/generation.json`
(`rigspark_core::GENERATION_JSON`), separate from the LLM `models.json` so LLM
ranking, signing, refresh, and parity goldens are untouched.

```json
{
  "schemaVersion": 1,
  "generatedAt": "2026-10-04T00:00:00Z",
  "models": [{
    "id": "flux1-schnell:fp8", "family": "flux1-schnell", "kind": "image",
    "params": "12B", "license": "apache-2.0", "openWeight": true,
    "releaseDate": "2024-08-01", "default": true,
    "source": "https://huggingface.co/black-forest-labs/FLUX.1-schnell",
    "workflow": "flux-checkpoint",
    "workflowSource": "https://comfyanonymous.github.io/ComfyUI_examples/flux/",
    "files": [{ "role": "checkpoint", "folder": "checkpoints",
      "repo": "Comfy-Org/flux1-schnell", "revision": "<40 hex>",
      "file": "flux1-schnell-fp8.safetensors", "sha256": "<64 hex>", "bytes": 17236328572 }]
  }]
}
```

Validation (typed serde, `deny_unknown_fields`, no nulls): unique ids; `kind ∈ {image, video}`;
licence on the existing allowlist and `openWeight: true`; `workflow ∈ {flux-checkpoint,
flux-split, wan-t2v}` with **exactly** the roles that workflow needs; `folder` fixed by role;
pinned 40-hex revision, 64-hex digest, safe relative file path, positive byte size;
`source`/`workflowSource` are HTTPS; exactly one `default: true` per kind present.

## 4. Advice: memory-fit verdict (offline, deterministic)

Using `sizing::memory_capacity` and the existing `HEADROOM`:

- `budget = usable × (1 − HEADROOM)` (VRAM for discrete GPUs, unified/free RAM otherwise).
- **yes** — all weight files fit in `budget` at once.
- **slow** — the largest file fits but not all together (ComfyUI swaps stages), or on a
  discrete GPU the largest file fits in the system-RAM budget (ComfyUI low-VRAM offload).
- **no** — the largest file fits nowhere.
- **speed** — always `unknown` (honesty gate: no sourced generation throughput data).

The verdict describes memory only and says so in output. `generate` refuses `no`
unless `--bypass` (integrity checks are never bypassed).

## 5. Runtime: ComfyUI client (`rigspark-runtime::generation`)

All network/filesystem boundaries are injected (`http::Transport`,
`acquire::DownloadTransport`, poll interval); tests never touch a real server.

1. **Readiness** — `GET /system_stats` must return a `system` object.
2. **Install** — per file, `Acquisition` rooted at
   `<comfyui>/models/<folder>/rigspark` downloads from the pinned HF revision, verifies
   SHA-256 + exact size, and re-hashes cached files on every run (fail-closed).
3. **Visibility** — `GET /models/<folder>` must list the installed file (separator-normalised);
   the listed spelling is used in the workflow so Windows paths work.
4. **Workflow** — built in Rust from the verified graph, then `ensure_local_only`.
5. **Submit** — `POST /prompt {"prompt": …}` → validated `prompt_id`; `node_errors` → typed error.
   Device-aware tuning: when `/system_stats` reports an `mps` device, Wan uses `euler` instead of
   `uni_pc` (the latter diverged on Apple MPS in live testing); a progress line announces it.
6. **Poll** — `GET /history/{id}` until `status.completed`; `status_str = error` → typed error;
   overall limit 2 h; Ctrl-C → best-effort `POST /queue {"delete":[id]}` + `POST /interrupt`.
7. **Fetch** — output descriptor from the workflow's save node (safe filename, `.png`/`.webp`,
   `type: output`), `GET /view?filename=&subfolder=&type=output`, stream ≤ 1 GiB into a
   temp file and persist **without clobbering** an existing path.

## 6. CLI

```
rigspark catalog --generation [--hardware FILE]
rigspark generate <image|video|MODEL_ID> --prompt TEXT [--output PATH] [--seed N]
                  [--comfyui-dir DIR] [--port 8188] [--bypass] [--json]
```

- `--comfyui-dir` falls back to `RIGSPARK_COMFYUI_DIR`; it must be a directory with a
  `models/` subdirectory. `--port` defaults to 8188 (ComfyUI default).
- `--prompt`: 1..=16384 bytes, no control characters other than newline/tab.
- `--seed`: 0..=2^53−1; random when omitted (reported in output for reproducibility).
- Default output: `./rigspark-<kind>-<prompt-id-prefix>.<png|webp>`; an explicit
  `--output` must use the produced extension.

### 6.1 Terminal UI (v1.1)

`rigspark generate --tui`, or `rigspark generate [image|video|ID]` on an interactive
terminal without `--prompt`, opens a form: model list with fit badges, prompt,
ComfyUI directory (prefilled from `--comfyui-dir`, `RIGSPARK_COMFYUI_DIR`, or
`~/ComfyUI`), and seed. Enter generates, Tab moves between fields, Esc/Ctrl+C cancels a
running job (removing the queued ComfyUI prompt) or exits. Progress lines stream
live. Saved paths are printed after the terminal is restored. Models that do not fit are
refused unless `--bypass` was passed.

### 6.2 Browser GUI (v1.1)

`rigspark gui` gains a **Create** view backed by loopback endpoints:

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/generation/models` | Models with memory-only fit, default ComfyUI dir/port |
| POST | `/api/generation/jobs` | Start one job (`{model, prompt, comfyuiDir, seed?, port?, bypass?}`); 409 if one runs |
| GET | `/api/generation/jobs/current` | Status, last 200 progress lines, result, `outputUrl` |
| POST | `/api/generation/jobs/current/cancel` | Cancel the running job |
| GET | `/api/generation/jobs/{id}/output` | PNG/WebP preview of the finished job only |

Starting and cancelling jobs requires the GUI capability token (`X-LLMUP-Token`), since jobs
write weights into a user-chosen directory. Unknown request fields (e.g. custom workflows) are
rejected. Outputs are saved under `~/.rigspark/generations/` (0700) and served only by job id.

### 6.2.1 Generation from chat (v1.2)

The chat composer has a **Text · Image · Video** toggle (`role="radiogroup"`, arrow-key navigable,
default Text). In Image or Video mode, sending a message (button or Enter) starts a generation job with
the message verbatim as the prompt, using the kind's default model and the Create view's ComfyUI
directory, port, and bypass setting. It is not sent to the LLM. The conversation shows the user bubble
plus an assistant bubble that streams progress and then renders the PNG or animated WebP inline with
its seed, path, and a Download link. While a generation runs, Send becomes Stop (cancels the job) and
the toggle is locked. Text mode uses the existing chat path unchanged. Generated media is not added to
LLM context or persisted session history (v1.2 limitation). The GUI keeps the last 64 finished outputs
viewable by job id, so earlier results in a thread keep working.

### 6.3 Resumable downloads (v1.1)

Live testing showed a dropped CDN connection discarding gigabytes of progress. Downloads
now resume with HTTP `Range` (up to 8 times, with backoff) when the server returns `206`, a
`Content-Range` starting at the verified offset, and the pinned commit. Hashing continues
over the appended bytes, and the final full SHA-256 and exact-size check are unchanged, so
a bad resume still fails closed.

## 7. Commands

```bash
cargo test -p rigspark-core --locked --test generation
cargo test -p rigspark-runtime --locked --test generation
cargo test -p rigspark-cli --locked --test generation_cli
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo native-retirement
```

## 8. Testing strategy

- **Core (unit/integration):** dataset parses; each validation rule rejects bad input;
  verdict yes/slow/no boundaries for unified, discrete, CPU-only hardware; resolve by
  kind/id; listing text shows `speed unknown`.
- **Runtime (integration with fake transports):** happy path image + video (submit →
  poll → fetch → file written); Partner-node rejection; non-loopback endpoint; node
  errors; execution error; unsafe output filename; clobber refusal; weights missing from
  ComfyUI listing; cancellation sends queue delete; integrity mismatch on download.
- **CLI:** help/flag contract, `catalog --generation` output, argument validation
  failures before any network or state access.

## 9. Boundaries

- **Always:** pinned + hashed weights, loopback only, allowlisted local nodes, typed errors.
- **Ask first:** adding models with non-Apache/MIT licenses, image-to-video or custom nodes,
  managing the ComfyUI install, new dependencies.
- **Never:** Partner/API nodes, credentials, cloud endpoints, fabricated speed numbers,
  overwriting user files, real network/ComfyUI in tests.

## 10. Success criteria

- [x] `catalog --generation` prints both models with yes/slow/no and `speed unknown`.
- [x] `generate image|video` produces a PNG / animated WebP via a running local ComfyUI
      (live on Apple M4 Max via CLI, TUI, GUI; image output coherent).
- [x] Wan 2.1 video content is coherent on Apple MPS at the official size (euler on MPS; see plan T10).
- [x] Any non-allowlisted node class, non-loopback endpoint, digest mismatch, or unsafe
      output descriptor fails closed with a typed error.
- [x] Existing LLM catalog/advice goldens unchanged; fmt, Clippy, tests, retirement gate pass.
