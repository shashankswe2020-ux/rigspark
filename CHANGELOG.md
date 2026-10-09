# Changelog

## Unreleased

### Fixes

- GUI: on narrow screens a long conversation could make the whole page
  scrollable, adding blank space under the chat and stopping the reply from
  following new text. The screen-reader-only "You"/"Assistant" labels now stay
  inside their message.

### CI

- Desktop verification runs on `main` again (it only targeted retired migration
  branches), uses the same `--test-threads=2` as CI, and finds ChromeDriver even
  when the runner image doesn't set `CHROMEWEBDRIVER`.
- Browser journeys retry clicks on list items that were re-rendered, and the
  narrow-screen scroll checks use the chat's real scroller.

## 3.0.2 - 2026-10-09

### Support the project

- `brew install` now ends with a short note: if Sparky helps you, please
  consider sponsoring the project at <https://buymeacoffee.com/shashanksw9>.
  Cargo can't print messages after an install, so `rigspark --help` ends with
  the same note. The README, guide and site link to it too, and the repository
  shows a Sponsor button.

### Fixes

- GUI: the "Skills this agent loads" checkboxes in the Library agent editor no
  longer stretch to half the row and squeeze each skill name onto several lines.

### Tests

- The `gui_launcher` process tests no longer time out under parallel load on
  macOS.

### Docs

- The guide's agents/skills and connectors recordings show the 3.0 interface.

## 3.0.1 - 2026-10-09

### Licensing

- Release archives now include `fonts.OFL.txt`, the SIL Open Font License 1.1
  for the Inter and JetBrains Mono fonts that `rigspark-gui` has embedded since
  3.0.0. `THIRD-PARTY.md` lists both fonts with their versions and upstreams.
- Correct the copyright notices in the font license shipped with the GUI and
  the site: they named Bricolage Grotesque instead of the fonts RigSpark
  actually ships (Inter 4.001 and JetBrains Mono 2.211).

### Tests

- The full-catalog ranking test no longer depends on the CI runner's live
  memory. Route tests check the limit bounds, and the full-catalog properties
  are checked against fixed hardware.

## 3.0.0 - 2026-10-08

### A redesigned workspace

`rigspark gui` and the desktop app have a new, minimal interface. Every feature
is still there, but each control now appears where and when you need it.

- **Model chip.** One chip in the top bar holds the model, runtime, harness, and
  agent pickers, the endpoint/turns/context status, and Refresh.
- **Inspector.** Live RAM/CPU/disk/latency, the latest call's token usage, and
  hardware move into a panel you open from the top bar, with ⌘I, or by clicking
  Sparky.
- **Sparky status companion.** The sidebar's Sparky and memory ring follow real
  RAM pressure (*All good*, *Getting busy*, *Memory is tight*). When telemetry
  is unavailable it says *Metrics offline* instead of guessing.
- **Chat.** A 3D Sparky welcome with suggestion pills; a composer **+** menu for
  context, skills, and the system prompt; a Text/Image/Video switch; an agent
  chip; and a round Send button. Sparky is the assistant's avatar.
- **Models.** A ranked list with a Sparky face per verdict, search, and
  *All / Runs well / Slow / Won't fit* filters. A memory bar appears only when
  both required and usable memory are known. Context window, KV cache, and fit
  toggles move into a **Fit settings** popover, and the per-model runtime
  picker appears on hover.
- **Full catalog.** Models now loads the whole ranked catalog (up to 1,000
  entries; 253 today) instead of the top 100, so search and filters reach every
  model. The top results are unchanged.
- **Connectors.** An empty state with Filesystem, Git, and Custom templates that
  open a pre-filled *Add connector* sheet. The JSON editor is a collapsible
  section.
- **Library.** Agents and Skills are tabs, with starter templates that pre-fill
  the form.
- **Runtime.** System Settings-style grouped lists for this machine and the
  inference runtimes. No hardware score is shown, because none is sourced.
- **Light and dark themes**, remembered across launches; ⌘1–⌘5 switch views and
  Esc closes popovers and sheets.
- Refreshing an empty chat keeps the welcome instead of a blank pane.

### Desktop app

- New app icon from the Sparky brand kit, with transparent corners.
- The window opens on the dark workspace colour instead of flashing white.

### Site

- The site's model count now comes from the shipped catalog (253 models, up
  from a hand-written 69). `cargo catalog-site` stamps it into the page, so the
  weekly admission keeps it current, and CI fails if it drifts.
- **Ask Sparky** now takes your graphics (Apple Silicon, NVIDIA, AMD/Intel, or
  CPU only), memory or VRAM, and free disk. It applies the CLI's own fit rule,
  marks downloads that won't fit on disk as *no*, and shows estimated tok/s only
  for hardware classes with sourced bandwidth data, otherwise *unknown*. Model
  sizes and hardware classes are generated from the catalog and `perf.json`
  instead of being typed into the page; across 1,008 combinations the preview
  matches `rigspark can-run` exactly.

### Breaking changes

- The GUI layout and markup changed. Element IDs used by the app's own scripts
  are kept, but anything that automates the old layout (for example, expecting
  the model pickers, fit controls, or metrics to be visible without opening
  their popover or panel) needs updating.
