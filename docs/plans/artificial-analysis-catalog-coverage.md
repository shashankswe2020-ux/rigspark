# Implementation Plan: Artificial Analysis Open-Weights Catalog Coverage

## Overview

Implement the approved `artificial-analysis-catalog-coverage.md` specification in
dependency order. The pipeline will capture the public Artificial Analysis Open Weights
inventory, resolve each model to immutable official weight artifacts, deduplicate benchmark
configurations by artifact digest, represent unsupported formats as advisory-only, and block
catalog publication whenever verified downloadable coverage is incomplete.

No production catalog publication is part of this plan. Each increment must leave the
workspace buildable and testable.

## Architecture Decisions

- Artificial Analysis supplies inventory membership and Open Weights classification only.
  Official publisher repositories supply license, revision, size, digest, and model facts.
- A recorded, typed public export is the deterministic test boundary. The collector must not
  rely on authentication, premium access, browser state, or undocumented credentialed APIs.
- Schema v4 represents availability explicitly and reads schema v2/v3 compatibly.
- Artifact digest is the deduplication identity. Names are matching inputs, never integrity
  identities.
- Coverage and admission are separate pure operations: collection produces evidence;
  admission consumes validated evidence.
- Publication fails closed on source failure, ambiguous identity, unsupported evidence, or an
  uncovered verified Open Weights artifact.

## Dependency Graph

```text
Public source contract
    ├── Schema-v4 availability ── lifecycle rejection
    └── Pure inventory + identity model
             └── Publisher artifact resolution
                     └── Coverage report + CLI
                             └── Catalog admission
                                     ├── CLI/TUI presentation
                                     ├── GUI presentation
                                     └── Workflow publication gate
```

## Phase 1: Source and Safety Foundations

### Task 1: Freeze the public inventory contract

**Description:** Establish a bounded, public Artificial Analysis input that exposes model
identity and Open Weights classification without authentication. Capture a minimal recorded
fixture and a typed parser before any network collector is added.

**Acceptance criteria:**

- The parser distinguishes Open Weights and proprietary rows and rejects an empty inventory.
- Multiple benchmark configurations retain enough source identity to be collapsed later.
- Malformed, oversized, unauthorized, or structurally changed responses fail explicitly.

**Verification:**

- `cargo test --locked -p rigspark-core --test artificial_analysis_coverage inventory`

**Dependencies:** None

**Files likely touched:**

- `crates/rigspark-core/src/artificial_analysis.rs`
- `crates/rigspark-core/src/lib.rs`
- `crates/rigspark-core/tests/artificial_analysis_coverage.rs`
- `crates/rigspark-core/fixtures/artificial-analysis-inventory.json`

**Estimated scope:** Medium

### Task 2: Add schema-v4 availability

**Description:** Add typed runnable and advisory-only availability states while preserving
schema-v2/v3 parsing semantics and catalog validation.

**Acceptance criteria:**

- Schema v4 requires availability on newly encoded entries.
- Schema v2/v3 entries resolve to their existing runnable behavior.
- Invalid backend/reason combinations and unsafe text fail catalog validation.

**Verification:**

- `cargo test --locked -p rigspark-core --test catalog availability`
- `cargo test --locked -p rigspark-core --test catalog_schema_compat`

**Dependencies:** None

**Files likely touched:**

- `crates/rigspark-core/src/catalog.rs`
- `crates/rigspark-core/tests/catalog.rs`
- `crates/rigspark-core/tests/catalog_schema_compat.rs`
- `crates/rigspark-core/data/models.json`

**Estimated scope:** Medium

### Task 3: Enforce advisory-only lifecycle rejection

**Description:** Make model activation reject advisory-only entries before acquisition,
filesystem mutation, runtime selection, or process spawn.

**Acceptance criteria:**

- `up` and `switch` return a typed backend-support-unavailable error.
- Injected transports, filesystems, and child-process harnesses observe zero side effects.
- Runnable schema-v4 and legacy entries preserve existing behavior.

**Verification:**

- `cargo test --locked -p rigspark-runtime --test lifecycle advisory_only`
- `cargo test --locked -p rigspark-cli --test workflow_parity advisory_only`

**Dependencies:** Task 2

**Files likely touched:**

- `crates/rigspark-runtime/src/lifecycle.rs`
- `crates/rigspark-runtime/src/acquire.rs`
- `crates/rigspark-runtime/tests/lifecycle.rs`
- `crates/rigspark-cli/tests/workflow_parity.rs`

**Estimated scope:** Medium

## Checkpoint 1: Safe Representation

- Schema v2/v3 compatibility and schema-v4 validation pass.
- Advisory-only entries cannot trigger network, storage, or process side effects.
- The public inventory fixture parses deterministically.
- `cargo build --workspace --locked`

## Phase 2: Evidence and Coverage

