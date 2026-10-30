#!/usr/bin/env python3
"""Print a side-by-side table from results/*.json."""
import json
import pathlib
import sys

d = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "results")
rs = json.loads((d / "rigspark.json").read_text())["summary"]
mg = json.loads((d / "magnitude.json").read_text())["summary"]

rows = [
    ("Generation speed (tok/s)", "decode_tps", True),
    ("Prompt processing (tok/s)", "prefill_tps", True),
    ("Time to first token, short prompt (s)", "short_ttft_s", False),
    ("Time to first token, long prompt (s)", "long_ttft_s", False),
]
lines = ["| Metric | RigSpark | Magnitude | Winner |", "|---|---:|---:|---|"]
for label, key, higher_better in rows:
    a, b = rs.get(key), mg.get(key)
    if a is None or b is None:
        lines.append(f"| {label} | {a} | {b} | n/a |")
        continue
    if higher_better:
        ratio = a / b
    else:
        ratio = b / a
    who = "RigSpark" if ratio > 1 else "Magnitude"
    x = ratio if ratio > 1 else 1 / ratio
    lines.append(f"| {label} | {a:.2f} | {b:.2f} | {who} ({x:.2f}x) |")
lines.append(f"\nLong prompt length: ~{int(rs.get('long_prompt_tokens') or 0)} tokens (RigSpark), "
             f"~{int(mg.get('long_prompt_tokens') or 0)} tokens (Magnitude)")
out = "\n".join(lines)
print(out)
(d / "comparison.md").write_text(out + "\n")
