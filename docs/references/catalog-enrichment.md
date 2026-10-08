# Catalog Enrichment Workflow

## Automatic admission (weekly)

The `admit` job in the Catalog Freshness workflow keeps the catalog current
without a human in the loop. Spec: `docs/specs/catalog-auto-admission.md`.

1. `cargo catalog-enrich` refreshes digests of existing entries.
2. `cargo catalog-admit --correct-curated` crawls `ollama.com/library?sort=newest`,
   reads each variant's manifest, config, license and GGUF header, and admits it as
   `provenance: auto` only when every fact is sourced. It writes a cited
   observation per entry plus the coverage scope, and re-observes curated entries:
   contradicted facts are corrected and recorded in
   `docs/references/catalog-corrections.json`, and architecture contradictions stay
   blockers. Rejections are cached by digest in
   `docs/references/catalog-admission-state.json`.
3. `cargo generation-admit` admits Comfy-Org text-to-image and text-to-video
   releases with open licenses as fit-only entries. Entries whose files match an
   official ComfyUI example (Wan 2.2 TI2V-5B, Qwen-Image) are runnable.
4. `cargo catalog-site` regenerates `site/data/latest.js` for Ask Sparky.
5. The full test suite and `cargo catalog-quality` run. A PR is opened only if every
   changed path is on the data allow-list. With `CATALOG_MERGE_TOKEN` configured,
   it is admin-merged after the required checks pass, and `catalog-publish.yml` is
   dispatched when the quality gate passed. Without the token, the PR waits for
   review.

Tests use `crates/rigspark-core/fixtures/catalog-baseline.json`, so admission
never moves goldens. The shipped catalog keeps invariants instead: 69 curated
entries, bootstrap fidelity except recorded corrections, and reproducible proxies.

Maintainer setup: add a fine-grained `CATALOG_MERGE_TOKEN` (this repository only;
contents and pull requests: read and write) and remove the required reviewer from
the `catalog-signing` environment.

## Proposals and review

The Catalog Freshness workflow runs Mondays at 03:17 UTC and can be dispatched
manually on `main`. It refreshes existing catalog metadata, collects source-backed
candidate proposals, reports release quality, and opens a review PR. It does not
merge, sign, or publish a catalog.

Before building or making paid requests, the workflow checks for an open
`catalog/auto-refresh-*` PR. If one exists, collection pauses and the job summary
links to it. Existing review work is never automatically closed or deleted.

## Frequent Runs

1. Review, merge, or explicitly close the previous automated PR. A blocked quality
  report can be merged as remediation history, but catalog data changes still
  require review and passing CI. Merging never authorizes publication.
2. For a smoke check, dispatch with `candidate_limit=1` and `use_ai=false` (the
  manual defaults). Literal source observations require no paid API call.
3. To check the OpenAI integration, explicitly enable `use_ai` for one candidate.
  Inspect `extractionStatus` and `extractionError`, not just job completion.
4. The next run resumes from `nextAfter` in the last merged proposal report. An
  explicit `after` overrides the cursor; `restart=true` starts from the beginning.
  Scheduled runs keep the 10-candidate budget and optional configured AI.
5. Read the job summary/PR body for source and extraction diagnostics, scores,
  hard blockers, missing variants, and entries needing evidence or correction.

Reports contain only the current batch; previous batches remain in git history.
Closing a PR without merging does not advance the stored cursor. An exhausted
cursor starts a new sweep next time, including newly discovered earlier names.
Failed candidates are not automatically retried; use `after` or `restart` to
revisit them deliberately. A sweep still does not enumerate all variants.

## Optional OpenAI Setup

In GitHub Settings > Secrets and variables > Actions, configure:

- Repository secret `OPENAI_API_KEY`: enter the key directly in GitHub, never in
  chat, a committed file, or a workflow command argument.
- Repository variable `OPENAI_CATALOG_MODEL`: an explicitly selected model that
  supports the Responses API and strict structured outputs. There is no default
  model. Prefer a pinned model version and configure provider-side spending alerts.

If either setting is absent or blank, registry collection still runs and AI
extraction is disabled. The secret is exposed only to the already-built collector
step, not builds or tests. The signing environment and its key remain separate.
GitHub Actions must be allowed to create pull requests in repository settings.