### Task 4: Normalize identities and deduplicate artifacts

**Description:** Implement pure matching inputs and immutable artifact identities. Collapse
Artificial Analysis configurations only after they resolve to the same publisher revision and
digest; reject ambiguous name-only matches.

**Acceptance criteria:**

- Configurations sharing a digest collapse to one artifact identity.
- Different sizes or revisions remain distinct.
- Ambiguous publisher/model aliases are reported, not guessed.

**Verification:**

- `cargo test --locked -p rigspark-core --test artificial_analysis_coverage identity`

**Dependencies:** Task 1

**Files likely touched:**

- `crates/rigspark-core/src/artificial_analysis.rs`
- `crates/rigspark-core/tests/artificial_analysis_coverage.rs`
- `crates/rigspark-core/fixtures/artificial-analysis-artifacts.json`

**Estimated scope:** Small

### Task 5: Resolve official publisher artifacts

**Description:** Add a bounded injected transport that resolves matched models against official
publisher metadata and returns immutable revision, license, file sizes, digests, format, and
sourced model facts.

**Acceptance criteria:**

- Only approved HTTPS publisher hosts and bounded typed responses are accepted.
- License, revision, digest, and size are mandatory for downloadable coverage.
- Unsupported but valid formats resolve successfully as advisory-only candidates.

**Verification:**

- `cargo test --locked -p rigspark-runtime --test artificial_analysis_coverage publisher`

**Dependencies:** Tasks 1 and 4

**Files likely touched:**

- `crates/rigspark-runtime/src/artificial_analysis.rs`
- `crates/rigspark-runtime/src/lib.rs`
- `crates/rigspark-runtime/tests/artificial_analysis_coverage.rs`
- `crates/rigspark-runtime/tests/fixtures/artificial-analysis-recorded.json`

**Estimated scope:** Medium

### Task 6: Build the coverage report and check command

**Description:** Produce the deterministic coverage denominator, status categories, unmatched
reasons, and CLI exit contract. The command writes reports atomically and check mode performs no
writes.

**Acceptance criteria:**

- The report counts rows, Open Weights models, unique artifacts, runnable, advisory-only,
  unmatched, proprietary exclusions, and collapsed duplicates.
- `--check` exits nonzero for any uncovered verified artifact or ambiguous identity.
- An upstream failure cannot replace the last verified inventory with an empty snapshot.

**Verification:**

- `cargo test --locked -p rigspark-core --test artificial_analysis_coverage report`
- `cargo test --locked -p rigspark-cli --test catalog_aa_coverage_cli`

**Dependencies:** Tasks 4 and 5

**Files likely touched:**

- `crates/rigspark-core/src/artificial_analysis.rs`
- `crates/rigspark-cli/src/catalog_aa_coverage.rs`
- `crates/rigspark-cli/tests/catalog_aa_coverage_cli.rs`
- `.cargo/config.toml`

**Estimated scope:** Medium

## Checkpoint 2: Deterministic Coverage

- Recorded end-to-end collection produces a complete typed coverage report.
- Failure fixtures prove empty and ambiguous inputs fail closed.
- No live network is used by tests.
- `cargo test --locked -p rigspark-core --test artificial_analysis_coverage`
- `cargo test --locked -p rigspark-runtime --test artificial_analysis_coverage`

## Phase 3: Catalog Integration

### Task 7: Admit resolved Artificial Analysis artifacts

**Description:** Merge resolved artifacts into the catalog without weakening Ollama admission.
Reuse an existing runnable artifact when possible; otherwise add an advisory-only entry with
honest unknowns.

**Acceptance criteria:**

- Every complete resolved artifact produces or matches exactly one catalog entry.
- Runnable entries retain integrity-pinned backend sources.
- Unsupported artifacts are visible but cannot be selected for activation.

**Verification:**

- `cargo test --locked -p rigspark-core --test artificial_analysis_admission`
- `cargo test --locked -p rigspark-runtime --test artificial_analysis_coverage admission`

**Dependencies:** Tasks 2, 5, and 6

**Files likely touched:**

- `crates/rigspark-core/src/artificial_analysis.rs`
- `crates/rigspark-core/tests/artificial_analysis_admission.rs`
- `crates/rigspark-runtime/src/artificial_analysis.rs`
- `crates/rigspark-runtime/tests/artificial_analysis_coverage.rs`
- `crates/rigspark-core/data/models.json`

**Estimated scope:** Medium

### Task 8: Present availability in CLI and TUI

**Description:** Show advisory-only models in recommendation and catalog output while preventing
install-oriented affordances and preserving accessible/plain output.

**Acceptance criteria:**

- `recommend`, `can-run`, and `catalog` label advisory-only entries "not yet installable".
- TUI actions cannot invoke activation for advisory-only entries.
- Existing runnable output snapshots remain stable except for intentional labels.