- `/static/mascot-avatar.jpg` and `/static/mascot-welcome.jpg` are removed.
  Brand assets are now served from `/static/brand/` and fonts from
  `/static/fonts/`. SVGs are served with a sandboxed Content-Security-Policy.

## 2.4.0 - 2026-10-08

### Image and video models

- Pick the active Text, Image, and Video model in separate Models tabs; Chat
  generates with the selected image or video model directly. The separate
  Create view is retired, ComfyUI is detected automatically, and finished
  images and videos have a Download action.
- Run FLUX.1 schnell FP16 through ComfyUI's official split workflow (UNET,
  T5, CLIP-L, and VAE as separate pinned files). The previous single-file
  entry could not load, because that file holds only the diffusion model.
- Add a Wan 2.1 T2V 1.3B BF16 variant, and lengthen Wan videos from 33 to 49
  frames (about 3 seconds at 16 fps).
- Add Qwen-Image and Wan 2.2 TI2V-5B built-in workflows, following ComfyUI's
  official example graphs.
- Skip re-hashing cached weights that are unchanged since their last full
  SHA-256 check (same device, inode, size, mtime, and ctime). Any change still
  forces a full hash; Windows and whole-second filesystems always re-hash.

### Catalog

- Admit the latest open local models every week. Text models are admitted only
  when every fact comes from the shipped artifact; image and video releases
  without a built-in workflow are fit-only and labelled *workflow coming*.
  Auto-admitted entries are marked *auto-sourced*, and unsourced facts stay
  `unknown`.
- Filter by recency with `--month 1|2|3` on `recommend` and `catalog`, `m` in
  the terminal UI, and "Released in" in the GUI.
- Add review-gated weekly image and video enrichment from Hugging Face,
  including current LFS metadata (`lfs.sha256`).
- Cut `recommend`, `can-run`, and `catalog` startup to about 10 ms by
  compiling validation patterns once instead of per model.

### Distribution and docs

- Publish a multi-platform container image (`ghcr.io/shashankswe2020-ux/rigspark`)
  built from the verified release archives. Linux release binaries need glibc
  2.39 or newer.
- Rewrite the README and site around Sparky.

## 2.3.0 - 2026-10-05

### Local image and video generation

- Add a curated, offline generation catalog with FLUX.1 schnell (image) and
  Wan 2.1 T2V 1.3B (video), both Apache-2.0, pinned to Hugging Face commits
  with SHA-256 digests. `rigspark catalog --generation` shows a memory-only fit
  verdict; generation speed is reported as `unknown`.
- Add `rigspark generate image|video` for an existing local ComfyUI install:
  verified weight install, built-in workflows from the official examples,
  loopback-only, no credentials, and a fail-closed allowlist that keeps paid
  Partner/API nodes unreachable. Outputs are never overwritten.
- Generate from the terminal UI (`rigspark generate --tui`), from the GUI Create
  view, or straight from chat with the new Text · Image · Video toggle.
- Use the euler sampler for Wan on Apple MPS, where the official uni_pc sampler
  produced corrupt video in live testing.
- Unload the previous workflow's models through ComfyUI's `/free` when
  switching between image and video on Apple Silicon (shared memory), or on
  discrete GPUs when free memory is below the next model's weights.
- Resume interrupted multi-gigabyte downloads with HTTP Range while keeping the
  final SHA-256 and size checks fail-closed.

### Terminal UI

- Replace the model list with a sortable table (rank, quant, memory need,
  verdict, tok/s, score) that drops low-priority columns on narrow terminals;
  add verdict filters, fuzzy name search with highlighted matches, a memory fit
  gauge, side-by-side comparison, and copy of the model id, `rigspark up`
  command, or a 72-column shareable card over OSC 52.
- Stream chat replies with lightweight markdown, scrollback, copy of the last
  reply, and locally measured first-output and total time.
- Show `up`, `switch`, and `down` progress as a stage checklist with elapsed
  time; unfinished stages stay unconfirmed.
- Add mouse support (opt out with `RIGSPARK_NO_MOUSE`), scrollbars,
  synchronized output, and `RIGSPARK_THEME` light and high-contrast palettes.

## 2.2.0 - 2026-10-02

### Catalog quality and automation

- Add source-backed catalog enrichment proposals with bounded official metadata
  collection, optional extraction, explicit human review, and separate evidence
  promotion, model admission, and signing steps.
- Require independently sourced 90% release-quality gates and preserve audit,
  discovery, tag-inventory, and Hugging Face evidence for reproducible review.
- Make weekly catalog refreshes resumable and diagnosable with cursor-based
  batches, deterministic observations, bounded rejection reasons, pending-review
  protection, and visible partial failures without relaxing publication gates.

### Models and advice

- Add Bonsai and Qwen3.5 catalog entries with pinned provenance and honest GUI
  advice. Uncalibrated binary throughput remains `unknown`.
- Distinguish newly installed and already-current signed catalog updates in the
  GUI, and keep success or failure feedback visible after an update attempt.

### Chat rendering

- Render TeX offline with pinned KaTeX assets while preserving the existing
  sanitized Markdown boundary.
- Harden rich-response rendering for malformed and unsafe content, preserve
  Unicode boundaries in streamed fixture responses, and align response timing
  metadata with rendered output.

### Distribution

