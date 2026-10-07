# Spec: Automatic catalog admission of the latest local models

Status: draft for review · Owner: catalog pipeline · Related: #254 (coverage gap), #278 (stalled refresh PR)

## Objective

Users want to try the newest local models as soon as Ollama publishes them. Today the
catalog can only grow by hand:

- `catalog-refresh` replays a static registry snapshot compiled into the binary.
- `catalog-enrich` refreshes sizes and digests of existing entries and never adds models.
- `catalog-propose` and `catalog-coverage` only file proposals and issues. 50 of 77
  monitored Ollama repositories are missing (#254), and the weekly run pauses while an
  auto-refresh PR waits for review (#278 has been open since 2026-10-05).
- Signed snapshot publication needs at least 90% fresh, cited evidence. Only 2 of 69
  entries have it (2.9%), so `catalog --update` has never shipped new models.

**Goal:** the weekly pipeline discovers new models on ollama.com (newest first) and admits
every variant whose facts it can source exactly. It merges the change and publishes a
signed catalog snapshot without a human in the loop. Users get the models through
`rigspark catalog --update` with no app release, and `rigspark up <model>` works
immediately.

**Decisions already made (2026-10-07):**

1. Fully automatic: admit, auto-merge, auto-publish a signed snapshot.
2. Discovery source: `https://ollama.com/library?sort=newest` plus the existing monitored
   inventory.
3. Catalog schema change approved: add `provenance` and allow `releaseDate`,
   `benchmarkProxy` and `kvBytesPerToken` to be absent on auto entries. Bump
   `schemaVersion` to 3 with a reader that also accepts version 2.

### User stories

- As a user, `rigspark catalog --update` gives me models Ollama released this week, with
  verdicts, and they are labelled **auto-sourced**.
- As a user, `rigspark up qwen3.8:27b` pulls weights that are verified against the
  digest the pipeline recorded, and refuses anything else.
- As a maintainer, I do nothing for a routine week. I'm only alerted when sourcing fails
  or facts contradict the catalog.

## Field sourcing (per admitted variant)

Every field is read from an upstream artifact or left absent. Nothing is estimated.

| Catalog field | Source | If it can't be sourced |
|---|---|---|
| `id`, `source.ollama` | tag from `ollama.com/library/{repo}/tags` | skip variant |
| `quantizations[0].diskBytes`, `sha256`, `projectors` | registry manifest (existing `parse_layer`) | skip variant |
| `quantizations[0].name` | config blob `file_type` | skip variant |
| `family` | config blob `model_family` | skip variant |
| `params` | GGUF `general.size_label`, else config `model_type`, normalised to `^\d+(\.\d+)?[BMT]$` | skip variant |
| `architecture`, `activeParams` | GGUF `{arch}.expert_count > 0` → `moe`; active from the size label `…-A3B`, else from tensor shapes | MoE with no sourced active params → skip |
| `contextLength` | GGUF `{arch}.context_length` | skip variant |
| `license`, `openWeight` | GGUF `general.license` mapped to the `LICENSES` allow-list, cross-checked against a fingerprint of the license blob | unknown or non-open → skip (reported) |
| `capabilities` | library chips: `tools`→tools, `vision`→vision, `thinking`→reasoning, `embedding`→embedding; `chat` unless embedding-only | never inferred from names |
| `kvBytesPerToken` | `block_count × head_count_kv × (key_length + value_length) × 2`, only for plain attention (no `ssm.*`, `full_attention_interval`, sliding-window or MLA keys) | absent (honesty gate) |
| `releaseDate` | Hugging Face `createdAt` of `general.base_model.0.repo_url` when it is a huggingface.co URL | absent |
| `benchmarkProxy` | not sourced | absent; ranking already falls back to weights |
| `provenance` | `"auto"` | — |
| `addedAt` | date of first admission by the pipeline (never changes) | — |

**Variant selection.** For each repository, the pipeline admits the plain size tags
(`^\d+(\.\d+)?[bm](-a\d+(\.\d+)?b)?$`, for example `27b` or `30b-a3b`). These are Ollama's
default quantizations. Tags are deduplicated by model-layer digest. Cloud-only models (a
`cloud` chip or `-cloud` tags), non-GGUF manifests (`format != gguf`, such as mlx, nvfp4
or mxfp8) and repositories outside `library/` are excluded.

**Budget.** Each run admits at most 25 repositories and 150 variants, newest first.
A per-run cursor resumes where the last run stopped. Every network read has a size cap: a
4 MiB manifest, 64 KiB config and license blobs, a 64 MiB streamed GGUF header and
2 MiB HTML pages.

## Trust and safety rules

- **Integrity, fail-closed:** auto entries always carry the model-layer `sha256` and size.
  `up` and `switch` verify against them exactly as for curated entries.
- **Curated wins:** auto admission never modifies or replaces a `provenance: curated`
  entry. When a curated entry exists for the same id, the auto variant is skipped.
- **Removal:** if a later run finds an auto entry's tag gone, its license changed to a
  non-open one, or its digest changed, the entry is updated or removed. That only
  happens to auto entries.
- **Hosts:** reads go only to `ollama.com`, `registry.ollama.ai`, the registry's blob
  redirect host (`*.r2.cloudflarestorage.com`, HTTPS only, used solely for blob range
  reads) and `huggingface.co/api/models`. No other redirects are followed.
- **Determinism and offline advice are unchanged:** the network is used only by the
  maintenance pipeline. User-facing advice reads the bundled or signed catalog.
- **Labelling:** the CLI (`recommend`, `can-run`, `catalog`), the TUI and the GUI show
  `auto-sourced` for `provenance: auto`. Absent fields render as `unknown`.

## Evidence and publication

Each auto entry gets a quality observation, built from the same reads, whose facts equal
the entry. It cites the manifest URL `https://registry.ollama.ai/v2/library/{repo}/manifests/{tag}`
and the page `https://ollama.com/library/{repo}:{tag}`, which already satisfies
`citations_match`. Each run also regenerates the `ollama-local-variants` scope from the
crawl, with `complete: true` only when the crawl finished within budget. Because evidence
is regenerated weekly, the 7-day freshness rule holds for auto entries.

## Tech stack

Rust 1.98.1, edition 2024. Existing crates and dependencies only: `reqwest`, `serde`,
`regex`, `time`, `sha2`, `tokio`. No new runtime dependencies, and no Node or Python
in the pipeline.

## Commands

```bash
cargo catalog-admit --dry-run                       # discover + source, print the diff, write nothing
cargo catalog-admit --fixture <recorded.json>       # offline replay for tests and debugging
cargo catalog-admit                                 # write models.json + quality evidence
cargo test -p rigspark-core --test catalog_admission
cargo test -p rigspark-runtime --test catalog_admission
cargo test --workspace --locked -- --test-threads=2
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo native-retirement
```

## Project structure

```
crates/rigspark-core/src/admission.rs        pure: library/tags HTML parsing, tag selection, GGUF metadata
                                              parsing, license mapping, KV geometry, entry + observation build
crates/rigspark-core/src/catalog.rs          schema v3: `provenance`, optional `releaseDate`, v2-compatible reader
crates/rigspark-runtime/src/admission.rs     transport trait (ollama.com, registry, blob range, HF), budgets, cursor
crates/rigspark-cli/src/catalog_admit.rs     `llmup-catalog-admit` binary
crates/rigspark-core/tests/catalog_admission.rs
crates/rigspark-runtime/tests/catalog_admission.rs + fixtures/admission/*.json (recorded responses)
.github/workflows/catalog-refresh.yml        run admit, open PR, enable auto-merge, dispatch publish
.github/workflows/catalog-publish.yml        runnable from the refresh workflow once gates pass
```

## Code style

This follows the existing collector: a pure parser in core and a transport trait in runtime.

```rust
pub fn kv_bytes_per_token(meta: &GgufMetadata) -> Option<f64> {
    if meta.has_any(&["ssm.", "full_attention_interval", "attention.sliding_window", "attention.kv_lora_rank"]) {
        return None; // non-uniform attention: the standard formula would fabricate a number
    }
    let per_layer = meta.u64("attention.head_count_kv")? * (meta.u64("attention.key_length")? + meta.u64("attention.value_length")?);
    Some((meta.u64("block_count")? * per_layer * 2) as f64)
}
```

## Testing strategy

- **Unit tests (core):** HTML parsing against real recorded pages; tag selection; GGUF
  parsing (truncated, oversized and malicious arrays); license mapping and fingerprints;
  KV rules, including the hybrid case returning `None`; observations equal entries; v2
  and v3 schema round-trips.
- **Integration tests (runtime):** the full admission run over a recorded transport.
  Covers the budget and cursor, curated entries left untouched, auto entries updated and
  removed, redirect host enforcement and cancellation. No live network in tests.
- **Gate tests:** a catalog with only auto entries plus generated evidence passes the
  correctness and coverage checks; a contradiction is still a blocker.
- **Regression:** existing `catalog`, `catalog_quality`, `catalog_updates`, sizing and
  ranking suites stay green.

## Boundaries

- **Always:** source or omit; fail closed on digests; keep curated entries untouched; cap
  every read; record fixtures for tests.
- **Ask first:** repository settings (ruleset bypass, signing environment reviewers); any
  new dependency; widening the host allow-list.
- **Never:** fabricate `benchmarkProxy`, `releaseDate`, KV or active params; admit
  non-open licenses; follow arbitrary redirects; hit the network in tests.

## Success criteria

1. A replay of today's recorded upstream data admits at least the 2026 releases visible
   in the newest list that are local and GGUF and have an open license (for example
   `qwen3.8:27b`), with every field matching the sourcing table above.
2. Every admitted entry has a digest, and `up` verification passes against it in the
   existing lifecycle tests.
3. The hybrid-attention `qwen3.8` has `kvBytesPerToken` absent. A plain-attention fixture
   gets the exact formula value.
4. Generated evidence makes every auto entry `verified` in `catalog-quality`.
5. A weekly run with no upstream changes produces no diff and no PR.
6. A v2 catalog still parses. A v3 catalog with `provenance: auto` and no `releaseDate`
   parses, and older clients reject it cleanly through the existing signed-update fallback.
7. The CLI, TUI and GUI show `auto-sourced`, and `unknown` for absent fields.
8. fmt, clippy (`-D warnings`), the full test suite, the build and `native-retirement` all pass.

## Decisions on the former open questions (2026-10-07)

1. **Auto-merge:** the workflow merges with `gh pr merge --admin --squash` using a
   maintainer-provided fine-grained PAT, the `CATALOG_MERGE_TOKEN` secret (this repo only,
   contents and pull-requests write). It merges only after required CI passes and only
   when the PR diff touches exactly `crates/rigspark-core/data/models.json`,
   `docs/references/catalog-quality-evidence.json`,
   `docs/references/catalog-quality-latest-report.json` and
   `docs/references/catalog-proposals.json`. Anything else leaves the PR for review.
2. **Gate policy, option (a):** each run also re-observes curated Ollama entries from the
   same sources. Facts the pipeline can't source for a curated entry (for example
   `benchmarkProxy`) are excluded from comparison, exactly as `matches_facts` already
   does for `kvBytesPerToken` and `benchmarkProxy`. Any mismatch in a sourced fact is a
   blocker, reported in the PR and in an issue.
3. **Signing:** the required reviewer on the `catalog-signing` environment is removed
   once the workflow lands. The quality gate (at least 90% fresh and verified, at least
   90% coverage, no blockers) is the only guard. The branch policy (main only) stays.

## Recency filter (`--month N`)

`recommend`, `catalog`, the TUI and the GUI accept a recency window of 1, 2 or 3 months
(TUI: `--month N` plus a keyboard toggle; GUI: a "Released in" selector with All, 1, 2
and 3 months). An entry's recency date is `releaseDate` when sourced. Otherwise it is
`addedAt`, a new optional `YYYY-MM-DD` field that the pipeline sets on the day it first
admits an auto entry and never changes after that. The UI says which date was used
("released" or "added"). Filtering uses the local clock only. Advice stays offline and
deterministic for a given date.

## Plan and tasks

1. **Schema v3** (`catalog.rs`, `enrich.rs`, `bootstrap.rs`, data, tests). Add
   `provenance`, optional `releaseDate` (auto entries only) and optional `addedAt`; the
   reader accepts v2 and v3.
   Verify: core catalog tests and v2/v3 round-trip tests.
2. **Pure parsers** (`core/admission.rs`). Library and tags HTML, tag selection,
   streaming GGUF metadata, license mapping, KV rule, MoE active params.
   Verify: unit tests on recorded fixtures.
3. **Transport and run** (`runtime/admission.rs`). Allow-listed hosts, blob range reads
   with the redirect rule, budgets, cursor and cancellation.
   Verify: integration tests on a recorded transport.
4. **Entry and evidence build, plus merge rules.** Curated wins; auto entries are
   updated or removed; observations equal the entries.
   Verify: gate tests.
5. **CLI binary `llmup-catalog-admit`** with `--dry-run` and `--fixture`.
   Verify: CLI tests and a dry run against live upstream (manual).
6. **Curated re-observation (gate option a).** Re-read curated Ollama entries weekly
   and emit evidence for sourced facts; a mismatch is a blocker.
   Verify: gate tests.
7. **Workflow.** Admit, open the PR, admin-merge with `CATALOG_MERGE_TOKEN` after CI,
   dispatch publish; remove the required reviewer on `catalog-signing`.
   Verify: workflow policy tests in `shipping_policy.rs`.
8. **Surfaces.** The `auto-sourced` label in the CLI, TUI and GUI, plus docs: README and
   `docs/references/catalog-enrichment.md`.
   Verify: report and TUI snapshot tests.
9. **Recency filter.** `--month 1|2|3` for `recommend`, `catalog` and the TUI (with a
   toggle key), and the GUI selector; labelled as released or added.
   Verify: core filter unit tests, CLI tests, TUI snapshot and GUI WebDriver journey.
