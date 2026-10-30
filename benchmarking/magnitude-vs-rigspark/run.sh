#!/usr/bin/env bash
# RigSpark vs Magnitude speed test on this Mac.
# Same model family and 4-bit quantization on both sides, run one at a time so
# they never compete for GPU memory. Override with env vars:
#   RS_MODEL=qwen3.5:4b MG_MODEL=qwen3.5-4b RUNS=5 ./run.sh
set -euo pipefail
cd "$(dirname "$0")"

RS_MODEL="${RS_MODEL:-qwen3.5:9b}"
MG_MODEL="${MG_MODEL:-qwen3.5-9b}"
RUNS="${RUNS:-3}"
RS_URL="http://127.0.0.1:11434/v1"
MG_URL="http://127.0.0.1:10100/inference/v1"
mkdir -p results
log() { printf '\n==> %s\n' "$*"; }

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing: $1 ($2)"; exit 1; }; }
need python3 "install Xcode command line tools"
need rigspark "brew install shashankswe2020-ux/tap/rigspark"
if ! command -v magnitude >/dev/null 2>&1; then
  [ -x "$HOME/.magnitude/bin/magnitude" ] && export PATH="$HOME/.magnitude/bin:$PATH"
fi
need magnitude "curl -fsSL https://magnitude.dev/install.sh | sh"

{ echo "date: $(date -u +%FT%TZ)"; sysctl -n machdep.cpu.brand_string;
  echo "memory_gb: $(( $(sysctl -n hw.memsize) / 1073741824 ))"; sw_vers -productVersion;
  rigspark --version; ollama --version 2>/dev/null || true; magnitude --version; } > results/machine.txt
cat results/machine.txt

# ---------- RigSpark (Ollama backend) ----------
log "Stopping Magnitude's model so it doesn't hold GPU memory"
magnitude models stop >/dev/null 2>&1 || true

log "RigSpark: rigspark up $RS_MODEL (downloads on first run)"
rigspark up "$RS_MODEL"
python3 bench.py --name rigspark --base-url "$RS_URL" --model "$RS_MODEL" --runs "$RUNS" --out results/rigspark.json
log "RigSpark: shutting down"
rigspark down || true
sleep 3

# ---------- Magnitude ----------
SERVE_PID=""
if ! curl -sf "$MG_URL/models" >/dev/null 2>&1; then
  log "Starting magnitude serve in the background"
  magnitude serve > results/magnitude-serve.log 2>&1 &
  SERVE_PID=$!
  for _ in $(seq 1 60); do curl -sf "$MG_URL/models" >/dev/null 2>&1 && break; sleep 2; done
fi
trap '[ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null || true' EXIT

log "Magnitude: pulling $MG_MODEL (downloads and tunes kernels on first run)"
magnitude catalog pull "$MG_MODEL" || true
MG_ID=""
for _ in $(seq 1 720); do
  MG_ID=$(curl -sf "$MG_URL/models" | python3 -c "
import json,sys
ids=[m['id'] for m in json.load(sys.stdin).get('data',[])]
hit=[i for i in ids if i.startswith('$MG_MODEL')]
print(hit[0] if hit else '')" 2>/dev/null || true)
  [ -n "$MG_ID" ] && break
  sleep 5
done
[ -n "$MG_ID" ] || { echo "Magnitude model never became available; check: magnitude models status"; exit 1; }
echo "Magnitude model id: $MG_ID"
python3 bench.py --name magnitude --base-url "$MG_URL" --model "$MG_ID" --runs "$RUNS" --out results/magnitude.json
magnitude models stop >/dev/null 2>&1 || true

log "Result"
python3 compare.py results
