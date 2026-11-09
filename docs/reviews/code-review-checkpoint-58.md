# Code Review Checkpoint 58: Spec — Desktop App (Tauri)

> **Reviewer:** Code Reviewer Agent (Staff Engineer)
> **Date:** 2026-08-10
> **Scope:** Spec review of `docs/specs/desktop-app-tauri.md` (v0.1) — architectural soundness and completeness before D-series implementation begins
> **Test suite:** 1456/1459 passing (3 pre-existing failures in `ollama-lifecycle.test.ts`, unrelated to this spec); typecheck ✅; build ✅

---

## Verdict: ❌ REQUEST CHANGES

**Overview:** The spec is well-motivated and the macro architecture (Rust lifecycle shell + WebView pointing at the Node GUI server) is sound. However, five issues block safe implementation: OQ3 is answered but §3.4 is not updated to match, creating a direct contradiction in the shutdown path; the `npx` fallback bypasses the binary path validation rules stated in the same spec; the port probe strategy is described using two incompatible mechanisms in two different sections; the CSP `'unsafe-inline'` in `default-src` invalidates the spec's own security claim about inline script execution; and SIGTERM on Windows is unimplemented in the Rust shutdown sequence. These must be resolved before D2 is assigned.

---

## Critical Issues

### C1. OQ3 answer contradicts §3.4 shutdown sequence

- **Section:** §3.4, §15 (OQ3)
- **Problem:** §3.4 states that `on_window_event(CloseRequested)` sends SIGTERM to the child and exits Tauri. OQ3 asks "Should the system tray icon persist after the window is closed?" and the recommended default is "Yes — add quit-only-from-tray behavior." These two behaviors are mutually exclusive: if the window close event triggers SIGTERM and app exit, there is no server to keep running and no tray to show. The spec resolves OQ3 in the table but never propagates the resolution back into §3.4, so implementors will implement graceful exit from the window close event — which is the opposite of what OQ3 intends.
- **Fix:** Update §3.4 to split the shutdown sequence into two paths: (a) `CloseRequested` → hide window, keep server alive, tray remains active; (b) tray "Quit" → SIGTERM → 3 s wait → SIGKILL → exit 0. Remove OQ3 from the open questions list since it has been resolved.

### C2. `npx` fallback bypasses binary path validation and requires shell execution

- **Section:** §3.3 step 5, §12.2, §16 (Boundaries)
- **Problem:** §12.2 requires the binary path to be an absolute path with no shell metacharacters, and §16 states "Never use `shell: true` or string interpolation when spawning the `llmup` binary." The `npx --no-install rigspark` fallback is not an absolute path — it is a command-with-argument string. Spawning `Command::new("npx").arg("--no-install").arg("rigspark")` resolves `npx` via PATH (not an absolute path), bypasses the absolute-path validation rule, and introduces a supply-chain risk: a malicious package named `rigspark` in a locally-configured npm registry would execute on first app launch without any user confirmation. There is no npm integrity check equivalent here. Additionally, `npx` itself can make network calls to resolve the package, violating the deterministic, offline posture of the project.
- **Fix:** Remove the `npx` fallback from §3.3. If the binary is not found via steps 1–4, go directly to the install dialog. Document in §3.3 that `npx` is explicitly excluded as a security boundary. If a future version wants a "download and run" flow, it belongs in a separate D-task with explicit security review.

### C3. Port probe strategy is inconsistent between §3.2 and §3.5

- **Section:** §3.2 step 3, §3.5
- **Problem:** §3.2 step 3 says "Rust probes for a free port starting at 4000" (sequential scan from a base port), while §3.5 says "Rust binds a `TcpListener` on `127.0.0.1:0` to get an OS-assigned free port." These are two different mechanisms. The sequential scan from 4000 is predictable, deterministic, and easy to unit test. The OS-assigned port (`:0`) is non-deterministic. They also produce different security properties: a port-0 approach minimizes the race window but produces an unpredictable port (fine since the URL comes from JSON); a sequential scan from a known base is user-visible (port 4000 is easier to firewall). An implementor reading this spec will need to pick one. §3.5 correctly notes that the race window is tolerable because the actual bound port comes from the JSON output — but §3.2 step 3 still describes a different mechanism that is never reconciled.
- **Fix:** Delete §3.2 step 3's "probe for a free port starting at 4000" language. Replace with "Rust selects a port by binding `TcpListener` on `127.0.0.1:0`, reading the OS-assigned port, closing the listener, and passing the port number to `llmup gui`." Retain §3.5 as the authoritative description. Add a note that `llmup gui`'s JSON output is the source of truth for the URL — the probed port is only a hint.

