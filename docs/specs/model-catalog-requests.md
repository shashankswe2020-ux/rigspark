# Spec: User-requested model catalog admission

Status: proposed · Date: 2026-10-13 · Owner: catalog pipeline

## Objective

Let a RigSpark user request a model that is missing from the model catalog without
having to understand the catalog schema or prepare a pull request.

The Models view adds a **Request a model** button. It opens a dedicated GitHub issue
form, where a signed-in GitHub user submits:

- one canonical Hugging Face model URL; and
- an optional reason or use case.

The created issue is the public status page. A GitHub Actions workflow validates the
request and either:

1. creates a source-backed draft catalog pull request and links it from the issue;
2. marks the issue as already cataloged or a duplicate; or
3. keeps the issue open with a precise blocked status, such as
   `catalog:needs-runnable-artifact`.

Submitting a request does not guarantee admission. A model reaches a draft pull request
only when RigSpark can prove all required catalog facts and identify an integrity-pinned
artifact supported by an existing runtime.

### User stories

- As a user browsing Models, I can request a missing model through a short, structured
  form.
- As a requester, I can follow one public URL to see validation results, blockers, and
  the resulting pull request.
- As a maintainer, I receive a reviewable data-only draft pull request for eligible
  requests instead of manually transcribing model metadata.
- As a maintainer, I can retry a blocked request after upstream artifacts or metadata
  become available.

## Recommended direction

Use a GitHub issue as the intake queue, audit record, deduplication surface, and public
status page:

```text
Models view
  -> GitHub model-request issue form
  -> issue opened with catalog:request
  -> catalog-request.yml
  -> typed Rust request validator/admission command
  -> status comment + labels
  -> data-only draft PR when eligible
  -> normal review and merge gates
  -> existing signed-catalog publication process
```

This route deliberately requires GitHub sign-in. GitHub Actions cannot safely accept an
anonymous browser submission by themselves. Anonymous intake would require a separately
hosted public broker with abuse prevention and a GitHub App credential. That
infrastructure is not justified for the first version.

The workflow must create a **draft** PR and must never push directly to `main`, merge the
PR, or publish a signed catalog. Existing review, CI, catalog-quality, signing, and
publication controls remain authoritative.

## User experience

### Models view

Place **Request a model** beside the existing catalog status/update controls in
`crates/rigspark-gui/static/index.html`.

- The control is a link styled as a button, not a local form submission.
- It opens the repository's `model_request.yml` issue form in the user's browser.
- Its accessible name is `Request a model`.
- Supporting text says: `Requires GitHub sign-in. Requests are public.`
- The app does not collect a GitHub token, email address, or other identity data.
- If the browser cannot be opened, show the copyable issue-form URL.

The first version does not attempt to prefill issue-form fields. GitHub supports
selecting an issue template and prefilling a title, but structured issue-form answers
must remain explicit user input.

### Issue form

Add `.github/ISSUE_TEMPLATE/model_request.yml` with:

| Field | Requirement |
|---|---|
| Hugging Face model URL | Required single-line input |
| Reason or use case | Optional textarea, maximum guidance of 1,000 characters |
| Integrity acknowledgement | Required checkbox explaining that requests need a public, license-compatible, runnable artifact |

The template applies `catalog:request` and uses the title prefix
`[Model request]: `. It must say that issue contents are public and that arbitrary links,
credentials, private-model access, and uploaded files must not be included.

Accepted input is exactly a canonical repository URL:

```text
https://huggingface.co/<owner>/<repository>
```

The validator rejects credentials, ports, query strings, fragments, extra path
components, non-HTTPS schemes, alternate hosts, and identifiers outside the bounded
Hugging Face owner/repository grammar. It normalizes the URL to a repository ID before
deduplication.

### Status model

Labels are the machine-readable request state:

| Label | Meaning | Terminal |
|---|---|---|
| `catalog:request` | Valid intake item | No |
| `catalog:validating` | Workflow owns the current attempt | No |
| `catalog:needs-runnable-artifact` | Base model is valid but no supported pinned GGUF, MLX, or Ollama artifact was proven | No |
| `catalog:needs-metadata` | Required source-backed catalog facts are absent or contradictory | No |
| `catalog:needs-maintainer` | Automation cannot safely decide | No |
| `catalog:pr-open` | Draft PR was created | No |
| `catalog:duplicate` | Canonical request already exists | Yes |
| `catalog:already-available` | Model or equivalent source is already cataloged | Yes |
| `catalog:rejected` | Invalid, private, gated, unsupported-license, or otherwise ineligible request | Yes |
| `catalog:admitted` | Catalog PR merged | Yes |
| `catalog:retry` | Maintainer-controlled request for another validation attempt | No |

Each workflow attempt posts or updates one bot-authored status comment with:

