# Spec: Artificial Analysis Open-Weights Catalog Coverage

Status: approved, implementation in progress · Owner: catalog pipeline · Extends:
`catalog-auto-admission.md`

## Objective

RigSpark's catalog must continuously represent every model in the Artificial Analysis
Intelligence Index that Artificial Analysis classifies as **Open Weights** and for which
downloadable model weights can be verified.

Coverage is based on unique downloadable weight artifacts, not leaderboard rows. Multiple
benchmark configurations, reasoning levels, providers, or inference settings that use the
same weights map to one catalog entry. Artificial Analysis scores are not copied into the
catalog and do not affect RigSpark ranking.

The requirement covers models even when they have no Ollama tag or no artifact that an
existing RigSpark backend can serve. Such entries remain useful for hardware-fit advice but
must be explicitly marked advisory-only; installation and serving fail before download with
a typed "backend support unavailable" error.

## Definitions

- **Index model:** a model shown by the public Artificial Analysis Intelligence Index.
- **Open Weights:** the index's public Open Weights/Open Source classification. Proprietary
  models are outside the coverage denominator.
- **Downloadable weights:** at least one official publisher artifact with a source URL,
  immutable revision, byte size, cryptographic digest, and a license accepted by RigSpark's
  open-weight policy.
- **Unique weight artifact:** publisher identity plus immutable revision and digest. Different
  leaderboard configurations over the same artifact do not create duplicate catalog entries.
- **Runnable:** at least one verified artifact can be installed and served by an existing
  RigSpark backend.
- **Advisory-only:** verified downloadable weights exist, but no existing backend adapter can
  serve them.

## Source and Evidence Contract

Artificial Analysis is the authoritative source for the coverage inventory and Open Weights
classification only. Artifact facts come from official model publishers:

| Fact | Required source | Failure behavior |
|---|---|---|
| Index membership and display name | Public Artificial Analysis Intelligence Index/export | Preserve the previous inventory and fail the coverage gate visibly |
| Open Weights classification | Public Artificial Analysis filter/export | Exclude proprietary entries from the denominator |
| Publisher and canonical model identity | Artificial Analysis metadata plus official publisher page | Leave unmatched and fail coverage |
| License | Official repository metadata or license file | Leave unmatched; never infer openness from the index label alone |
| Revision and files | Official publisher repository API | Leave unmatched |
| Digest and byte size | Publisher LFS metadata or immutable registry manifest | Leave unmatched |
| Parameter count and architecture | Weight metadata/config; GGUF header when applicable | Use `unknown` where the catalog permits it; never estimate |
| Runnable backend | Existing backend capability and artifact-format mapping | Mark advisory-only when unsupported |

The collector may use a documented public export or public page data. It must not bypass
authentication, use premium-only endpoints, or depend on browser credentials. All external
documents are bounded, typed, validated, and captured as deterministic test fixtures.

## Catalog Contract

Catalog schema version 4 adds an availability field:

```json
{
  "availability": {
    "status": "runnable",
    "backend": "ollama"
  }
}
```

or:

```json
{
  "availability": {
    "status": "advisory-only",
    "reason": "backend-format-unsupported"
  }
}
```

Rules:

- Existing schema-v2 and schema-v3 catalogs remain readable. Missing availability on those
  versions means runnable under their existing source rules.
- New auto-admitted entries always include availability.
- `recommend`, `can-run`, `catalog`, the TUI, and the GUI show advisory-only entries and their
  memory verdicts, clearly labelled **not yet installable**.
- `up` and `switch` reject advisory-only entries before network or process activity.
- Unknown sizing inputs produce `unknown`; coverage never justifies fabricated memory or
  throughput figures.
- Artifact aliases and Artificial Analysis configurations are deduplicated by immutable
  digest. Distinct parameter sizes or distinct weight revisions remain separate entries.
- Removal from the current index does not immediately delete a catalog entry. It leaves the
  coverage scope and follows the existing reviewed retirement policy.

### Staged availability rollout

The first availability increment adds schema-v4 reading and lifecycle preflight, without
republishing or rewriting the bundled schema-v3 catalog. `SCHEMA_VERSION` remains the existing
maintenance writer version; `MAX_SCHEMA_VERSION` is the highest reader-supported version.
Publication switches to v4 only when admission and all presentation surfaces support it.
V4 fields are forbidden in legacy documents, and v4 documents must give every model explicit
availability. Legacy entries retain their existing source selection; no runnable backend is
invented for an old Hugging Face-only entry.