- Align generated Homebrew formula metadata with release archives.

## 2.1.0 - 2026-09-30

### Independently Updated Catalogs

- `rigspark catalog --update` explicitly downloads and activates a signed model
  catalog without replacing the application. Advice stays offline.
- `rigspark catalog --status` and the Models view expose snapshot provenance;
  the GUI has an explicit update action with non-destructive failure handling.
- Ed25519 verification, bounded HTTPS downloads, atomic shared cache activation,
  revision rollback checks, and verified previous/bundled fallback protect updates.
- Provision the production public key and protected catalog-only publication
  workflow. The channel must be published before live updates are available.

### Qwen 3.6 and Projector Integrity

- Add `qwen3.6:35b` (35B total / 3B active) with sourced model and vision-projector
  sizes/digests. Hybrid KV geometry and unsourced benchmark proxy remain unknown.
- Count projector weights in enrichment sizing and pin projector identity/size
  during Ollama pull and context activation. Invalid pins fail before acquisition.
- Add optional `projectors` metadata to catalog quantizations. Older strict
  readers reject this extension instead of ignoring integrity requirements.

## 2.0.0 - 2026-09-30

### Breaking: renamed to RigSpark

- The primary executable is now `rigspark`; `llmup` stays as a compatibility
  alias. Crates publish as `rigspark-cli`, `rigspark-gui`, `rigspark-core`,
  `rigspark-runtime` and `rigspark-crossterm`, replacing the `llmup-*` crates.
- Environment variables use the `RIGSPARK_` prefix, including `RIGSPARK_HOME`.
  Stop running servers, then move your data directory to `~/.rigspark` or point
  `RIGSPARK_HOME` at it. Nothing is moved or deleted automatically.
- The desktop bundle identifier is now `org.rigspark.desktop`, so operating
  system permissions may need to be approved again.
- Release archives are named `rigspark-<target>`.

### KV cache profiles

- `--kv-cache` sizes advice at a chosen KV cache type (`fp16`, `q8_0`, `q4_0`),
  and `up`/`switch` apply the cache profile to owned launches.
- The browser GUI adds a KV cache selector with offline re-sizing.

### Fixes

- The browser GUI keeps its recommendation JSON unchanged unless a KV cache type
  is chosen, matching `can-run`.

## 1.0.2 - 2026-09-28

### Fixes

- Tool-using chat works again with Ollama 0.32 and newer. Ollama now adds an
  `id` and an `index` to each tool call; rigspark rejected those fields, so
  chat with MCP connectors or agent tools either failed outright or silently
  dropped every tool call and returned an empty reply.

## 1.0.1 - 2026-09-27

### Install

- Every tagged release now ships verified prebuilt archives for macOS (Apple
  Silicon, Intel), Linux (x64, ARM64) and Windows (x64) with a `SHA256SUMS` file.
  Install with `brew install shashankswe2020-ux/tap/rigspark` or
  `cargo binstall rigspark-cli rigspark-gui`.

### Planning and hardware profiles

- `llmup plan <model>` shows every execution path (GPU, multi-GPU, CPU offload,
  unified memory, CPU) with fit, memory shortfall and throughput basis. Paths
  split across devices report speed as `unknown` rather than an estimate.
- `--hardware <profile.json>` evaluates `recommend`, `can-run`, `doctor`,
  `catalog` and `plan` against a saved hardware profile.
- Unified memory is detected from memory topology (NVIDIA shared-memory SoCs and
  AMD APUs), not only Apple Silicon.

### Fixes

- `llmup down --forget` clears a stale pointer to an attached server whose daemon
  restarted; previously `up`, `down` and `doctor` failed until `state.json` was
  edited by hand. Owned servers are still never cleared without stopping them.
- `llmup gui` stops its browser host when the terminal closes, and the host
  handles `SIGTERM` and `SIGHUP` gracefully, so GUI-started Ollama daemons are no
  longer orphaned.
- The GUI never sends connector environment values (such as API secrets) to the
  browser; the config editor shows placeholders and keeps saved values.
- GUI model start failures show the real reason instead of `invalid request`.
- GUI sizes are labelled GiB/MiB/KiB to match their binary units, the chat turn
  count refreshes when switching chats, and per-message tok/s covers decode time
  only and is omitted when a reply is too short to estimate.

## 1.0.0 - 2026-09-26

### Native Rust release

- `rigspark` is now a native Rust application. The TypeScript sources, npm
  package, Electron shell, and every Node.js build, test and CI dependency are
  retired; `cargo native-retirement` fails if any return.
- Install from crates.io: `cargo install rigspark-cli --locked --bin llmup --bin rigspark`
  and `cargo install rigspark-gui --locked`. The npm package receives no further
  releases; commands, flags, JSON output and `~/.rigspark` state are unchanged.
- Measured against the published 0.11.4 on an Apple M4 Max: startup is about 24×
  faster (5.8 ms vs 139.6 ms), advice commands 1.4–1.7× faster, and peak memory
  falls from about 72 MiB to 15 MiB. Advice JSON is equivalent.
- The curated dataset moved to `crates/rigspark-core/data/` and the vendored browser
  libraries to `crates/rigspark-gui/vendor/` so they ship inside the published crates.