### C4. CSP `'unsafe-inline'` in `default-src` invalidates the spec's script injection claim

- **Section:** §5.2
- **Problem:** The CSP in §5.2 is:
  ```
  default-src 'self' http://127.0.0.1:* 'unsafe-inline'
  ```
  Because `default-src` is the fallback for all directives including `script-src`, and there is no explicit `script-src` directive, `'unsafe-inline'` applies to scripts. This means inline scripts (e.g., `<script>malicious_code()</script>` in a model response) would execute — directly contradicting the immediately following claim: "`script-src 'self'` — no inline scripts injected by model output can execute." The GUI spec requires model output to be rendered as text nodes (not innerHTML), which is the primary defense, but the CSP is the defence-in-depth layer and it must not lie about what it permits.
- **Fix:** Split the CSP into explicit directives. Add `script-src 'self'` (no `'unsafe-inline'`) and move `'unsafe-inline'` only to `style-src` if inline styles are required:
  ```
  default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src http://127.0.0.1:*; img-src 'self' data:;
  ```
  Remove the incorrect annotation. Confirm with the frontend author whether `'unsafe-inline'` in `style-src` is actually needed (Tauri injects some inline styles for window chrome that may require it).

### C5. SIGTERM is not available on Windows — shutdown sequence is broken on one of three target platforms

- **Section:** §3.4, §8 (Platform targets), §13.4 D2 acceptance criteria
- **Problem:** §3.4 sends SIGTERM to the Node.js child process. On Windows, `TerminateProcess` (the Windows equivalent of SIGKILL, not SIGTERM) is what Rust's `Child::kill()` sends. Node.js on Windows does not receive SIGTERM from a parent process via `kill(pid, SIGTERM)` — the Node.js `process.on('SIGTERM', ...)` handler will never fire. This means the GUI server's graceful shutdown handler (GUI spec G6: "Server stops cleanly on SIGINT without leaving the port open") will not run on Windows. The port will be released by OS when the process dies, but `state.json` may be left inconsistent, open file handles may not flush, and any in-flight SSE streams will be aborted without `done` events.
- **Fix:** Add a platform-specific subsection to §3.4. On Windows: instead of SIGTERM, send a control event (`GenerateConsoleCtrlEvent(CTRL_C_EVENT, pid)`) if the child is in the same process group, or define a Windows-specific soft-shutdown IPC channel (e.g., the Rust backend `POST`s to `/api/shutdown` on the loopback server before killing the process). Document this explicitly. Add a D2 acceptance criterion: "Closing the window on Windows exits the `llmup gui` child cleanly without leaving `state.json` dirty."

---

## Important Issues

### I1. Startup JSON parsing contract is fragile — non-JSON stdout lines not addressed

- **Section:** §3.2 step 5
- **Problem:** Step 5 says Rust "reads stdout until a valid JSON line is received." Node.js startup can emit non-JSON text to stdout before the server is ready: npm deprecation warnings, `ExperimentalWarning` messages, any `console.log` added during debugging. The spec does not say whether Rust should skip non-JSON lines (correct behavior) or fail on the first non-JSON line (catastrophic for users who have a verbose Node install). Without this, implementors will disagree and the behavior will be different across platforms.
- **Fix:** Add to step 5: "Rust reads stdout line-by-line, skipping lines that do not parse as JSON. The first line that parses as valid JSON with a `url` field is accepted. All other lines are written to the desktop app's debug log." Also update §13.1 to add a test for `parse_server_json()` with interleaved non-JSON lines.

### I2. Crash recovery behavior is underspecified

- **Section:** §3.2, §8 (D2 acceptance criteria)
- **Problem:** The D2 acceptance criterion states "Child crash is detected; notification is shown; tray icon dims." It does not specify: (a) whether Rust attempts to respawn the server after a crash and, if so, how many times and with what backoff; (b) what state the window shows (a blank page? an error overlay? the last rendered page?); (c) what happens when the user clicks "Open" in the tray after a crash. Without this, each implementor will make a different choice and the behavior will be inconsistent across versions.
- **Fix:** Add a §3.6 "Crash recovery" subsection specifying: on child exit with non-zero code or unexpected exit, (1) show OS notification "rigspark: server stopped — open to restart", (2) dim tray icon, (3) navigate WebView to an error page (`about:blank` with an injected error message or a bundled `error.html`), (4) no automatic respawn in v1 (user must click tray "Open" to trigger a fresh startup sequence from step 1). Add this as a D4 deliverable since it requires the tray to be wired.

