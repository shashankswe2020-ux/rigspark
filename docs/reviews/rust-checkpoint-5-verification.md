# Rust Checkpoint 5 Verification

Date: 2026-09-18. Status: R20-R22 complete at the checkpoint verification scope.
Feature-branch commits and CI were authorized. No merge, production routing switch,
release, or Electron removal is included.

## Native Entry Points

```sh
cargo run --locked -p rigspark-gui -- --port 59103
cargo run --locked --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo test --locked -p rigspark-gui
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Browser and differential verification still use retained Node tooling:

```sh
npm run rust:gui:e2e
npm run rust:gui-parity
```

Set `RIGSPARK_HOME` to an isolated directory before experimental mutations.
Builds require the pinned Rust toolchain and platform system libraries, not npm.
Marked and DOMPurify are vendored with their licenses and embedded at compile time.
Native CI builds pass before npm dependencies are installed. The retained
`rust:gui:e2e` verification tooling uses a separate Rust example with an injected
model and fresh temporary home; it does not start a real inference process.
Use `RUST_GUI_TEST_PORT` to choose a free browser-test port (default 4322).

## Verification Results

- Workspace fmt, strict Clippy, locked tests and build: passed.
- Tauri fmt, strict Clippy, locked tests and build: passed.
- Tauri command-dispatch tests: approved root picker invocation succeeds;
  artifact path, foreign origin/port, and non-main window are denied.
- Real macOS WebView smoke: frontend, composer, and injected bridge load;
  process exits successfully without starting inference.
- Three Chromium journeys against native HTTP/SSE: passed. Replies persist and
  restore; completion is announced; cancellation saves no partial exchange;
  Escape returns context-picker focus; mobile has no horizontal overflow;
  desktop shows seven metric canvases. Screenshots were inspected.
- Host tests cover asset embedding, artifact sandboxing/traversal, body limits,
  origin/token/listener checks, shutdown admission, session isolation, duplicate
  runs, SSE disconnect cancellation, and tool approval/disconnect.
- GUI recommendation parity: 24 complete model-list contracts. Prior sizing,
  advice, state, and workflow parity gates pass unchanged.
- TypeScript: lint/typecheck/build and all 2,066 tests pass; coverage thresholds
  pass (85.31% statements, 79.34% branches, 81.39% functions, 86.76% lines).

## Review Findings Addressed

- Caller-supplied listeners previously could bind outside the expected loopback
  address. `serve` now validates the exact address and port before accepting work.
- Approval events could reach the browser before the reply slot existed. Slots
  now precede publication and accept one response only.
- Connector disconnect could wait behind an agent awaiting approval. Mutations
  now cancel before waiting on the manager and clear session grants.
- Requests could enter after shutdown. The boundary now returns 503, and cleanup
  runs after server errors as well as ordinary shutdown.
- A Tauri capability containing `http://127.0.0.1:port/` is not path-restricted:
  Tauri normalizes `/` to `*`. The literal-root `/{}` pattern is tested through
  the actual dispatcher. Navigation and command-level window checks remain.

Sources: [Tauri capabilities](https://v2.tauri.app/security/capabilities/),
[Tauri 2.11.5 test API](https://docs.rs/tauri/2.11.5/tauri/test/index.html), and
the pinned tauri-utils 2.9.3 `RemoteUrlPattern::from_str` implementation.
Tauri documents Linux iframe-origin limitations. Artifact responses prohibit
scripts via sandbox CSP; the launch document prohibits framing. No artifact
document is granted the picker capability.

## Completed R22 Gate

Update on 2026-09-19: the native dialog process coordinator is now Rust. Full
macOS/Linux/Windows verification passed at `5e7a842` in run `35442346134`,
including actual Linux/Windows Cancel/select/root/revoke/exit tests. The obsolete
Node coordinator was removed only after this evidence. Browser journey tooling
still requires migration; this does not close R26.

| Check | macOS arm64 | Linux | Windows |
| --- | --- | --- | --- |
| Native build, tests, Clippy | Passed | Passed | Passed |
| Actual WebView launch/bridge | Passed | Passed (WebKitGTK/Xvfb) | Passed (WebView2) |
| Picker IPC with injected selection | Passed | Passed | Passed |
| Physical folder dialog select/cancel | Passed (Accessibility) | Passed (AT-SPI/GTK) | Passed (native IDOK/UI Automation) |
| Browser streaming/cancel/keyboard | Passed (Chromium) | Passed (Chromium) | Passed (Chromium) |

Passing full matrix, commit `36c96fee7ae1bfdbeaf04e4b8efeb61aaa8cf220`:
https://github.com/rigspark/rigspark/actions/runs/35355772910
Jobs: Windows `105634643675`, Linux `105634643698`, macOS `105634643880`.
All native lint/test/build, real WebView smoke, and three browser journeys pass.
Linux and Windows also pass actual Cancel, exact-folder selection, root
registration/revocation, and exit. macOS actual dialog evidence is recorded below.
Artifact IPC denial is exercised through Tauri's dispatcher; artifact sandboxing
is covered at the HTTP boundary. This is not a claim of exhaustive WebView exploit
testing or signed-installer certification.

On 2026-09-18, the user authorized feature-branch commits, pushes, and verification
CI. Commit `f60c3a4` started the three-platform `Rust Desktop Verification` workflow:
https://github.com/rigspark/rigspark/actions/runs/35316992201
The complete native build/lint/test/WebView/browser matrix passed for all three
platforms at commit `59dffd0`:
https://github.com/rigspark/rigspark/actions/runs/35325069584
Platform jobs: macOS `105536111599`, Windows `105536111731`, Linux `105536111831`.

Native execution found and fixed Linux `O_PATH` directory descriptors being used
for fsync/chmod, Windows-only unused mutability, a missing Windows icon resource,
and Unix-only absolute paths in adapter/inference fixtures. Private permissions,
descriptor-relative operations, and identity checks were retained; no test gate
was weakened. Test startup waits now have deadlines and CI collects all failures.

Historical actual-folder-dialog failure at commit `c4f36b3`:
https://github.com/rigspark/rigspark/actions/runs/35340732466
It uses the real picker twice (Cancel, then selection), checks the selected path
against a disposable directory, registers and revokes that root, and requires a
successful exit. Linux uses GTK portal/Xvfb; Windows uses UI Automation. Mock
picker tests are not substituted for this gate.

Previously both platforms opened two real picker dialogs, but Linux never completed the
second selection before the smoke deadline. Windows returned a directory other
than the required disposable target, which the smoke guard rejected before root
registration. These failures were resolved without weakening the selected-path
assertion: smoke-only initial directories and unique titles avoid stale-dialog
targeting; Linux invokes the observed enabled `select` action through AT-SPI;
Windows invokes the actual native confirmation control. This verifies real folder
selection/cancellation, not arbitrary keyboard navigation through filesystem trees.
The driver checks the success marker as well as the process
exit code, so an exit code of zero alone cannot falsely pass the test.

The browser fixture also now waits for session creation and active-session render
before sending chat. Previously an already-empty transcript could satisfy setup
before New Chat finished, allowing its late completion to clear the test reply.

The actual macOS folder dialog was exercised through Accessibility automation:
Browse opened the native sheet; Cancel dismissed it; reopening and selecting a
disposable directory registered a workspace root. Authenticated root revocation
cleared the capability. Closing the native window terminated its process normally.
No inference runtime was started during this check.

R22 closes using the combined native matrix and macOS dialog evidence above.
Signed installers, distribution,
native runtime smoke with real weights, dependency-release audits, and production
cutover remain checkpoint 6. Tauri bundle generation is disabled until those gates.