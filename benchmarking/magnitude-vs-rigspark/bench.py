#!/usr/bin/env python3
"""Measure local inference speed through an OpenAI-compatible streaming endpoint.

Both engines are measured by the same client code, so request overhead is the
same on both sides. Standard library only.

Usage:
  bench.py --name rigspark --base-url http://127.0.0.1:11434/v1 --model qwen3.5:9b --out results/rigspark.json
"""
import argparse
import json
import random
import statistics
import string
import sys
import time
import urllib.request

SHORT_PROMPT = (
    "Write a detailed technical explanation of how a hash map handles collisions, "
    "covering chaining, open addressing, load factor and resizing. Use full sentences."
)

FILLER = (
    "The quarterly operations log records shipments, inventory counts, supplier "
    "delays, maintenance windows and staffing notes for each warehouse region. "
)


def long_prompt(target_words: int) -> str:
    # A random nonce at the very start defeats prefix caches, so every run
    # pays for the full prompt.
    nonce = "".join(random.choices(string.ascii_lowercase, k=12))
    body = (FILLER * (target_words // len(FILLER.split()) + 1)).split()[:target_words]
    return f"Log id {nonce}.\n" + " ".join(body) + "\n\nSummarise the log above in two sentences."


def run_once(base_url, model, prompt, max_tokens, api_key, timeout):
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": 0,
        "stream": True,
        "stream_options": {"include_usage": True},
    }
    req = urllib.request.Request(
        base_url.rstrip("/") + "/chat/completions",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {api_key}"},
    )
    t0 = time.perf_counter()
    t_first = None
    t_last = None
    chunks = 0
    usage = None
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        for raw in resp:
            line = raw.decode("utf-8", "replace").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            try:
                obj = json.loads(data)
            except json.JSONDecodeError:
                continue
            if obj.get("usage"):
                usage = obj["usage"]
            for choice in obj.get("choices") or []:
                delta = choice.get("delta") or {}
                text = (delta.get("content") or "") + (delta.get("reasoning_content") or "") + (
                    delta.get("reasoning") or ""
                )
                if text:
                    now = time.perf_counter()
                    if t_first is None:
                        t_first = now
                    t_last = now
                    chunks += 1
    t_end = time.perf_counter()
    if t_first is None:
        raise RuntimeError("no tokens streamed back")
    completion = (usage or {}).get("completion_tokens") or chunks
    prompt_tokens = (usage or {}).get("prompt_tokens")
    ttft = t_first - t0
    gen_window = (t_last or t_end) - t_first
    decode_tps = (completion - 1) / gen_window if gen_window > 0 and completion > 1 else None
    return {
        "ttft_s": ttft,
        "total_s": t_end - t0,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion,
        "usage_reported": usage is not None,
        "decode_tps": decode_tps,
        "prefill_tps": (prompt_tokens / ttft) if prompt_tokens and ttft > 0 else None,
    }


def med(rows, key):
    vals = [r[key] for r in rows if r.get(key) is not None]
    return statistics.median(vals) if vals else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--model", required=True)
    ap.add_argument("--api-key", default="local")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--gen-tokens", type=int, default=256)
    ap.add_argument("--long-words", type=int, default=1500)
    ap.add_argument("--timeout", type=float, default=900)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    print(f"[{a.name}] warm-up (loads the model; not counted)...", flush=True)
    run_once(a.base_url, a.model, "Say hi.", 8, a.api_key, a.timeout)

    workloads = {
        "decode": lambda: (SHORT_PROMPT, a.gen_tokens),
        "prefill": lambda: (long_prompt(a.long_words), 32),
    }
    results = {"name": a.name, "model": a.model, "base_url": a.base_url, "runs": {}}
    for wl, make in workloads.items():
        rows = []
        for i in range(a.runs):
            prompt, n = make()
            r = run_once(a.base_url, a.model, prompt, n, a.api_key, a.timeout)
            rows.append(r)
            print(
                f"[{a.name}] {wl} run {i + 1}/{a.runs}: "
                f"ttft {r['ttft_s']:.2f}s, decode {r['decode_tps'] or 0:.1f} tok/s, "
                f"prefill {r['prefill_tps'] or 0:.0f} tok/s, out {r['completion_tokens']} tok",
                flush=True,
            )
        results["runs"][wl] = rows
    results["summary"] = {
        "decode_tps": med(results["runs"]["decode"], "decode_tps"),
        "short_ttft_s": med(results["runs"]["decode"], "ttft_s"),
        "prefill_tps": med(results["runs"]["prefill"], "prefill_tps"),
        "long_ttft_s": med(results["runs"]["prefill"], "ttft_s"),
        "long_prompt_tokens": med(results["runs"]["prefill"], "prompt_tokens"),
    }
    with open(a.out, "w") as f:
        json.dump(results, f, indent=2)
    print(f"[{a.name}] summary: {json.dumps(results['summary'])}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # noqa: BLE001
        print(f"benchmark failed: {e}", file=sys.stderr)
        sys.exit(1)