- The bounded-input crossterm patch is published as `rigspark-crossterm`; Ratatui
  keeps upstream crossterm for rendering only, and a test pins that all terminal
  input goes through the patched parser.

### Fixes found during the migration

- Hardware detection no longer times out on macOS: free disk space comes from
  `statvfs` instead of a purgeable-space query that could take 30+ seconds.
- Auto-selection and `doctor` never choose attach-only LM Studio as the default.
- MLX model directories that name a custom loader (`model_file`) are rejected.
- Windows LM Studio executables are trusted regardless of path prefix, separator
  or case differences.
- `switch` and `down` reject a `--backend` or `RIGSPARK_BACKEND` that conflicts
  with the active server instead of ignoring it.
- Provider text after an SSE `[DONE]` terminator is no longer shown.
- Memory stores readable by group or other users now fail closed.

### CI

- All workflows run without Node-backed actions: an exact-revision `git fetch`
  replaces checkout, Pages publishes the `gh-pages` branch, and the backlog
  workflow uses `gh project item-add`. The Node TUI compatibility matrix is removed.
- Browser client unit suites (run reducer, SSE framing, Markdown policy,
  calculator template, live telemetry) now run in real Chrome through WebDriver.

## 0.11.4 - 2026-09-17

### Development Tooling

- Upgraded Vitest and V8 coverage to 5.0.1 and refreshed vulnerable transitive
  development dependencies. Root and desktop dependency audits report no known
  vulnerabilities at release preparation.
- Development tests now require Node.js 22.12+; built CLI runtime compatibility
  remains Node.js 18+. CI runs build/test tooling on Node 22 before checking the
  built CLI on the existing Node 18/20/22/24 matrix.
- Added backend validation and failure-path tests, keeping the existing coverage
  thresholds intact under the updated coverage engine.

### Model Selection and Context

- Added `--bypass` to `up` and `switch` for explicit estimated-fit overrides,
  while retaining weight integrity, disk, loopback, and process-ownership checks.
- Added installed Ollama model comparisons through `recommend --installed` and
  `can-run --installed`, with exact tags, custom ports, context sizing, and an
  optional known-fit filter. Missing KV geometry and throughput remain unknown.
- Added `can-run --context` and runtime context configuration for Ollama. Context
  variants preserve the original model tag and are used by CLI and desktop chat;
  `ls` reports the runtime tag for OpenCode and other OpenAI-compatible clients.
- Added desktop installed-model selection, 64K/custom context controls, fit
  filtering, and explicit bypass confirmation. Starting from either catalog
  cards or model details now carries the displayed context into activation.

### Integrity and Compatibility

- Verify uncatalogued installed models against their local manifests and every
  referenced blob, preserving catalog digest or size-floor checks when available.
  Local content integrity is explicitly distinguished from catalog provenance.
- Preserve externally owned Ollama daemons and reject changed process identity
  or model manifests before recording activation. Switching models clears stale
  context-variant metadata.
- Keep ordinary advice deterministic and offline, and preserve side-effect-free
  launch-module imports through shared context validation.

## 0.11.3 - 2026-09-03

**OpenCode harness with visible tool activity, plus a create-workspace surface.**

### Chat Panel and OpenCode Harness

- Added the `opencode` chat harness. It drives the OpenCode CLI in JSON mode
  through a shell-free child process, streams its `text` events, and now
  surfaces `tool_use` and `reasoning` events as inline markdown blockquotes
  (e.g. `🔧 \`write\` · path`, `🔧 \`bash\` · command`) so the panel shows the
  model's actual tool loop instead of only the final answer. Bare model IDs
  are normalized to `ollama/<id>` and an inline local Ollama provider is
  injected when the target is Ollama, so a workspace-selected local model just
  works.
- Fail-closed by default: `permission: "deny"`, `share: "disabled"`,
  `autoupdate: false`, `snapshot: false`, `shell: false`. Setting the env var
  `RIGSPARK_OPENCODE_UNRESTRICTED=1` explicitly opts a local machine into
  OpenCode's full tool loop (`permission: "allow"`, `share: "auto"`,
  `autoupdate: true`, `snapshot: true`). This is not the default and must be
  set on the machine — the shipped registry remains deny-by-default.
- Added output caps (1 MiB prompt / 16 MiB stdout), typed cancellation,
  malformed-event and non-zero-exit fail-closed handling, and 11 harness tests.

### Workspace

- Added `POST /api/workspace/root/create` and `WorkspaceService.createRoot()`
  so the browser panel can create an approved workspace root explicitly,
  behind the existing per-launch token and exact-origin checks. Existing
  registration semantics, canonicalization, and denylist behavior are
  unchanged.
- Added a built-in calculator template in the workspace picker: propose,
  review through the existing reviewed-edit path, and open in a sandboxed
  same-origin preview iframe (no model-generated code is ever loaded here).
  The runtime uses a CSP-safe recursive-descent evaluator, not `eval`.

### Backend Capabilities

- Added an explicit `supported | unsupported | unknown` embedding-layer offload
  capability to backend adapters. Current backends report `unknown` until
  runtime support can be observed without inference.

### Model Selection

- Prefer a recognized higher-precision quantization when multiple fitting
  variants have the same measured memory footprint. Unknown quantization
  formats remain ordered by measured size without fabricated quality estimates.