- normalized repository ID;
- attempt result and stable reason code;
- facts proven and facts still missing;
- draft PR URL when created; and
- the workflow run URL for diagnostics.

Do not expose raw internal errors, tokens, runner paths, requester IP addresses, or
unbounded upstream response bodies.

## Admission contract

The request workflow is orchestration only. Source parsing, validation, artifact
selection, catalog construction, and evidence construction belong in a typed Rust
maintenance command so they can be tested with recorded fixtures.

An eligible request must satisfy all of the following:

1. The submitted repository is public and not gated.
2. The Hugging Face API returns a pinned 40-hex commit revision.
3. The license maps unambiguously to RigSpark's open-weight allow-list.
4. Model family, parameter count, architecture, context length, capabilities, and other
   required catalog fields are sourced exactly or are permitted to remain absent by the
   current schema and honesty gate.
5. At least one existing runtime can execute a proven artifact:
   - an Ollama tag with exact manifest digest and size;
   - a GGUF file pinned by repository, commit, path, SHA-256, and byte size; or
   - an MLX manifest pinned by repository and commit with every required file's SHA-256
     and byte size.
6. The artifact's relationship to the requested model is source-backed; name similarity
   is not evidence.
7. The resulting catalog and quality evidence pass existing parsers and quality checks.

If the submitted page contains base weights but no verifiable runtime artifact, keep the
issue open as `catalog:needs-runnable-artifact`. Do not fabricate a quantization, build
one in CI, admit a metadata-only model, or silently substitute a similarly named model.

Auto-generated entries use the existing `provenance: auto` semantics. Curated entries
win, and request automation must not rewrite a curated entry.

## GitHub Actions design

Add `.github/workflows/catalog-request.yml`.

### Triggers

```yaml
on:
  issues:
    types: [opened, reopened, labeled]
```

Run only when:

- the issue has `catalog:request`; and
- the event is `opened` or `reopened`, or the newly applied label is
  `catalog:retry`.

Use a concurrency group keyed by normalized repository ID after parsing. Before parsing,
serialize by issue number. A repository-wide admission lock is acceptable for the MVP
and avoids duplicate branches caused by near-simultaneous requests.

The workflow must not trigger on arbitrary issue edits. A maintainer retries an updated
request by applying `catalog:retry`.

### Permissions

Use job-level least privilege:

- validation/status job: `contents: read`, `issues: write`;
- draft-PR job: `contents: write`, `issues: write`, `pull-requests: write`;
- all unspecified permissions: `none`.

Do not use `pull_request_target`. Do not check out or execute requester-controlled code.
Do not place GitHub expressions containing issue title/body directly inside shell
scripts. The Rust command reads the bounded GitHub event JSON from
`$GITHUB_EVENT_PATH`.

### Workflow stages

1. **Claim**
   - Replace any retry label with `catalog:validating`.
   - Write the initial status comment.
2. **Parse**
   - Read the issue-form field from the event document.
   - Enforce issue-body and field-size limits.
   - Normalize and validate the Hugging Face URL.
3. **Deduplicate**
   - Check `models.json` by normalized `source.hf`, `source.gguf.repo`, and
     `source.mlx.repo`.
   - Check open model-request issues and request branches.
   - Link and close duplicates instead of running admission twice.
4. **Source**
   - Fetch only approved HTTPS hosts with bounded responses, deadlines, redirect rules,
     and recorded fixtures for tests.
   - Treat all Hugging Face and registry responses as untrusted typed input.
5. **Decide**
   - Emit a typed outcome: `eligible`, `blocked`, `rejected`, `duplicate`, or
     `alreadyAvailable`.
   - Blocked and rejected outcomes make no repository changes.
6. **Build**
   - From the current `main` SHA, update only the catalog and directly associated
     quality-evidence files.
   - Run targeted catalog/admission tests, catalog parsing, and quality checks.
7. **Constrain**
   - Refuse changes outside an explicit data-file allow-list.
   - Refuse an empty diff or a diff affecting unrelated catalog entries.
8. **Propose**
   - Push `catalog/request-<issue-number>-<repository-slug>`.
   - Create a draft PR titled
     `chore(catalog): add <repository> (refs #<issue-number>)`.
   - Include sourced facts, unknown facts, artifact digests, validation commands, and the
     request issue in the PR body.
9. **Report**
   - Set `catalog:pr-open`, update the status comment, and leave the issue open.
   - A later workflow or maintainer closes the issue as `catalog:admitted` when the PR
     merges.

Use the default `GITHUB_TOKEN` for the draft PR. Pull requests created by
`GITHUB_TOKEN` do not start another workflow run. A maintainer marking the draft ready
for review provides the human boundary that starts normal pull-request CI. Do not add a
maintainer PAT merely to bypass this boundary in the MVP.

### Fail-closed behavior

