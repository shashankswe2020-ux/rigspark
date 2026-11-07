# Demo Production

[Back to README](../../README.md)

## Current Demo

The [README video](../../assets/rigspark.mp4) is derived from the current
[VHS terminal recording](../../assets/demo.gif). It is 19.76 seconds,
1200x700, with H.264 video and no audio.

The [README preview](../../assets/rigspark-preview.gif) is derived from that
same recording: 19.8 seconds, 960x560, 10 fps, silent loop.

## Previous Render

The notes below describe the superseded render, not the current README video.
Its GIF, poster, and share copy are retained but are no longer used in the README.

The previous demo was rendered with **Hyperframes 0.8.85**, using the
[brag skill](https://github.com/latent-spaces/brag/tree/c893c5ed52aed84e3e2ee56787de869fccdae6b0/skills/brag).
It is a new composition of existing product footage, not a fresh runtime
benchmark or a new live capture.

## Deliverables

- Previous video: 22 seconds, 1920x1080, 30 fps, H.264/AAC; replaced by the current demo.
- [Previous preview](../../assets/brag.gif): 22 seconds, 960x540, 12 fps, silent loop.
- [Poster](../../assets/brag.jpg): the settled evidence screen at 11 seconds, also baked into the video's first frame.
- [Share copy](../../assets/brag-share-copy.txt).

## Storyboard

| Time | Content |
| --- | --- |
| 0-4s | Know before you download; yes / slow / no verdicts |
| 4-10s | Real terminal recommendations and model details |
| 10-13.5s | Model evidence, with the estimates-not-benchmarks caveat |
| 13.5-17s | Local chat workspace |
| 17-22s | Product name, installation command, and supported platforms |

## Sources and Credits

- Product media: [original CLI recording](../../assets/demo.gif), [model evidence](../../assets/model-performance.png), and [workspace](../../assets/screenshot-workspace.png).
- Visual identity: the project's site styles (`site/styles.css` in the private rigspark-web repository).
- Music: **Happy Beats / Business Moves, Vol. 12** by [ende.app](https://ende.app/en), from the brag skill's bundled music library. No voiceover.
- Type: Archivo and IBM Plex Mono, from [Google Fonts](https://github.com/google/fonts), embedded in the render.
- Hyperframes domain skill revision: `73d26a5187c2b24c7613e9ec58abfc97b4d5c954` in [heygen-com/hyperframes](https://github.com/heygen-com/hyperframes).

## Verification

Hyperframes `check --snapshots` passed with zero errors, zero layout findings,
and 23/23 contrast checks passing. Six non-blocking lint warnings suggested
splitting scenes into separate composition files. Key frames were inspected.
The final MP4 was fully decoded with FFmpeg and its duration, resolution,
frame rate, and audio stream checked with FFprobe.

Hyperframes rendered all 660 frames with hardware-GPU browser capture. FFmpeg
then baked in the poster and derived the GIF. The renderer's newly downloaded
Chrome failed to start; selecting the working cached Chrome with
`HYPERFRAMES_BROWSER_PATH` resolved it.

All Node-based authoring tools and the editable composition were kept in an
external temporary workspace. No Node dependencies or build tools were added
to this Rust repository.