## 0.11.2 - 2026-09-01

**Broader model coverage with an auditable catalog pipeline.**

### Model Catalog

- Added the official `gemma4:e4b-it-qat` Ollama artifact with its verified model
  digest, exact weight size, 128K context cap, and QAT Q4 metadata. Context
  memory remains explicitly unknown until Gemma 4 hybrid-attention KV geometry
  is curated.
- Added curated Ollama entries for Gemma 3n E2B/E4B, Qwen3
  0.6B/1.7B/4B, and Phi-4 Mini 3.8B, including verified model-layer sizes and
  digests. Unsupported KV-cache geometry remains explicitly unknown.
- Added a weekly upstream repository coverage audit that reconciles one GitHub
  issue with missing discovery candidates. It never auto-admits models, and it
  cannot detect missing variants inside an already-covered repository because
  Ollama does not expose a public tag-enumeration endpoint.
- Added catalog overview and enrichment-process documentation to the README and
  project site.

## 0.11.1 - 2026-08-30

**Readable, secure Markdown responses in the local AI workspace.**

### New Features

- Assistant responses now render sanitized GitHub-Flavored Markdown with
  headings, nested and task lists, blockquotes, tables, links, code fences,
  language labels, and persistent Copy/HTML Preview actions.
- Streaming output is frame-batched, converges to the same final DOM regardless
  of token fragmentation, and follows only while the reader remains near the
  bottom.
- Replaced the connector demo with two real, explicitly approved WHOOP tool
  calls and an actual-value health briefing.

### Security and Accessibility

- Added GUI-specific multiline sanitization without weakening terminal output
  sanitization, plus strict Marked/DOMPurify tag, attribute, link, and image
  policies.
- Added a deny-by-default CSP, no-store handling for token-bearing HTML,
  MIME/referrer/frame/permissions hardening, and scriptless artifact previews.
- Upgraded the desktop packaging toolchain to `electron-builder@26.15.3`,
  removing the critical/high archive and credential-redirect advisories in the
  previous builder dependency graph.
- Added semantic browser coverage, stable accessible names, keyboard focus,
  concise streaming announcements, hostile-input tests, and responsive checks
  down to 320 px.

## 0.11.0 - 2026-08-30

**Model performance intelligence and a complete local AI workspace.**

### New Features

- **Dedicated model performance view.** Selecting a catalog model now opens a
  complete performance dossier with the composite recommendation score and all
  five score dimensions, estimated throughput and provenance, memory and
  context/KV-cache evidence, model metadata, capabilities, supported runtimes,
  quantization sizing and integrity state, and catalog sources. Unknown values
  remain explicitly unknown.
- **Durable chat workspace.** Browser and desktop chat now support bounded,
  owner-only multi-session history with create, search, rename, archive, delete,
  export, and restart recovery.
- **Reliable run lifecycle.** Server-owned run IDs, stop/cancellation
  propagation, terminal-state guards, durable failures, retry, and fragmented
  SSE handling prevent duplicate or late completions.
- **Explicit workspace context.** Users can select a bounded workspace, attach
  files or line ranges, paste terminal output or diagnostics, and include
  read-only Git status/diffs. The context ledger records hashes, sizes,
  truncation, and inclusion decisions.
- **Safe tools and edits.** MCP calls expose locally classified risk, redacted
  arguments/results, and approval decisions. Model edits remain inert proposals
  until diff review and hash-guarded apply; stale or conflicting files fail
  closed.
- **Native folder selection.** The hardened Electron shell exposes one narrow,
  sandbox-safe directory chooser bridge without granting renderer filesystem
  access.
- **Docker distribution.** A digest-pinned, non-root, multi-platform CLI image
  is published to GitHub Container Registry for `linux/amd64` and
  `linux/arm64`, with immutable release tags and `latest`.

### Security and Privacy

- Workspace roots require explicit user selection and remain containment- and
  symlink-checked; reads are bounded and mutations are revision/hash guarded.
- Sending workspace context to a cloud harness requires a visible disclosure
  decision before any content leaves the machine.
- The GUI and desktop Runtime Host remain loopback-only with strict Host,
  Origin, content-type, and capability checks.

### Documentation and Validation

- README and site now lead with the model performance view and provide npm,
  native desktop, and Docker installation choices.
- Added deterministic browser and Electron journeys for chat, sessions,
  cancellation, context, tools, edits, accessibility, and responsive model
  details.
- Added client reducer and arbitrary-fragment SSE tests plus expanded GUI,
  workspace-policy, session-store, and release-packaging coverage.

## 0.10.0 - 2026-08-27

**Intel Arc/Xe GPU detection.**

### New Features

