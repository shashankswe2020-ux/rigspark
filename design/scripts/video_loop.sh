#!/usr/bin/env bash
# Turn the Runway clip into a seamless ping-pong hero loop (H.264 faststart + VP9 + poster).
# usage: design/scripts/video_loop.sh raw.mp4 site/brand/video
set -euo pipefail
RAW=$1; OUT=${2:-site/brand/video}
ffmpeg -y -i "$RAW" -filter_complex "[0:v]trim=start_frame=2,setpts=PTS-STARTPTS,split[a][b];[b]reverse[r];[a][r]concat=n=2:v=1,fps=24,format=yuv420p[v]" \
  -map "[v]" -c:v libx264 -crf 22 -preset slow -movflags +faststart -an "$OUT/sparky-loop.mp4"
ffmpeg -y -i "$OUT/sparky-loop.mp4" -c:v libvpx-vp9 -b:v 0 -crf 36 -row-mt 1 -an "$OUT/sparky-loop.webm"
ffmpeg -y -i "$RAW" -vf "select=eq(n\,2)" -frames:v 1 -q:v 3 "$OUT/sparky-loop-poster.jpg"
