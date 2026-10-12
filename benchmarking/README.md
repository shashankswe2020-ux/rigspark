# Model benchmarks

Keep one folder per catalog model, replacing `:` with `-` in the folder name.
Each folder contains the report, prompts and grading criteria, and recorded
results. Identify the hardware, exact model artifact, runtime revision, local
patches, sampling settings, and generation limits.

These are manually collected benchmark records, not the proposed runtime
benchmark CLI. Results do not automatically update the offline catalog or
throughput dataset. Never commit model weights, compiled runtimes, credentials,
machine-local paths, or unrelated conversation history here.

| Model | Report | Scope |
|---|---|---|
| `bitnet-b1.58:2b` | [BitNet b1.58 2B-4T](bitnet-b1.58-2b/README.md) | 15-case chat smoke test, capped and uncapped |