- **Intel GPU detection.** Hardware detection now recognizes Intel Arc (discrete)
  and Xe (integrated) GPUs. A discrete Arc's dedicated VRAM counts toward fit and
  the yes/slow/no verdict; integrated Xe stays conservative (shared memory isn't
  credited). With no sourced Intel performance profile, throughput stays
  `unknown` rather than guessing (honesty gate). (#223)

### Fixes

- macOS desktop app is ad-hoc signed so Apple Silicon no longer reports it as
  “damaged”.
- Desktop installers (macOS / Windows / Linux) now attach to the published
  GitHub Release on every release.
- Site: OS-aware installer bar with a platform dropdown and per-OS icons.

## 0.9.1 - 2026-08-27

**Desktop app icon, one-click installers, and a green release pipeline.**

### New Features

- **Desktop app icon.** The Electron app and site now use the brand silver
  diamond mark (matching the GUI rail), wired into the window, the macOS Dock,
  and electron-builder for all platforms.
- **Direct desktop downloads.** The site's Desktop section links straight to the
  packaged installers (macOS `.dmg`, Windows `.exe`, Linux `.AppImage`).
- **Release CI builds installers.** The release workflow now builds and publishes
  the desktop installers for macOS, Windows, and Linux on every tagged release.

### Fixes

- Make the release workflow pass reliably: build before test so dist-dependent
  tests have `dist/bin.js`, poll for a stable rendered frame in TUI tests to end
  first-paint races on slower CI runners, and refresh the noninteractive goldens.

## 0.9.0 - 2026-08-27

**Agentic browser workspace: connectors, agents & skills, tools, and inline graphs.**

### New Features

- **MCP connectors.** Attach Model Context Protocol servers to the workspace —
  local `stdio` commands or loopback HTTP/SSE only. Add, enable, disable, and
  remove connectors; each connector's tools become available to the chat, which
  the model calls in an agentic tool loop.
- **Agents & skills library.** Author reusable **agents** (persona / system
  prompts) and **skills** (instruction blocks), stored locally as markdown with
  YAML frontmatter (the Claude Code / Codex convention). Full create / edit /
  enable / disable / delete. An agent can bundle the skills it always loads, and
  any skill can be toggled per message; the selection is composed into a single
  system prompt server-side.
- **Inline images & graphs in chat.** The chat panel now renders generated
  images and graphs inline, served from a validated, loopback-only artifacts
  endpoint (`GET /api/images/:name`) — basename-only, image-extension allowlist,
  no traversal.
- **Backend picker in the workspace.** Recommendation cards expose a per-model
  runtime selector so every backend (Ollama, llama.cpp, MLX, LM Studio) is
  reachable directly from the browser, not just the auto-selected default.

### Fixes

- **MLX executable check** now recognizes macOS framework Python
  (`.../Python.framework/.../Python`), so MLX chat works on Homebrew Python
  instead of being rejected as an unapproved backend executable.
- **Local chat harness** captures the live listener process identity, so
  fail-closed inference works for attached backends (e.g. LM Studio) instead of
  refusing to run without process/model-path identity.
- The Runtime pill in the chat header reflects the active backend instead of the
  dropdown default.

### Catalog

- Added small, fast validation models to the catalog and bootstrap sources
  (`qwen2.5:0.5b` across Ollama/HF/GGUF and `qwen2.5:0.5b-mlx`).

### Documentation

- Recorded new workspace demos (agents + skills + tools solving and plotting a
  quadratic with an inline graph, and the MCP connector lifecycle) and refreshed
  the site and README with the current UI.

## 0.8.1 - 2026-08-26

**Browser GUI bug fixes.**

### Fixes

- Made the Session sidebar functional: the "Current session" button now reloads
  chat history and switches to the chat view (was dead, non-functional UI).
- Removed misleading static "Previous run"/"Memory" placeholders that had no
  backing store.
- The turn count now refreshes after a successful chat instead of going stale
  until a manual refresh.
- The status strip reflects the real active-model endpoint instead of a
  hardcoded `127.0.0.1:11434`; `/api/models/active` now owns the active-model
  card, fixing a race that could overwrite it with the session placeholder.

## 0.8.0 - 2026-08-26

**In-browser model management + neobrutalist site redesign.**

### New Features

- The browser GUI can now manage models end to end: pick a recommended model for
  your hardware or start one directly, then chat — all driven by the same
  `recommend`, `up`, and `ls` engine as the CLI.
- Added a `GuiModelManager` bridge with `GET /api/models/recommended`,
  `GET /api/models/active`, and `POST /api/models/up` routes. Requests are
  Zod-validated and models are brought online through the verified `up`
  lifecycle (integrity checks and active-server state included).
- The Models view surfaces the same yes/slow/no verdicts and est. tok/s as the
  CLI, with a Start button that serves a chosen model on `127.0.0.1`.

### Documentation

- Recorded a real browser GUI demo GIF and Models-view screenshot; added a
  Browser GUI section to the README.
- Redesigned the marketing site with a neobrutalist theme (thick borders, hard
  offset shadows, cream paper, colored icon tiles) documenting every feature,
  the full 11-command surface, and the new Browser GUI.

### Validation

- Added GUI model-management unit coverage and server route tests.
- Verified with the project’s full typecheck, lint, build, and test gates.

## 0.7.0 - 2026-08-26

**Browser GUI + pluggable chat harness adapters.**

### New Features

- Added a loopback-only browser GUI for interactive chat sessions.
- Added a pluggable chat harness registry for `local`, `claude`, `openai`, and `openai-compatible` providers.
- `llmup gui` now starts a browser-backed local/cloud chat server with safe host validation and JSON output mode.
- `llmup chat --harness <name>` routes non-local prompts through the selected provider without disturbing the default local backend path.
- Cloud harness availability checks fail closed when required credentials or runtime configuration are missing.

### Validation

- Added harness unit coverage, GUI server coverage, and chat regression tests for the non-local harness path.
- Verified the feature with the project’s full test, typecheck, build, and lint gates.
- Refreshed the release demo assets to match the current command surface.

## 0.6.1 - 2026-08-10

**Documentation patch.**

- Add real TUI screenshots and end-to-end demo GIF recorded with vhs
- Replace HTML `<pre>` mockups with actual terminal captures
- Include VHS tape files for reproducible re-recording (`assets/*.tape`)

## 0.6.0 - 2026-08-10

**Terminal User Interface (TUI) — Release Candidate.**

This release adds a full interactive terminal UI that activates automatically in
capable terminals (TTY ≥60×16) and degrades gracefully to plain text elsewhere.

### New Features

- **Interactive TUI** with Ink 5 + React 18 rendering to stderr:
  - **Recommend screen** — searchable, scrollable model list with selection,
    marking, comparison, and detail overlays.
  - **Doctor dashboard** — box-drawn diagnostics, backend version table, AI
    Hardware Score axes.
  - **Catalog browser** — search, filter, refresh diff, and model details.
  - **Lifecycle progress** — real-time staged pull/verify/serve with Ctrl+C
    cancellation and partial-state compensation.
  - **Chat screen** — multi-line input (Ctrl+J), streaming responses, draft
    validation (32 KiB / 8192 graphemes / 256 lines), session summary on exit.
  - **Model picker** — keyboard-navigable model selection for switch/migrate.
  - **`ls` card** — active server status with auto-exit for implicit TUI.
- **Accessible mode** (`--accessible`) — cooked line-oriented fallback for
  screen readers. Never enters raw mode or writes cursor-control sequences.
- **Mode auto-selection** — visual / accessible / plain chosen by terminal
  capabilities, environment, and user flags.
- **Cancellation model** — signals trigger a 30-second cleanup timeout with
  compensation (partial state cleared, cursor restored, raw mode exited).
  Second signal forces immediate exit with documented exit codes.
- **Session ownership** — terminal resources (raw mode, cursor, resize listener,
  stdin pause/resume) are tracked and restored on any exit path.
- **Performance budgets** — cold-start regression ≤20 ms p90, TUI module load
  ≤150 ms p90, tarball increase ≤250 KiB, install increase ≤15 MiB.
- **Terminal hygiene guarantees** — no stuck raw mode, no hidden cursor, no
  orphan processes (proven by automated smoke tests).

### Infrastructure

- 23 new TUI test files (337 assertions) covering screens, session, keys,
  cancellation, chat limits, lifecycle, model picker, and budget gates.
- TUI-specific CI workflow (`tui-compatibility.yml`) with runtime-proof,
  package-budget, and dependency-policy jobs across macOS/Linux/Windows ×
  Node 18/20/22/24.
- `tui:package-budget`, `tui:runtime-budget`, and `tui:dependency-policy`
  scripts enforce performance gates in CI and locally.
- Total test count: **1459 tests** across **87 files**, all passing.

### Dependencies Added

- `ink` ^5.2.1, `react` ^18.3.1 (lazy-imported; zero cost on non-TUI paths)

## 0.5.0 - 2026-08-08

Phase 2 llama.cpp production hardening after real-process smoke testing.

- Replaced invalid GGUF catalog coordinates with verified Hugging Face commit,
  filename, size, and LFS SHA-256 metadata; self-managed weights now require a
  digest and are never served after an unverified pull.
- Bounded and serialized direct downloads with cancellation, byte ceilings,
  progress, redirect/SSRF validation, stale-part cleanup, owner-only cache
  permissions, symlink refusal, and atomic verified promotion.
- Chat and migration now use the active loopback endpoint and canonical runtime
  model alias instead of hard-coded ports/Ollama ids.
- llama.cpp attach/spawn/stop now bind HTTP identity to the expected model path,
  alias, listening address, PID, canonical executable, and process start time.
- Fixed repeated/cross-backend `up`, single-model `switch`, backend preference
  precedence, persistent log-pipe deadlocks, and stale-state cleanup.
- Real Ollama smoke testing fixed macOS listener identity for `ollama serve` and
  implemented the advertised embedding capability with trusted-endpoint checks,
  bounded requests/responses, timeout/cancellation, and strict vector validation.
- Added the Apple-Silicon MLX backend (audited `mlx-lm==0.31.3`) with platform gating,
  loopback-only lifecycle, process-bound inference, bounded OpenAI-compatible
  chat, per-session bearer authentication, browser-origin/content-type/body-size
  guards, custom-code refusal, vector-less embedding fallback, and fail-closed multi-file repository
  acquisition from a pinned per-file SHA-256/size manifest. Direct-adapter real
  smoke passed pull/cache→serve→inference→stop with SmolLM2 360M on a custom
  port; catalog/CLI MLX smoke remains gated on curated MLX source data.
- Completed Phase 3 backend selection: MLX is auto-preferred only on Apple
  Silicon, is omitted from non-Apple `recommend`/`can-run` servability output,
  and remains honesty-gated to unknown throughput because no cited MLX
  efficiency scalar is shipped.
- Added the attach-only LM Studio backend for GGUF and Apple-Silicon MLX models.
  It discovers downloaded models through bounded, schema-validated `lms` JSON,
  verifies locatable GGUFs against catalog SHA-256, names any unavoidable
  delegated-integrity boundary, and never auto-selects or owns the LM Studio
  process. Attach/readiness/chat/embedding are bound to the exact trusted
  executable, PID, process start, model identifier, and delegated model path.
  Real LM Studio 0.4.20+1 smoke passed exact marker chat and 768-dimensional
  embeddings on a custom loopback port.

## 0.4.1 - 2026-08-07

Bug fix: `--version` now reports the actual installed version.

- `rigspark --version` was printing a hard-coded `0.3.2` string that had
  drifted from the real package version. The CLI now reads the version from the
  bundled `package.json` at runtime, so it always matches the installed release
  and can never drift again.

## 0.4.0 - 2026-08-07

Pluggable backends (foundation) — the advisor now understands that a model can be
served by more than one runtime, and surfaces which backends apply. Ollama
remains the sole servable backend in this release; the new flags are informational.

- `doctor` gains a **Backends** section: each known backend (`ollama`,
  `llamacpp`, `mlx`, `lmstudio`) with its installed/version status and the
  default. Detection is offline and best-effort — an absent backend reports
  cleanly rather than erroring.
- `recommend` and `can-run` now surface a **`backends`** list per model and a
  **`throughputBackend`** field in `--json`, pinned to `ollama` by default so
  advice stays deterministic and byte-identical regardless of what is installed.
- `recommend`/`can-run` gain **`--backend <name>`** to scope the throughput
  estimate to a specific runtime; `recommend` gains opt-in
  **`--available-backends`** to filter to models servable by an installed
  backend. The default advice path never probes installation and never drops
  models — unsourced `(class, backend)` pairs report `unknown` (honesty gate).
- The model catalog now accepts **`gguf`** and **`mlx`** sources alongside
  `ollama`, and memory capture is vector-less when the active backend cannot
  embed (no fabricated vectors). Internal: backend registry, capability
  descriptors, intent-split selection, fail-closed user config, and a state
  schema v2 (with v1→v2 migration) now underpin all commands.

## 0.3.2 - 2026-08-06

Docs: the README now includes visual aids so command output is easier to grasp.

- Added Mermaid diagrams — a command lifecycle flowchart and a `yes / slow / no`
  verdict decision tree — plus `xychart-beta` performance graphs for estimated
  throughput (tok/s) and the AI Hardware Score breakdown. No code changes.

## 0.3.1 - 2026-08-06

Bug fix: `up` no longer fails for models without a recorded catalog digest.

- The size-only integrity fallback previously required the downloaded weights to
  match the catalog's approximate `diskBytes` **exactly**. Because that figure is
  a rough estimate — real Ollama pulls routinely differ, and are often larger —
  `up` (and `switch`) failed with a spurious `size mismatch` for any model that
  lacks a recorded SHA-256. The fallback is now a plausibility floor: it rejects
  only grossly-truncated/empty downloads (below half the estimate) while
  tolerating benign differences. The strict digest path — and Ollama's own
  manifest verification during `pull` — are unchanged, so integrity is preserved.

## 0.3.0 - 2026-08-06

Context-window sizing — `recommend` now understands how much context each model
can actually hold on your machine, and how a target window changes the ranking.

- `recommend` gains **`--context <tokens>`**: re-ranks the catalog with the KV
  cache sized at your chosen window (fp16), so models that no longer fit at that
  context drop out or fall to `slow` with a `context-bound` reason.
- `recommend` gains **`--max-context`**: reports the largest context each model
  can hold on your hardware, bounded by either the model geometry or your memory
  (`boundBy: model | hardware`).
- `--context` and `--max-context` are mutually exclusive; both surface in
  `--json` alongside a `kvPrecision: "fp16"` field. Unknown geometry reports
  `unknown` rather than a fabricated number (honesty gate).
- Site adds a full command reference; README revamped with real per-command
  terminal output.

## 0.2.0 - 2026-08-06

Local AI Hardware Advisor (v1.0) — the tool now tells you not just what fits, but
how well it will run, with no pricing data or maintenance liability.

- `doctor` now reports an **AI Hardware Score** (0–100) and your **primary
  bottleneck** (VRAM / RAM / compute / storage); `--json` includes both.
- New **`can-run <model>`** command: a single `yes | slow | no` verdict with the
  binding reason and an estimated tok/s range. Exits non-zero only for `no`, so
  it is scriptable (`rigspark can-run <model> && rigspark up <model>`).
- `recommend` gains a **Verdict** (✓ yes / ⚠️ slow / ❌ no) and **Est. tok/s**
  column; `--json` gains `verdict` and `estTokPerSec` per row.
- Added a memory-bandwidth **throughput estimator** (roofline model) backed by a
  curated, cited hardware performance dataset (`data/perf.json`). Throughput is
  always a range; hardware with no profile reports `unknown` rather than a
  fabricated number (honesty gate).
- Ranking order and existing command behavior are unchanged.

## 0.1.0 - 2026-08-05

- Initial public release of rigspark.
- Added hardware-aware model recommendation and local install/serve flows.
- Added chat, migrate, ls, catalog, and doctor commands.
- Added a curated model catalog with weekly refresh automation hooks.