### I3. macOS universal binary CI step is missing the x86_64 Rust toolchain installation

- **Section:** §10.3 (CI workflow)
- **Problem:** The CI matrix uses `macos-14` (Apple Silicon) to build `universal-apple-darwin`. A universal binary requires both `aarch64-apple-darwin` and `x86_64-apple-darwin` targets. The `dtolnay/rust-toolchain@stable` step adds `targets: ${{ matrix.target }}` where `matrix.target` is `universal-apple-darwin`. `universal-apple-darwin` is not a valid Rust target triple — it is a Tauri-internal label. Tauri's build system handles cross-compilation internally, but it requires both toolchain targets to be installed via `rustup target add aarch64-apple-darwin x86_64-apple-darwin`. The developer machine setup in §10.1 documents this, but the CI step does not. The build will fail at the x86_64 lipo step.
- **Fix:** Add an explicit `rustup target add aarch64-apple-darwin x86_64-apple-darwin` step in the CI workflow for the macOS build, or pass `targets: aarch64-apple-darwin x86_64-apple-darwin` to `dtolnay/rust-toolchain`. Validate the final `lipo` output in the CI smoke test.

### I4. Tauri v2 shell scope for `llmup` binary is not specified

- **Section:** §5.3 (Capability allowlist)
- **Problem:** The capability grants `shell:allow-execute` without specifying the shell plugin scope. In Tauri v2, `shell:allow-execute` requires a corresponding `allowlist` scope in `tauri.conf.json` (or a plugin permission scope file) that names which programs are permitted. Without the scope, Tauri v2 defaults to blocking all spawning. The spec leaves the scope definition absent, which means either (a) the CI build will fail because the permission is incomplete, or (b) the implementor adds `shell:allow-execute-all` to make it work quickly, which allows the WebView to spawn arbitrary programs — defeating the containment claim.
- **Fix:** Add a `scope` block to §5.3 showing the exact Tauri v2 shell permission scope that restricts execution to the `llmup` binary pattern. Example: `"shell:allow-execute": { "allow": [{"name": "llmup", "cmd": "llmup", "args": true}] }`. Reference the Tauri v2 shell plugin documentation's scope format.

### I5. OQ2 resolution ("Add to D3") not reflected in D3 deliverables

- **Section:** §14 (D3), §15 (OQ2)
- **Problem:** OQ2 asks whether the window should remember its position and size. The recommended default is "Yes — Tauri `window-state` plugin saves it automatically. Add to D3." D3's deliverables list does not mention the `window-state` plugin, its Cargo dependency, or the `tauri.conf.json` plugin registration required. This is a resolved open question whose resolution was not propagated into the implementation plan.
- **Fix:** Add to D3 deliverables: "Register `tauri-plugin-window-state` in `Cargo.toml` and `tauri.conf.json`; call `app.handle().plugin(tauri_plugin_window_state::Builder::default().build())?` in `lib.rs`; add the `window-state:default` capability permission." Remove OQ2 from the open questions list.

### I6. `TAURI_SIGNING_PUBLIC_KEY` must be embedded at build time but no CI step provides it

- **Section:** §9 (Auto-update), §10.3 (CI workflow)
- **Problem:** The updater config in §9 shows `"pubkey": "{{TAURI_SIGNING_PUBLIC_KEY}}"` — a template variable that must be resolved at build time. The CI workflow in §10.3 passes `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` as env vars, but no `TAURI_SIGNING_PUBLIC_KEY`. Tauri's updater derives the embedded public key from the private key during the build via `tauri signer sign`. Without the public key present in `tauri.conf.json` (or as an env var that Tauri reads), the updater plugin will either embed an empty key (silent failure on update) or fail the build.
- **Fix:** Add `TAURI_SIGNING_PUBLIC_KEY: ${{ secrets.TAURI_SIGNING_PUBLIC_KEY }}` to the CI env block in §10.3. Add a step in the D5 task to generate the key pair and store both the private key (as a secret) and the public key (as a non-secret env var or baked into `tauri.conf.json`) in the GitHub repository settings.

### I7. `tauri dev` mode workflow is underspecified — collision with user-run `llmup gui`

