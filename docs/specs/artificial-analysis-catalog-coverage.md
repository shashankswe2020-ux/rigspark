# Spec: Artificial Analysis Open-Weights Catalog Coverage

Status: proposed, pending approval · Owner: catalog pipeline · Extends:
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

## Continuous Coverage Gate

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