**Verification:**

- `cargo test --locked -p rigspark-cli --test accessible_recommend`
- `cargo test --locked -p rigspark-cli --test accessible_catalog`
- `cargo test --locked -p rigspark-cli --test tui_pty`

**Dependencies:** Tasks 2, 3, and 7

**Files likely touched:**

- `crates/rigspark-cli/src/accessible_recommend.rs`
- `crates/rigspark-cli/src/accessible_catalog.rs`
- `crates/rigspark-cli/src/tui_models.rs`
- `crates/rigspark-cli/tests/accessible_recommend.rs`
- `crates/rigspark-cli/tests/accessible_catalog.rs`

**Estimated scope:** Medium

### Task 9: Present availability in the GUI

**Description:** Add the same advisory-only state to the shared GUI model response and embedded
client, with disabled install actions and an explanatory status.

**Acceptance criteria:**

- GUI API responses expose typed availability.
- Advisory-only cards remain discoverable and show hardware advice.
- Install/switch controls are absent or disabled with an accessible explanation.

**Verification:**

- `cargo test --locked -p rigspark-gui models`
- `cargo native-browser-smoke`

**Dependencies:** Tasks 2, 3, and 7

**Files likely touched:**

- `crates/rigspark-gui/src/models.rs`
- `crates/rigspark-gui/static/chat.js`
- `crates/rigspark-gui/static/styles.css`
- `crates/rigspark-gui/tests/models.rs`

**Estimated scope:** Medium

## Checkpoint 3: User-Facing Behavior

- Runnable and advisory-only artifacts are both discoverable.
- Advice remains offline and deterministic.
- Every activation surface rejects advisory-only entries before side effects.
- CLI/TUI/GUI focused suites pass.

## Phase 4: Continuous Enforcement

### Task 10: Gate catalog publication on Artificial Analysis coverage

**Description:** Wire collection, admission, report generation, and `--check` into the weekly
catalog workflow before quality checking, signing, or publication. Reconcile a dedicated issue
for unmatched models and source failures.

**Acceptance criteria:**

- Coverage runs before signing/publication and blocks both on failure.
- Workflow permissions remain minimal and no credentials are exposed to untrusted pull requests.
- Only the approved inventory, report, catalog, and evidence paths enter data-only admission PRs.

**Verification:**

- `cargo test --locked -p rigspark-cli --test shipping_policy artificial_analysis`
- `cargo test --locked -p rigspark-cli --test catalog_aa_coverage_cli`

**Dependencies:** Tasks 6 and 7

**Files likely touched:**

- `.github/workflows/catalog-refresh.yml`
- `crates/rigspark-cli/tests/shipping_policy.rs`
- `docs/references/artificial-analysis-inventory.json`
- `docs/references/artificial-analysis-coverage.json`
- `docs/references/guide.md`

**Estimated scope:** Medium

### Task 11: Run release-quality verification

**Description:** Run the full project gates, inspect only failures related to the change, and
prepare the result for code review. Do not publish a production catalog.

**Acceptance criteria:**

- All focused and workspace tests pass.
- Build, Clippy, formatting, and native-retirement gates pass.
- A five-axis review finds no unresolved correctness, architecture, security, performance, or
  readability blocker.

**Verification:**

```bash
cargo test --workspace --locked -- --test-threads=2
cargo build --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo native-retirement
```

**Dependencies:** Tasks 8, 9, and 10

**Files likely touched:** None unless verification reveals a tightly coupled defect.

**Estimated scope:** Small

## Checkpoint 4: Complete

- All approved success criteria are demonstrably satisfied.
- Coverage is complete against the recorded current public inventory.
- Production publication remains a separate explicit approval.
- The change is ready for merge review.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Public index structure changes | High | Typed structural checks, non-empty invariant, retained last verified snapshot, blocking issue |
| Index names do not uniquely map to publisher repositories | High | Explicit alias evidence and ambiguity rejection; never fuzzy-admit |
| Downloadable weights are too large to inspect | Medium | Metadata/LFS APIs and bounded header ranges; never download full weights in maintenance |
| License label differs from official license | High | Official license is authoritative; conflict blocks admission |
| Advisory-only entries imply false installability | High | Typed availability, lifecycle preflight, disabled UI actions |
| Catalog size grows materially | Medium | Digest deduplication and measured serialized-size limits |
| Artificial Analysis terms disallow automation | High | Use only documented public export/page data; stop and request approval if compliant automation is unavailable |

## Parallelization

- After Task 2, Task 3 can proceed independently of Tasks 4-6.
- After Task 7, Tasks 8 and 9 can proceed in parallel.
- Task 10 waits for the report and admission contracts.
- Task 11 is strictly last.

## Open Questions

None at plan time. If Task 1 cannot establish a compliant public machine-readable contract,
implementation stops before network code and returns to specification review.