- **Section:** §10.2 (Local build commands)
- **Problem:** §10.2 describes `npm --prefix desktop run tauri dev` as "hot-reload WebView against running llmup gui." It does not specify whether Tauri dev mode spawns its own `llmup gui` child process (via the lifecycle in `lifecycle.rs`) or expects the developer to already have one running. If `tauri dev` spawns a child and the developer also has `llmup gui` running on port 4000 (the default), the child process will fail to bind and the error dialog appears on every `tauri dev` launch. If `tauri dev` does not spawn a child, it needs a dev-mode flag or env var to point the WebView at an already-running server URL — which is not defined.
- **Fix:** Add a §10.4 "Development workflow" subsection specifying: in dev mode, `LLMUP_URL=http://127.0.0.1:4000` (or similar) env var skips binary discovery and server spawn, pointing the WebView directly at the provided URL. This is the standard pattern for Tauri apps wrapping an external server. Document the two-terminal workflow: terminal 1: `llmup gui --port 4000 --no-open`; terminal 2: `LLMUP_URL=http://127.0.0.1:4000 npm --prefix desktop run tauri dev`.

---

## Suggestions

### S1. Update endpoint URL in §9 exposes a specific GitHub username — use a canonical placeholder

- **Section:** §9 (Auto-update endpoints)
- **Problem:** The `endpoints` URL is `https://github.com/rigspark/rigspark/releases/...`. This appears to be a personal fork URL left from a draft. If this is not the canonical repository, it will silently route update checks to a different (possibly abandoned or malicious) repository. Even if it is the correct URL, hardcoding a personal GitHub username in a spec is fragile against repository transfers.
- **Fix:** Replace with `https://github.com/<canonical-owner>/rigspark/releases/latest/download/latest.json` and resolve the canonical owner before D5.

### S2. Binary path allowlist does not check file ownership or world-writable bit

- **Section:** §12.2
- **Problem:** The binary path validation checks for absolute path, executable bit, and no shell metacharacters. It does not check whether the binary file is world-writable (an attacker with filesystem write access could replace the binary after validation). On multi-user macOS/Linux systems, this is a privilege escalation vector when running as a non-root user.
- **Fix:** Add to §12.2: "The binary's parent directory must not be world-writable (`(metadata.permissions().mode() & 0o002) == 0` on Unix). If world-writable, reject with `AppError::BinaryUntrusted`." Note this is not applicable on Windows.

### S3. "Error splash Retry" behavior conflicts with the "install dialog" for binary not found

- **Section:** §3.3, §14 (D3 deliverables)
- **Problem:** §3.3 specifies a friendly "Install rigspark first" dialog on binary discovery failure. D3 specifies a separate "Error splash with Retry and Quit buttons" for startup failure. It is unclear whether binary-not-found hits the install dialog or the error splash, and what "Retry" means in each context (re-run discovery from step 1? re-spawn with the same binary?).
- **Fix:** Clarify the error handling tree: binary-not-found → install dialog (with copy-npm-command button, no Retry); server-start-failure (JSON timeout, bind failure) → error splash (with Retry triggering full restart from step 1, and Quit). These are distinct failure modes with distinct UX.

### S4. D3 missing deliverable: Vitest config setup for `desktop/src/tests/`

- **Section:** §14 (D3), §13.2
- **Problem:** §13.2 describes TypeScript IPC tests located in `desktop/src/tests/` using Vitest with mocked Tauri APIs. D3's deliverables say "TypeScript frontend tests (Vitest, mocked `@tauri-apps/api/core`)" but do not include a deliverable for creating the `vitest.config.ts` in `desktop/`, configuring the `desktop/package.json` test script, or adding the Tauri API mock to the test setup file.
- **Fix:** Add to D3 deliverables: "`desktop/vitest.config.ts` with `@tauri-apps/api/core` aliased to `desktop/src/tests/mocks/tauri-api.ts`; `desktop/package.json` `test` script: `vitest run`."

### S5. `--json` contract should pin the GUI spec version it depends on

- **Section:** §3.2 step 5, §1 (Assumptions §6)
- **Problem:** The startup sequence depends on `llmup gui --json` outputting `{"url":..., "harness":..., "port":...}`. This is defined in the GUI spec §3.1. If the GUI spec's JSON format changes in a future G-task (e.g., adding a required field or renaming `url`), the Tauri parser will silently fail to find the `url` field and show the error dialog. There is no pinning or version assertion in the contract.
- **Fix:** Add a note to §3.2 step 5: "The JSON schema is defined in gui-and-harness-adapters.md §3.1. The Rust parser uses `serde_json` with `#[serde(deny_unknown_fields = false)]` — unknown fields are ignored; `url` is the only required field." This ensures forward compatibility. Add a Rust unit test that verifies future additions to the JSON don't break parsing.

