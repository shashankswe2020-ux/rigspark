# BitNet b1.58 2B-4T: local chat benchmark

## Result

Both completed runs passed **7/15 strict checks (46.7%)**. Removing the requested
output-token cap allowed two mathematical explanations to finish, but neither
followed the answer-only instruction. A separate **post-hoc manual content
review** of the uncapped run found **11/15 correct answers (73.3%)**, ignoring
extra explanation and Markdown fences.

This is a small, synthetic chat smoke test, **not an IQ score or a standardized
intelligence benchmark**. It does not establish superiority over another model,
production readiness on other machines, or a catalog throughput estimate.

| Setting / outcome | Capped | No requested output cap |
|---|---|---|
| Temperature / seed | 0 / 42 | 0 / 42 |
| Requested output limit | 128 tokens | `max_tokens=-1`, `n_predict=-1` |
| Per-request wall-clock timeout | 120 seconds | 600 seconds |
| Completed requests | 15/15 | 15/15 |
| Responses reaching output cap | 2 | 0 |
| Timeouts | 0 | 0 |
| Strict passes | 7/15 | 7/15 |
| Content-only manual review | Not scored | 11/15 |

The pinned runtime retains a 4096-token training-context safety guard. "Uncapped"
means no requested output-token limit, not bypassing that guard. No uncapped
response reached it; the longest response was 286 generated tokens.

## Evidence and grading

- [cases.json](cases.json): all 15 prompts, multi-turn messages, expected regexes
  and expected JSON.
- [capped.json](capped.json): original 128-token results and response text.
- [uncapped.json](uncapped.json): uncapped results, timings, response text,
  comparison, and the explicitly separate manual content assessment.

The strict rubric was fixed before execution: match the requested answer or JSON
object, obey case-sensitive instructions where specified, and complete without
reaching a generation limit. Explanations fail an answer-only instruction;
Markdown fences fail a JSON-only instruction. There were no per-case retries or
model-based judges. Questions and grading were unchanged between the two runs.

The older server reports `finish_reason=stop` even at the requested token cap.
The capped record therefore also checks the token counts and marks its two
128-token responses as truncated. Adding that annotation changed no scores.

### Effect of removing the cap

| Case | Uncapped result | Generated tokens | Strict outcome |
|---|---|---:|---|
| Pencil word problem | Correct answer: 29, with explanation | 132 | Fail: integer-only instruction |
| Fraction addition | Correct answer: 5/6, with explanation | 286 | Fail: fraction-only instruction |

Every other response was unchanged. Four substantive errors remained:

| Case | Expected | Actual |
|---|---|---|
| Transitive syllogism | Yes | No |
| Python list aliasing | 2 | 3 |
| Rust inclusive range sum | 10 | 5 |
| Tracking the moved key | green drawer | Red box |

Multiplication and JSON extraction were also correct in content but failed their
format requirements. The four format-only failures explain the difference
between the strict score and the post-hoc content review.

## Environment and provenance

- Run date: 2026-10-12.
- Hardware: Apple M4 Max, arm64 macOS, 38,654,705,664 bytes RAM (36 GiB).
- Build tools: Apple clang 21.0.0, CMake 3.31.10.
- RigSpark: 3.0.2, BitNet integration based on commit `21e5464`.
- Catalog ID: `bitnet-b1.58:2b`, 2.4B parameters, MIT license.
- Artifact repository: `microsoft/bitnet-b1.58-2B-4T-gguf`.
- Artifact revision: `a1f2f1c765812aa8af3f6eda4a313707064bba15`.
- File: `ggml-model-i2_s.gguf`, 1,187,801,280 bytes.
- Artifact SHA-256:
  `4221b252fdd5fd25e15847adfeb5ee88886506ba50b8a34548374492884c2162`.
- BitNet revision: `01eb415772c342d9f20dc42772f1583ae1e5b102`.
- llama.cpp submodule: `1f86f058de0c3f4098dedae2ae8653c335c868a1`.
- Patched binary SHA-256 on the test machine:
  `205b44b13cd876230d664bf1146806f02c65df004a539e516ad8a38cc33cef77`.

The binary is **not pristine upstream**. It includes the
[ARM I2_S patch](../../scripts/bitnet-arm-i2s.patch). Static linking was enabled;
BLAS, Metal, OpenMP, and TL1 were disabled. See the
[tested build instructions](../../docs/references/guide.md#tested-apple-silicon-build).
The binary hash identifies the tested build, not a portable reproducible-build
guarantee.

## Failures resolved before the completed runs

1. Unmodified ARM kernels decoded the wrong packed weight layout and mishandled
   activation/output strides. Regression tests failed before the repair and
   passed afterward for batch widths 1, 4, 8, 16, 32, 48, 64 and inner dimensions
   128, 256, 2560, 6912.
2. An initial benchmark attempt crashed on a longer prompt in BLAS I2_S
   dequantization. Disabling BLAS kept execution on the tested CPU kernels. The
   full suite was then restarted unchanged; that aborted attempt is not scored.
3. The newer diagnostic runtime also had a wrong squared-ReLU selection and an
   obsolete embedded chat template. It was not used for the completed runs.
   The selected older runtime uses its built-in BitNet formatting with
   `User`/`Assistant` roles and `<|eot_id|>` boundaries.

The official weights were unchanged and independently rehashed. Inspection
found nonzero packed data and nonzero scales in every I2_S tensor; an upstream
report of all-zero tensors did not reproduce for this pinned artifact.

## Reproduction and lifecycle

Build the pinned, patched runtime, keep `RIGSPARK_BITNET_SERVER` set, and use an
isolated `RIGSPARK_HOME`. Start the catalog model through `rigspark up` on a free
loopback port with context 4096. Do not replace an existing user session.

Send each case's `messages` to `/v1/chat/completions` sequentially, using model
`bitnet-b1.58:2b`, temperature 0, seed 42, `stream=false`, and
`cache_prompt=false`. For capped requests set `max_tokens=128`; for uncapped
requests set both `max_tokens=-1` and `n_predict=-1`. Apply the recorded timeout
and rubric. Stop the owned process through `rigspark down`, including on error.

These measurements used the real production-started server with verified cached
weights, not test mocks. Production `rigspark chat` separately returned `4` for
2 + 2 and `29` for the previously crashing long word problem. All benchmark
servers were stopped, ports released, and isolated active state cleared.
The existing Ollama daemon and configured Qwen session were preserved.

## Sources

- [Pinned official model card](https://huggingface.co/microsoft/bitnet-b1.58-2B-4T-gguf/blob/a1f2f1c765812aa8af3f6eda4a313707064bba15/README.md)
- [Pinned official artifact metadata](https://huggingface.co/api/models/microsoft/bitnet-b1.58-2B-4T-gguf/revision/a1f2f1c765812aa8af3f6eda4a313707064bba15?blobs=true)
- [Pinned runtime source](https://github.com/microsoft/BitNet/tree/01eb415772c342d9f20dc42772f1583ae1e5b102)
- [ARM layout failure report: microsoft/BitNet#585](https://github.com/microsoft/BitNet/issues/585)
- [Scalar fallback report: microsoft/BitNet#600](https://github.com/microsoft/BitNet/issues/600)