Each run selects at most 10 candidates, makes at most one OpenAI request per
candidate, and permits at most 2,000 output tokens per request (including reasoning
tokens). There are no application or HTTP-client retries. A candidate config is
limited to 64 KiB; requests have a 10-second connection and 60-second total timeout.
These are request/token bounds, not a dollar guarantee; cost depends on the chosen
model and input tokens. Re-running a workflow can incur a new set of charges.

## Sources and Trust

Discovery uses Ollama's public integration inventory and excludes repositories
already represented in the curated catalog. It examines up to 10 missing
repository names alphabetically after the selected cursor, deduplicated, fetching
their `latest` manifests. The report includes `nextAfter` and `remainingCandidates`.
It does not enumerate tags within known repositories or certify complete variant
coverage. Unavailable or cloud-only manifests are reported without inventing pins.

Only manifests and small config blobs are downloaded, never model weights.
Config bytes must match the manifest's exact size and SHA-256 before extraction.
Registry GET redirects are limited to three and restricted to HTTPS on
`registry.ollama.ai` and the observed Ollama storage account
`dd20bb891979d25aebc8bec07b2b3bbc.r2.cloudflarestorage.com`. Storage-host changes
fail closed and require a reviewed allowlist change. OpenAI requests cannot redirect.

OpenAI receives the verified public config JSON, not repository files or secrets.
The request supplies no tools and sets `store: false` (this disables response
retrieval storage, not all provider retention). Returned claims must match an
allowed field/JSON-pointer pair and the literal source value. Unsupported, null,
duplicate, fabricated, malformed, incomplete, or oversized results are rejected.
An empty claim list is valid; unknown facts remain unknown.

The validator and prompt share the same field/path table. Ollama's `model_type`
is a parameter-size label (for example, `42B`), not an architecture. Deterministic
`sourceClaims` copy supported literal values directly from the verified config,
even with AI disabled or rejected. These observations are separate from AI
`claims` and are not reviewed quality evidence. Neither set performs unit
conversion or infers catalog-ready values.

`extractionError` records bounded static reasons such as `duplicate claim field`
or `claim value differs from source`. Raw rejected model output and HTTP error
bodies are not logged or persisted. The CLI writes partial reports and exits 2
for source/extraction errors, 1 for fatal input/output errors, and 0 for collection
without errors. Actions preserves partial results in the review PR before marking
the run failed; quality-policy failure alone remains a remediation condition.

Manifest-derived artifact pins and sizes are independent of AI. The proposal
collector never modifies the curated catalog or quality evidence. Existing
`catalog-enrich` updates to known artifact metadata still appear separately in
the same reviewed PR.

## Review and Publication

The PR contains `docs/references/catalog-proposals.json` (raw source documents,
their hashes, artifact observations, extraction status, and unverified claims)
and `docs/references/catalog-quality-latest-report.json` (the offline quality
diagnostic). Treat source text as untrusted data, not commands or instructions.

Reviewers must independently verify model identity, license, capabilities,
parameter counts, context, geometry, and integrity pins before admission. Registry
config often lacks these facts; this collector does not fetch Hugging Face model
cards or infer missing values. `inventoryComplete` is always false and
`requiresReview` always true. Reviewers must separately maintain the evidence
described in [catalog-quality.md](catalog-quality.md).

Quality failure permits a remediation PR but never publication. The independent
90% freshness and correctness thresholds, complete inventory requirement, known
mismatch blocking, and protected signing approval remain unchanged.

## Local Verification

```sh
cargo test --locked -p rigspark-runtime --test catalog_proposals
cargo test --locked -p rigspark-cli --test catalog_propose_cli
cargo catalog-propose --source-only --limit 1 --out /tmp/catalog-proposals.json
cargo catalog-propose --source-only --resume-from /tmp/catalog-proposals.json --out /tmp/catalog-proposals-next.json
```

The first two commands use injected fixtures with no live API calls. The third is
an explicit public-metadata smoke check with AI disabled. `--fixture` selects a
recorded inventory/response transport with no network fallback; `--now` fixes the
report timestamp. Output cannot replace the input catalog, fixture, or resume report.

OpenAI request/response contract:
[official Responses API reference](https://developers.openai.com/api/reference/resources/responses/methods/create).
The first paid run returned rejected claims for all 10 candidates. The old report
did not preserve rejection reasons, so its exact cause cannot be reconstructed.
The observed config inputs exposed the incorrect `model_type` mapping, now covered
by regression tests. Validate the corrected paid path with one candidate after
deployment; offline fixtures do not prove provider behavior.