For weights that are not installable by a current backend, `source.weights` stores an immutable
publisher repository/revision and a list of pinned files (`file`, `bytes`, `sha256`). It is
accepted only for advisory-only entries, with one quantization whose disk size exactly equals
the manifest sum. This increment supports safetensors and GGUF weight files only; unknown
formats remain unresolved, not implicitly accepted. Configs and executable repository code
are not weight files. Manifests are bounded to 256 unique safe paths, and every file has a
positive integral size and SHA-256. Sharded weights have per-file digests, not a fabricated
aggregate weight digest.

The guard is shared by CLI and GUI activation and the native runtime application. It runs
before hardware/backend probing and runtime state access, including explicit installed-model
selection, source-reference aliases, bypass and already-active/switch shortcuts. Down and
doctor remain usable for existing runtime state. `ModelUnavailable` is the typed core error;
runtime and HTTP boundaries use the repository's existing error presentation.

## Continuous Coverage Gate

### Identity matching contract

An explicit `PublisherMapping` associates an exact Artificial Analysis creator slug and
release slug with one repository. Names, suffixes and reasoning settings never select a
repository. Identical mappings are idempotent; conflicting repositories for the same pair
are an error. Missing mappings remain unmatched, and proprietary rows are separately excluded.
These associations alone do not verify publisher authority, license, or downloadability.

After publisher resolution, each observation names its original index row ID and pinned
source. Unknown, unmatched, proprietary or wrong-repository observations are rejected.
Artifact identity comprises the exact repository, lowercase immutable revision, and sorted
multiset of lowercase SHA-256/byte-size pairs. Order and file aliases do not affect identity;
distinct repositories, revisions or weight digests do. Shards are compared individually,
without inventing an aggregate digest. The same digest with conflicting byte sizes is an
error. Rows and groups are deterministically sorted, and duplicate observations do not
inflate group membership.

Inputs are bounded to 10,000 rows, mappings and observations each, with the shared catalog
weight-manifest validator enforcing file-level constraints. Grouping an empty observation
set produces no artifacts, not a claim of complete coverage. The publication report must
reconcile every in-scope row against these groups and report unresolved rows.

### Publisher resolution contract

The Hugging Face resolver consumes a **reviewed official-publisher association and an explicit
complete export file selection**. A successful metadata lookup does not establish publisher
authority: maintainers must verify the association against the publisher's own release page
before it becomes a collection input. No associations or exports are inferred from display names,
and this increment does not ship real mappings or import catalog entries.

The resolver reuses the admission transport, reads `/api/models/{owner}/{repo}?blobs=true`, and
requires an exact repository match, a 40-hex revision, explicit `private: false` and `gated: false`,
an accepted unambiguous license tag, and agreement with a model-card license when present.
Every selected file needs publisher LFS SHA-256 and size evidence; a separately reported file
size must agree. Missing/duplicate files, mixed weight formats, inconsistent digest sizes, unsafe
paths and invalid manifests fail explicitly. Standard `-00001-of-00002` shard sets must contain
every index exactly once. Nonstandard export layouts still require a reviewed complete selection;
filename matching alone is not a general proof of model completeness.

The result records safetensors or GGUF format, pinned files and license, without claiming backend
support. When listed by the publisher, the root `config.json` is fetched at the resolved immutable
revision, never at `main`. If no config is listed, config-derived facts and its source URL stay
explicitly unknown; verified weights are still resolved.
Only sourced `model_type` and `max_position_embeddings` facts are retained; absent values stay
unknown. This is the config's position limit, not a claim about runtime-supported context or
RoPE scaling. Parameter count, attention geometry and backend capability still need separate
evidence during admission. Repository code is never fetched or executed.

Metadata is capped at 4 MiB, config at 64 KiB, and each request at 120 seconds. Responses must
be HTTP 200, including through injected transports; partial responses, failed config reads,
oversized documents, malformed JSON and cancellation are errors, not successful unknown-only
records. The existing HTTPS host/redirect policy gains only pinned root `config.json` reads.
Full-response reads continue to EOF at the exact cap, rejecting any subsequent bytes before
extending the buffer; range reads may stop at their cap.
No weight downloads, credentials, cache writes or catalog mutations occur in this resolver.

Protocol sources:

