# Implementation Plan: Generation Catalog Enrichment and Unified Model Selection

## Overview

Extend the approved image/video generation feature with safe Hugging Face enrichment
and a single GUI model-selection flow. Text, image, and video models live in one Models
surface; Chat uses one active selection per kind; the separate Create surface is retired.

## Architecture Decisions

- Keep `generation.json` separate from the text catalog so advice remains deterministic
  and existing text signing/ranking contracts do not change.
- Use one typed enrichment implementation parameterized by generation kind, invoked by
  two independent GitHub Actions workflows.
- Treat Hugging Face as untrusted input. Only built-in workflow families are eligible for
  automatic catalog mutation; all other candidates appear in a review report.
- Persist image/video selections in browser-local state. Text activity continues to
  come from the runtime API; ComfyUI runtime defaults come from the generation API.
- Keep generation execution behind the existing authenticated loopback API.

## Task List

### Phase 1: Enrichment Foundation

- [ ] Task 1: Add typed generation enrichment
  - Acceptance: parses bounded Hugging Face metadata; enriches only the requested kind
    and allowlisted family; pins revision, sizes, and digests; validates final catalog.
  - Verify: `cargo test --locked -p rigspark-core --test generation_enrich`
  - Files: core enrichment module, core module export, focused tests and fixtures.
  - Dependencies: None.

- [ ] Task 2: Add a maintenance CLI
  - Acceptance: image/video kind is explicit; dry-run/report and write modes are
    deterministic; invalid or partial candidates fail visibly; unknowns are reported.
  - Verify: focused CLI integration test.
  - Files: CLI binary, Cargo target/alias, CLI integration test.
  - Dependencies: Task 1.

### Checkpoint: Enrichment Foundation

- [ ] Core and CLI focused tests pass.
- [ ] A second identical run is a byte-identical no-op.

### Phase 2: Review-Gated Automation

- [ ] Task 3: Add dedicated image and video workflows
  - Acceptance: each scheduled/manual workflow runs the focused tests and CLI for one
    kind, detects catalog/report changes, and opens a dedicated review PR without direct
    protected-branch writes.
  - Verify: workflow structure tests plus YAML inspection.
  - Files: two workflow files and workflow-focused test.
  - Dependencies: Task 2.

### Phase 3: Unified Models and Chat

- [ ] Task 4: Add generation-kind Models views
  - Acceptance: keyboard-accessible Text/Image/Video tabs; generation cards filter by
    kind; choosing a card updates the active selection; active summary names all kinds.
  - Verify: GUI browser fixture/model journey assertions.
  - Files: GUI markup, styles, generation JS, browser fixture/journey test.
  - Dependencies: None.

- [ ] Task 5: Move generation into minimalist Chat and retire Create
  - Acceptance: Create is absent from navigation and markup; Chat automatically uses
    detected ComfyUI defaults without configuration fields; generation uses the selected
    image/video model and successful replies expose a direct Download action.
  - Verify: GUI generation API tests and browser smoke/journey assertions.
  - Files: GUI markup, styles, generation JS, focused browser tests.
  - Dependencies: Task 4.

### Checkpoint: Unified GUI

- [ ] Text model start behavior is unchanged.
- [ ] Image and video Chat generation use their selected models.
- [ ] Desktop and mobile layouts have no horizontal overflow.

### Phase 4: Final Verification

- [ ] Task 6: Update user documentation and run quality gates
  - Acceptance: docs describe Models tabs, active selections, Chat generation, and both
    maintenance pipelines; no Create-view instructions remain.
  - Verify: `cargo fmt --all -- --check`; focused tests; workspace Clippy/build/tests;
    `cargo native-retirement`.
  - Dependencies: Tasks 3 and 5.

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Hugging Face metadata changes or is incomplete | High | Typed bounded parsing, immutable revisions, digest requirement, visible rejection report |
| Metadata-only model is not runnable | High | Automatic mutation only for built-in workflow-family mappings |
| GUI selection diverges from execution | High | One selected-model state consumed by cards, active summary, and Chat request payload |
| Create retirement removes required settings | Medium | Move every required generation setting into Chat before removing the view |
| Scheduled workflows race | Medium | Kind-specific concurrency groups and pending-PR checks |

## Open Questions

None. The v1.2 behavior and automation boundary are approved in the feature spec.