- Invalid request: label `catalog:rejected`, explain the stable reason, close the issue.
- Upstream timeout or transient failure: label `catalog:needs-maintainer`, keep open.
- Missing runnable artifact: label `catalog:needs-runnable-artifact`, keep open.
- Missing/contradictory facts: label `catalog:needs-metadata`, keep open.
- Test, quality, or allow-list failure: make no branch/PR, label
  `catalog:needs-maintainer`, keep open.
- Partial push or PR-creation failure: report the branch URL and require maintainer
  cleanup/retry; never report success.

## API and data contracts

The Rust command should return one bounded JSON document to the workflow:

```json
{
  "schemaVersion": 1,
  "issueNumber": 321,
  "repository": "owner/model",
  "outcome": "blocked",
  "reasonCode": "RUNNABLE_ARTIFACT_MISSING",
  "summary": "No supported pinned GGUF, MLX, or Ollama artifact was proven.",
  "changedFiles": [],
  "modelIds": [],
  "evidenceUrls": ["https://huggingface.co/owner/model"]
}
```

Rules:

- `outcome` and `reasonCode` are closed enums.
- Human-readable `summary` is display-only and never controls workflow branching.
- `changedFiles` is validated against the workflow allow-list.
- URLs are parsed and re-serialized typed values, not copied unchecked from issue text.
- Unknown fields and unsupported `schemaVersion` values fail closed.

No public RigSpark HTTP API is added. The loopback GUI remains loopback-only and stores
no GitHub credential.

## Gaps and mitigations

| Gap | Why it matters | MVP treatment |
|---|---|---|
| GitHub sign-in is required | No-infrastructure GitHub issue creation is not anonymous | State this beside the button and in the form |
| Base model is not a runnable artifact | Most Hugging Face model pages cannot directly satisfy GGUF/MLX/Ollama runtime requirements | Keep the issue open with `catalog:needs-runnable-artifact` |
| Reverse artifact discovery is ambiguous | Similar names do not prove lineage | Admit only source-backed relationships; otherwise block |
| Workflow-created PRs do not trigger CI | GitHub suppresses recursive workflow runs from `GITHUB_TOKEN` | Create a draft; maintainer marks it ready |
| Issue body is hostile input | Shell injection, SSRF, oversized input, and log injection are possible | Parse `$GITHUB_EVENT_PATH` in typed Rust; fixed hosts and bounds; strip controls |
| Duplicate requests can race | Two issues can produce conflicting branches | Normalize IDs, serialize admission, check catalog/issues/branches |
| Upstream metadata changes | Branch facts can become stale before merge | Pin revisions/digests and rerun validation in PR CI |
| Catalog publication is privileged | Admission must not imply signing authority | Keep publish workflow separate and protected |
| Labels can loop workflows | Bot label changes emit issue events | Trigger only on opened/reopened or explicit `catalog:retry` |
| Issue reasons may contain sensitive data | Issues are public forever | Warn before submit; never copy the reason into catalog data |

## Alternatives considered

### 1. Direct `workflow_dispatch` or `repository_dispatch` from the GUI

Rejected. It requires a GitHub credential in the client or an unauthenticated credential
broker. A shipped token would be extractable and could be abused.

### 2. Anonymous public broker

Viable later, but rejected for the MVP. It requires hosted infrastructure, bot
protection, rate limiting, privacy policy decisions, availability monitoring, and a
GitHub App secret. If anonymous demand is proven, add a narrow broker that creates the
same issue contract; do not redesign the downstream workflow.

### 3. Mailbox/object-store intake polled by a scheduled Action

Rejected. It adds infrastructure while producing slower feedback and a second status
store. GitHub issues already provide durable state and discussion.

### 4. Draft PR generated in the user's browser

Rejected. Catalog admission requires bounded upstream reads, typed validation, exact
digests, and tests. Browser-side generation would duplicate trusted Rust logic and make
integrity failures harder to control.

### 5. Metadata-only catalog entry

Rejected. A model without a proven executable artifact cannot satisfy RigSpark's
runnable catalog promise and may appear selectable while no backend can run it.

### 6. Automatic conversion or quantization in Actions

Rejected for the MVP. It is expensive, slow, difficult to reproduce, and expands the
supply-chain and artifact-hosting trust boundary.

## Tech stack

- Existing embedded HTML/CSS/JavaScript GUI; no Node.js or TypeScript.
- GitHub issue forms and GitHub Actions.
- Rust 1.98.1 maintenance command using existing workspace dependencies where possible.
- Existing catalog schema, quality evidence, runtime source types, and signed publication
  workflow.

Adding a new runtime dependency, external service, catalog schema field, or dataset
format requires separate approval.

## Project structure