- [Hugging Face Hub API endpoints](https://huggingface.co/docs/hub/api)
- [Hugging Face model-info API reference](https://huggingface.co/docs/huggingface_hub/package_reference/hf_api#huggingface_hub.HfApi.model_info)

The catalog workflow captures a versioned Artificial Analysis inventory before admission,
then emits a machine-readable report with:

- total public index rows;
- Open Weights rows;
- unique Open Weights artifacts;
- covered runnable artifacts;
- covered advisory-only artifacts;
- unmatched models and explicit reasons;
- duplicate leaderboard configurations collapsed;
- source timestamp and digest.

Publication passes only when:

1. every unique Open Weights model with verifiable downloadable weights maps to exactly one or
   more catalog entries representing its distinct artifacts;
2. no proprietary model is admitted by this source;
3. every runnable entry has an integrity-pinned backend source;
4. every unsupported entry is advisory-only;
5. no unresolved identity collision or ambiguous artifact mapping exists.

An upstream fetch or parse failure is not interpreted as an empty inventory. The workflow
preserves the previous verified inventory, opens or updates a remediation issue, and blocks
catalog publication.

## Commands

```bash
cargo catalog-aa-coverage --fixture <recorded.json>
cargo catalog-aa-coverage --check
cargo catalog-admit --dry-run
cargo test --locked -p rigspark-core --test artificial_analysis_coverage
cargo test --locked -p rigspark-runtime --test artificial_analysis_coverage
cargo test --workspace --locked -- --test-threads=2
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo native-retirement
```

## Project Structure

```text
crates/rigspark-core/src/artificial_analysis.rs
    Typed inventory, identity normalization, artifact deduplication, coverage report.
crates/rigspark-runtime/src/artificial_analysis.rs
    Bounded public-source and official-publisher transports.
crates/rigspark-cli/src/catalog_aa_coverage.rs
    Maintenance command and check-mode exit contract.
crates/rigspark-core/src/catalog.rs
    Schema-v4 availability types and backward-compatible parsing.
crates/rigspark-core/tests/artificial_analysis_coverage.rs
crates/rigspark-runtime/tests/artificial_analysis_coverage.rs
docs/references/artificial-analysis-inventory.json
docs/references/artificial-analysis-coverage.json
```

## Code Style

Use typed states instead of strings or sentinel values:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Availability {
    Runnable { backend: RuntimeBackend },
    AdvisoryOnly { reason: AdvisoryReason },
}
```

External names and URLs are untrusted input. Normalize only documented aliases, retain source
identity in evidence, and reject ambiguous matches rather than choosing heuristically.

## Testing Strategy

- Pure unit tests cover model-name normalization, configuration collapsing, digest
  deduplication, proprietary exclusion, and coverage arithmetic.
- Recorded fixtures cover the public index response and official publisher metadata. Tests
  never contact Artificial Analysis, Hugging Face, Ollama, or another registry.
- Schema tests cover v2/v3 compatibility, v4 round trips, invalid availability combinations,
  and advisory-only lifecycle rejection.
- Workflow policy tests prove the coverage check runs before signing/publication and that a
  failed or empty upstream inventory blocks publication.
- CLI, TUI, and GUI tests prove advisory-only entries are visible and cannot be installed.

## Boundaries

- **Always:** preserve offline deterministic advice; validate and bound every external
  document; pin downloadable artifacts by digest; fail closed on identity, license, and
  integrity ambiguity; keep coverage evidence reviewable.
- **Ask first:** add a dependency, add a backend, change the Artificial Analysis source
  contract, or publish a production catalog.
- **Never:** scrape authenticated or premium data; copy Artificial Analysis benchmark scores
  into ranking; fabricate missing hardware figures; treat Open Weights labelling as license
  proof; hide unsupported entries as runnable; make advice commands access the network.

## Success Criteria

1. A deterministic fixture containing multiple leaderboard configurations over one artifact
   produces one catalog artifact identity.
2. Every verified Open Weights artifact is represented as runnable or advisory-only.
3. A fixture with one unmatched downloadable Open Weights model makes `--check` fail and names
   the unmatched model and reason.
4. Proprietary rows never enter the catalog or the Open Weights coverage denominator.
5. Advisory-only `up` and `switch` fail before any network, filesystem mutation, or child
   process spawn.
6. Empty, malformed, unauthorized, or structurally changed upstream responses block
   publication without replacing the last verified inventory.
7. The full workspace quality gates pass.

## Open Questions

None. The requirement decisions are:

- continuous enforcement, not a one-time import;
- every Open Weights model with verifiable downloadable weights;
- one catalog identity per unique downloadable weight artifact;
- explicit advisory-only catalog state when no current backend can serve the artifact.
