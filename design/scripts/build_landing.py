"""Build design/redesign/landing.html from landing.src.html: inline icons + data rows.
Icons are inlined (CSS mask/url() fails under file://)."""
import json, re, pathlib
K = pathlib.Path(__file__).resolve().parents[2]
src = (K / "design/redesign/landing.src.html").read_text()
icons = K / "site/brand/icons"

def icon(name):
    s = (icons / f"{name}.svg").read_text().strip()
    s = re.sub(r' width="24" height="24"', '', s)
    return f'<span class="i" aria-hidden="true">{s}</span>'

# Q4_K_M estimates as published on rigspark.si (FAQ + real recommend output)
MODELS = [
    dict(id="llama3.1:8b", label="Llama 3.1 8B", mem=5.3, params=8, dense=True),
    dict(id="qwen3:14b", label="Qwen3 14B", mem=9.9, params=14, dense=True),
    dict(id="gemma3:27b", label="Gemma 3 27B", mem=18.6, params=27, dense=True),
    dict(id="qwen3:30b-a3b", label="Qwen3 30B-A3B (MoE)", mem=19.9, params=30, dense=False),
    dict(id="qwen3:32b", label="Qwen3 32B", mem=21.6, params=32, dense=True),
    dict(id="llama3.3:70b", label="Llama 3.3 70B", mem=45.5, params=70, dense=True),
]
RAM = [8, 16, 24, 32, 48, 64, 96, 128]
RUNTIMES = [("layers", "Ollama"), ("layers", "llama.cpp"), ("chip", "MLX on Apple Silicon"), ("layers", "LM Studio"),
            ("image", "ComfyUI"), ("plug", "MCP servers"), ("terminal", "macOS · Linux · Windows")]
COMPARE = ["Run inference", "Hardware-aware recommendations", "yes / slow / no verdicts + est. tok/s",
           "AI Hardware Score (0–100)", "KV-cache-aware context sizing", "Interactive terminal UI",
           "Loopback-only browser workspace", "Agents, skills &amp; MCP tools", "Multi-backend (4 runtimes)",
           "SHA-256 integrity verification", "Offline, deterministic advice"]

rep = {
    "{{marquee}}": "".join(f"<span>{icon(i)}{t}</span>" for i, t in RUNTIMES),
    "{{ram}}": "".join(f'<button data-v="{r}" aria-pressed="false">{r} GB</button>' for r in RAM),
    "{{models}}": "".join(f'<button data-v="{m["id"]}" aria-pressed="false">{m["label"]}</button>' for m in MODELS),
    "{{faces_json}}": json.dumps({k: (K / f"site/brand/illustrations/sparky-{k}.svg").read_text().strip() for k in ("yes", "slow", "no")}),
    "{{model_json}}": json.dumps([{k: m[k] for k in ("id", "mem", "params", "dense")} for m in MODELS]),
    "{{cmp_them}}": "".join(f'<li class="{"" if i == 0 else "dim"}"><span class="m {"y" if i == 0 else "n"}">{"✓" if i == 0 else "–"}</span>{c}</li>' for i, c in enumerate(COMPARE)),
    "{{cmp_us}}": "".join(f'<li><span class="m y">✓</span>{c}</li>' for c in COMPARE),
    "{{compare}}": "".join(
        f'<tr><td>{c}</td><td class="{"y" if i == 0 else "n"}">{"✓" if i == 0 else "–"}</td><td class="us y">✓</td></tr>'
        for i, c in enumerate(COMPARE)),
}
out = src
for k, v in rep.items():
    out = out.replace(k, v)
out = re.sub(r"\{\{i:([a-z\-]+)\}\}", lambda m: icon(m.group(1)), out)
assert "{{" not in out, re.findall(r"\{\{[^}]+\}\}", out)
(K / "design/redesign/landing.html").write_text(out)
print("landing.html", len(out) // 1024, "KB")