```text
.github/ISSUE_TEMPLATE/model_request.yml       structured request intake
.github/workflows/catalog-request.yml          request orchestration and draft PR
crates/rigspark-cli/src/catalog_request.rs     event parsing and maintenance CLI
crates/rigspark-core/src/                      pure request/admission decisions as appropriate
crates/rigspark-runtime/src/                   injected bounded upstream transports
crates/rigspark-cli/tests/                     CLI and recorded-fixture contracts
crates/rigspark-runtime/tests/                 transport/orchestration contracts
crates/rigspark-gui/static/                    Models button and fallback UX
crates/rigspark-gui/tests/                     static/API/browser contract coverage
```

Exact Rust module boundaries should reuse the existing catalog admission and Hugging Face
transport abstractions rather than duplicate them.

## Commands

```bash
cargo test --locked -p rigspark-cli --test catalog_request_cli
cargo test --locked -p rigspark-runtime --test catalog_requests
cargo test --locked -p rigspark-gui
cargo test --workspace --locked -- --test-threads=2
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --workspace --locked
cargo native-retirement
```

## Testing strategy

### Unit and fixture tests

- canonical URL acceptance and rejection table;
- bounded issue-form parsing from recorded event JSON;
- stable outcome/reason-code serialization;
- duplicate matching across all supported source coordinates;
- public/private/gated/license combinations;
- runnable-artifact eligibility and missing-artifact blocked outcome;
- control-character stripping and bounded status text;
- changed-file allow-list enforcement;
- no mutation of curated entries.

All upstream network behavior uses recorded fixtures and injected transports. Tests never
hit Hugging Face, Ollama, or GitHub.

### Workflow contract tests

- only the intended issue events can run admission;
- label changes made by the bot do not create loops;
- permissions are job-scoped and minimal;
- no `pull_request_target`;
- no untrusted GitHub expression is interpolated into a shell command;
- PRs are draft and no merge/publish step exists;
- data-file allow-list matches the Rust command's possible outputs.

### GUI tests

- button is present in the Models catalog controls;
- accessible name and sign-in/public warning are present;
- URL selects `model_request.yml`;
- browser-open failure exposes a copyable URL;
- existing catalog update behavior is unchanged.

## Boundaries

### Always

- Validate issue input and every upstream response as untrusted.
- Pin executable artifacts by exact revision, SHA-256, and byte size.
- Preserve the honesty gate: unknown remains unknown; missing required facts block.
- Keep advice deterministic and offline.
- Create a draft PR from a dedicated branch with a strict changed-file allow-list.
- Keep the issue as the durable status and audit record.

### Ask first

- Add anonymous intake or any hosted request broker.
- Add a GitHub App, PAT, CAPTCHA provider, or new secret.
- Add runtime dependencies or support a new artifact/backend type.
- Change the catalog schema, evidence format, or memory-store layout.
- Auto-merge a request PR or auto-publish after request admission.

### Never

- Ship a GitHub token or accept one from the user.
- Interpolate issue content into shell commands.
- Execute code, scripts, or workflow definitions from a requested repository.
- Fetch arbitrary URLs supplied in issue text.
- Admit metadata-only or unverified executable weights.
- Push directly to `main`, bypass review, or invoke signing from the request workflow.

## Success criteria

1. The Models view exposes a keyboard-accessible **Request a model** control with clear
   GitHub sign-in and public-request disclosure.
2. A valid request creates one public issue that remains the canonical status URL.
3. Invalid and unsupported requests produce a stable label, bounded explanation, and no
   repository changes.
4. A base-weight-only request remains open as
   `catalog:needs-runnable-artifact`.
5. An eligible recorded fixture produces only allow-listed data changes and one linked
   draft PR.
6. No client credential, maintainer PAT, direct `main` write, auto-merge, or signing
   capability is introduced.
7. Catalog integrity, quality, full workspace tests, Clippy, formatting, build, and
   native-retirement gates pass before merge.
8. Existing catalog update, recommendation, and signed publication behavior is
   unchanged.

## Not doing

- Anonymous submission — requires a public broker and abuse controls.
- Automatic model conversion or quantization — too costly and expands trust scope.
- Metadata-only catalog entries — not runnable and violates the catalog promise.
- Direct merge or publication — request automation proposes; existing controls decide.
- Email notifications — GitHub issue subscription is the status channel.
- Arbitrary model hosts — MVP accepts canonical Hugging Face repository URLs only.

## Open questions before implementation

- Which exact existing Rust admission components can be reused for requested text-model
  artifact discovery without broadening their current source allow-lists?
- Should the first implementation accept both GGUF and MLX requests, or land GGUF as the
  first vertical slice and add MLX after its fixture coverage is complete?
- Which labels must be created in repository settings before enabling the workflow?
- Should merged request PRs close their issue through `Closes #N`, or should a separate
  merge-status workflow apply `catalog:admitted` before closing?