---

## What's Done Well

- **Separation of concerns is clean and principled.** Rust does exactly three things: discover, spawn, relay the URL. All LLM logic stays in TypeScript. The IPC surface (four commands) is minimal and well-scoped.
- **Security-first defaults throughout.** `Command::new(path).arg(...)` (no shell), loopback-only CSP `connect-src`, capability allowlist, Ed25519 signed updates, path validation before spawn, no credentials in source — these are all correct and enforced at the spec level, not just mentioned.
- **Tauri v2 capability model is used correctly.** The spec uses the v2 `capabilities` array format and named permission identifiers (`core:default`, `shell:allow-execute`, `notification:default`) rather than the v1 `allowlist` map. This is the right target.
- **Dependency graph in §14 is precise.** G1–G6 must be done before D1, D1→D2→D3→D4→D5→D6 is the right linear order since each phase depends on the previous layer's tested output. This prevents parallel-implementation confusion.
- **Acceptance criteria per D-task are detailed and testable.** The D1–D6 checklists cover binary discovery paths, lifecycle events, IPC contracts, platform installers, and update integrity — all verifiable without manual inspection.
- **No bundled runtime is correct for v1.** Deferring self-contained binary to D2/Node SEA avoids cross-compilation complexity and keeps the first release buildable with a single npm install.
- **`#[must_use]` and no `unwrap()` in Rust conventions (§11.1)** are exactly right for a Tauri backend. These are often omitted in first drafts and cause silent error swallowing.

---

## Verification Story

| Check                | Status | Notes                                                                                               |
| -------------------- | ------ | --------------------------------------------------------------------------------------------------- |
| Tests reviewed       | ⚠️     | 1456/1459 passing; 3 pre-existing failures in `ollama-lifecycle.test.ts` unrelated to this spec    |
| Build verified       | ✅     | `npm run build` and `npm run typecheck` pass cleanly                                                |
| Security checked     | ❌     | C2 (npx supply chain), C4 (CSP unsafe-inline), C5 (SIGTERM on Windows) block security sign-off     |
| Spec completeness    | ❌     | C1 (OQ3 contradiction), C3 (port probe inconsistency) block unambiguous implementation             |

---

## Action Items

| #  | Priority  | Issue                                                             | Target                       |
|----|-----------|-------------------------------------------------------------------|------------------------------|
| 1  | Critical  | OQ3 resolved but §3.4 not updated — window close kills server     | Fix spec before D2           |
| 2  | Critical  | `npx` fallback bypasses path validation and introduces supply chain risk | Remove from §3.3 before D2 |
| 3  | Critical  | Port probe mechanism described inconsistently (§3.2 vs §3.5)     | Fix spec before D2           |
| 4  | Critical  | CSP `unsafe-inline` in `default-src` allows inline scripts        | Fix §5.2 before D1           |
| 5  | Critical  | SIGTERM undefined on Windows — graceful shutdown broken           | Add Windows path to §3.4 before D2 |
| 6  | Important | Non-JSON stdout lines during startup not handled                  | Fix §3.2 step 5 before D2   |
| 7  | Important | Crash recovery behavior unspecified (respawn? UX?)               | Add §3.6 before D4           |
| 8  | Important | macOS universal build CI missing x86_64 toolchain install         | Fix §10.3 before D6          |
| 9  | Important | Tauri v2 shell scope not specified — allow-execute-all risk       | Fix §5.3 before D1           |
| 10 | Important | OQ2 ("Add to D3") not in D3 deliverables                         | Update §14 D3 before D3      |
| 11 | Important | `TAURI_SIGNING_PUBLIC_KEY` not in CI env — update will be broken  | Fix §10.3 + §9 before D5    |
| 12 | Important | `tauri dev` workflow underspecified — port collision risk          | Add §10.4 before D1         |
| 13 | Suggestion | Update endpoint URL exposes personal GitHub username              | Fix before D5                |
| 14 | Suggestion | Binary path validation missing world-writable check               | Add to §12.2 before D2      |
| 15 | Suggestion | Error splash "Retry" vs install dialog behaviors conflict         | Clarify in §3.3 + D3 before D3 |
| 16 | Suggestion | D3 missing Vitest config setup deliverable                        | Update §14 D3 before D3      |
| 17 | Suggestion | `--json` contract not pinned to GUI spec version                  | Add note to §3.2 before D2  |
