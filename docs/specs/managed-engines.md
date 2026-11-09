# Spec: Managed engines (ComfyUI and mlx-audio)

> Status: **Implementing**
> Last updated: 2026-11-09
> Supersedes the "Attach-only" install decision in
> [image-video-generation.md](./image-video-generation.md) and the "pip install mlx-audio"
> step in the voice docs.

## 1. Problem

Image and video generation need ComfyUI; voice on Apple Silicon needs `mlx-audio`. Until now the
user had to install both from a terminal and start ComfyUI by hand before every session
(`python main.py`). Most people who pick **Image**, **Video** or **Voice** in the composer
never get past that.

## 2. Decision

One consent click, then automatic (confirmed with the maintainer, 2026-11-09):

1. The first time Image, Video or Voice is chosen and no engine is available, the composer
   shows one card: what will be installed, where, and that it downloads several GB. Nothing
   is downloaded before the click.
2. RigSpark installs the engine into its own folder (`<home>/engines`), never into the
   user's Python, Homebrew or `~/ComfyUI`.
3. After that RigSpark starts, health-checks, reuses and stops the engine itself, the same
   way it runs Ollama. Quitting RigSpark stops what it started.

Existing installs keep working and win: a ComfyUI already listening on `127.0.0.1:8188`, an
existing `~/ComfyUI` (or `RIGSPARK_COMFYUI_DIR`) with its own virtual environment, and an
`mlx_audio.server` on `PATH` or in `RIGSPARK_MLX_AUDIO_SERVER` are used as before. An
existing `~/ComfyUI` that is not running is started by RigSpark with its own Python.

## 3. What gets installed (all pinned)

| Piece | Pin | Verified by |
| --- | --- | --- |
| `uv` (installs Python and packages) | 0.13.7, official GitHub release tarball per platform | SHA-256 from the release's `.sha256` file, checked before extraction |
| ComfyUI | v0.38.2 source tarball (GPL-3.0, unmodified upstream) | SHA-256 of the tarball |
| Python | 3.12 (python-build-standalone via `uv`) | `uv` |
| ComfyUI's packages | its `requirements.txt`, resolved with `--exclude-newer 2026-11-01` | `uv`; the date makes two installs resolve the same versions |
| mlx-audio (Apple Silicon only) | `mlx-audio==0.6.4`, same date | `uv` |

Model weights are unchanged: still pinned and SHA-256 verified by the generation and voice
code before ComfyUI or mlx-audio sees them.

`uv`'s cache, its Python builds and both environments live under `<home>/engines`; `uv`
runs with `UV_NO_CONFIG=1` so a user's own `uv` or pip configuration cannot redirect it.
Downloads RigSpark makes itself accept only HTTPS from GitHub's release and archive hosts,
follow at most five redirects, and stop at a size ceiling. An interrupted install leaves no
"installed" marker, so the next click starts clean.

ComfyUI is GPL-3.0. RigSpark (MIT) does not ship or modify it: it downloads the upstream
release at the user's request into the user's own folder, like `pip install` would.

## 4. Lifecycle

- **Start**: `<venv>/bin/python <ComfyUI>/main.py --listen 127.0.0.1 --port <free port>
  --disable-auto-launch`, ready when `GET /system_stats` answers (first start imports
  PyTorch, so up to 5 minutes).
- **Reuse**: a started ComfyUI serves every later generation until RigSpark quits.
- **Stop**: on shutdown, or if it exits it is restarted on the next generation.
- **Loopback only**: never `--listen 0.0.0.0`.

## 5. Platforms

| | ComfyUI | Voice |
| --- | --- | --- |
| macOS, Apple Silicon | managed | managed (mlx-audio) |
| macOS, Intel | managed (CPU; slow) | attach a local server |
| Linux x86_64 / arm64 | managed | attach a local server |
| Windows | existing install or attach (managed install later: needs the CUDA wheel index) | attach a local server |

## 6. Surfaces

- `GET /api/generation/engines`: each engine's state (`missing`, `installing`, `ready`, `failed`,
  `unsupported`), source (`managed`, `existing`), current step and detail.
- `POST /api/generation/engines/{comfyui|mlx-audio}/install`: starts the install; same-origin and
  token checked like every other mutation.
- Generation jobs no longer require a ComfyUI directory or port from the client: RigSpark
  resolves and starts ComfyUI itself.
- Composer: a setup card in place of "ComfyUI was not detected" and "pip install mlx-audio".

## 7. Testing

The network (`Fetch`) and install commands (`Run`) are injected. Tests cover: pins and
platform selection, digest and host refusal, step order and arguments, the installed marker
only after every step succeeds, existing installs winning, and ComfyUI start/reuse/stop with
a fake `python` that serves `/system_stats`. No test downloads anything or starts ComfyUI.
