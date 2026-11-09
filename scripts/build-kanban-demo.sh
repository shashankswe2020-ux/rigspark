#!/usr/bin/env bash
# Composes a polished MP4 for the multi-file Kanban demo, showing OpenCode's
# actual tool activity (bash + write + read cards) inline in the chat panel.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FRAMES="$ROOT/demos/chat-features/frames-kanban"
OUT_DIR="$ROOT/demos/chat-features"
WORK="$(mktemp -d /tmp/llmup-kanban.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

PY=/tmp/llmup-demo-venv/bin/python
[[ -x "$PY" ]] || { echo "run: python3 -m venv /tmp/llmup-demo-venv && /tmp/llmup-demo-venv/bin/pip install Pillow" >&2; exit 1; }

DUR=3.2
FPS=30
Z_FRAMES=$(python3 -c "print(int($DUR*$FPS))")

"$PY" - "$FRAMES" "$WORK" <<'PY'
import sys, os
from PIL import Image, ImageDraw, ImageFont

frames_dir, work = sys.argv[1], sys.argv[2]
W, H = 1600, 1066

def font(sz):
    for c in ("/System/Library/Fonts/Supplemental/Arial Bold.ttf",
              "/System/Library/Fonts/Helvetica.ttc",
              "/System/Library/Fonts/Supplemental/Arial.ttf"):
        if os.path.exists(c):
            try: return ImageFont.truetype(c, sz)
            except Exception: pass
    return ImageFont.load_default()

def center(draw, y, text, fnt, fill):
    bbox = draw.textbbox((0,0), text, font=fnt)
    tw = bbox[2] - bbox[0]
    draw.text(((W - tw) // 2, y), text, font=fnt, fill=fill)

def make_card(path, lines):
    img = Image.new("RGB", (W, H), "black")
    d = ImageDraw.Draw(img)
    y = H // 2 - 140
    for text, sz, color in lines:
        center(d, y, text, font(sz), color)
        y += sz + 30
    img.save(path)

make_card(os.path.join(work, "00-title.png"), [
    ("Local Qwen3-30B builds a", 76, "white"),
    ("multi-file Kanban app", 76, "white"),
    ("via OpenCode's tool loop, on my Mac", 42, "#9dd0ff"),
    ("bash + write + read, no cloud, 100% local", 32, "#7aa2c9"),
])

make_card(os.path.join(work, "99-outro.png"), [
    ("What you just saw", 40, "#9dd0ff"),
    ("1 prompt · 6 tool calls · 3 files · 73 seconds", 44, "white"),
    ("rigspark + opencode + ollama", 40, "white"),
    ("github.com/rigspark/rigspark", 34, "#9dd0ff"),
])

# Story frames with captions.  07 is the "money shot" showing all tool cards
# inline in the chat panel, so it gets a longer hold via the runner.
story = [
    ("01-fresh.png",              "opencode + Qwen3-30B, ollama runtime, localhost only"),
    ("02-workspace-attached.png", "Point at a fresh workspace folder"),
    ("03-prompt.png",             "Ask for a multi-file Kanban: bash, write, read"),
    ("04-tools-00.png",           "OpenCode's tool loop starts running locally"),
    ("07-tools-panel.png",        "Every tool call surfaces inline · bash + 3 writes + 2 reads"),
    ("05-response.png",           "Final line arrives: KANBAN_BUILT"),
    ("06-kanban-loaded.png",      "Open the app OpenCode just wrote · 4 cards across 3 columns"),
]

BAR_H = 170
for i, (name, cap) in enumerate(story, start=1):
    src = Image.open(os.path.join(frames_dir, name)).convert("RGB")
    sw, sh = src.size
    # cover-fit into W×H, cropping from the TOP for the tall panel shot
    scale = max(W / sw, H / sh)
    new = src.resize((int(sw*scale+0.5), int(sh*scale+0.5)), Image.LANCZOS)
    nx = (new.size[0] - W) // 2
    if name == "07-tools-panel.png":
        ny = 0  # keep the top so tool cards are visible
    else:
        ny = (new.size[1] - H) // 2
    canvas = new.crop((nx, ny, nx + W, ny + H))
    bar = Image.new("RGBA", (W, BAR_H), (0, 0, 0, 160))
    canvas.paste(bar, (0, H - BAR_H), bar)
    d = ImageDraw.Draw(canvas)
    step = f"{i} / {len(story)}"
    d.text((56, H - BAR_H + 22), step, font=font(30), fill="#7aa2c9")
    d.text((56, H - BAR_H + 72), cap, font=font(40), fill="white")
    canvas.save(os.path.join(work, f"cap-{i:02d}.png"))
print("baked")
PY

encode_clip () {
  local in="$1" out="$2" dur="${3:-$DUR}"
  local z=$(python3 -c "print(int($dur*$FPS))")
  local fout=$(python3 -c "print(f'{$dur-0.35:.2f}')")
  ffmpeg -y -loop 1 -i "$in" -vf "\
zoompan=z='min(zoom+0.0012,1.08)':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=${z}:s=1600x1066:fps=${FPS},\
fade=t=in:st=0:d=0.35,fade=t=out:st=${fout}:d=0.35" \
    -c:v libx264 -pix_fmt yuv420p -r $FPS -t "$dur" "$out" >/dev/null 2>&1
}

encode_clip "$WORK/00-title.png" "$WORK/00-title.mp4" 3.5
# Hold the tool-cards panel frame (cap-05) longer so viewers can read them.
for i in 1 2 3 4 5 6 7; do
  idx=$(printf '%02d' "$i")
  dur=$DUR
  [[ $i -eq 5 ]] && dur=5.0
  [[ $i -eq 7 ]] && dur=3.8
  encode_clip "$WORK/cap-${idx}.png" "$WORK/${idx}-clip.mp4" "$dur"
done
encode_clip "$WORK/99-outro.png" "$WORK/99-outro.mp4" 3.5

list="$WORK/list.txt"
: > "$list"
for f in "$WORK"/00-title.mp4 "$WORK"/[0-9][0-9]-clip.mp4 "$WORK"/99-outro.mp4; do
  echo "file '$f'" >> "$list"
done
OUT_MP4="$OUT_DIR/opencode-qwen3-30b-kanban.mp4"
ffmpeg -y -f concat -safe 0 -i "$list" -c:v libx264 -pix_fmt yuv420p -r $FPS "$OUT_MP4" >/dev/null 2>&1

OUT_GIF="$OUT_DIR/opencode-qwen3-30b-kanban.gif"
ffmpeg -y -i "$OUT_MP4" -vf "fps=10,scale=720:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=96:stats_mode=diff[p];[s1][p]paletteuse=dither=sierra2_4a" -loop 0 "$OUT_GIF" >/dev/null 2>&1

ls -lh "$OUT_MP4" "$OUT_GIF"
