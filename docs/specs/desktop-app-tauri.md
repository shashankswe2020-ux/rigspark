# Spec: Desktop App (Tauri)

> Status: **Draft (v0.1)** — pending sub-agent review and human approval.
> Last updated: 2026-10-30
> Implementation: macOS packaging (`.app`/`.dmg`, unsigned), menu-bar tray with close-to-hide, launch at login (`tauri-plugin-autostart`) and signature-verified updates (`tauri-plugin-updater`, `latest.json` on GitHub releases) ship in `apps/desktop/src-tauri`. Notarization, Windows and Linux installers remain pending.
> Related: [gui-and-harness-adapters.md](./gui-and-harness-adapters.md),
> [rigspark.md](./rigspark.md),
> [terminal-user-interface.md](./terminal-user-interface.md)
> Prerequisite: GUI spec ([gui-and-harness-adapters.md](./gui-and-harness-adapters.md))
> must be approved and G1–G6 must be implemented before desktop work begins.

---

## 0. Why Tauri, not Electron

| Concern | Electron | Tauri |
|---|---|---|
| Bundle size | ~120 MB (ships full Chromium) | ~6–12 MB (uses OS WebView) |
| RAM at idle | ~200–400 MB | ~20–60 MB |
| Startup time | 3–8 s (cold) | < 1 s |
| Backend language | Node.js (same runtime) | Rust (compile-time safe, zero GC) |
| Dependency count | Enormous (node_modules + Chromium) | Rust std + Tauri crates only |
| Code signing | Complex, per-platform | Native per-platform support |
| Security model | Node.js process injection risk | OS sandbox + CSP + capability allowlist |
| Auto-update | electron-updater (npm dep) | tauri-updater (built-in) |

The rigspark project values minimal footprint and security. Tauri aligns with
those values. Electron would add a ~100 MB Chromium download for every user —
larger than every model weight considered "tiny" in the catalog.

---

## 1. Assumptions and decisions pending approval

1. **Tauri v2** (latest stable) is the target. Tauri v1 is EOL.
2. **Sidecar architecture**: The Tauri Rust backend discovers and spawns the
   installed `llmup` CLI binary as a managed child process using Tauri's
   `sidecar` or platform-native child spawn, then points the embedded WebView at
   the loopback GUI server URL.
3. **Phase D1 (v1 of this spec)**: The desktop app **requires `rigspark`
   to be installed** (via npm or the npm package). It discovers the binary at
   runtime — it does NOT bundle Node.js or compile a standalone binary. This
   trades self-containedness for build simplicity and eliminates cross-compilation
   risk.
4. **Phase D2 (future spec)**: Self-contained binary using Node.js SEA (requires
   Node ≥ 20 stable) or a bundled minimal Node runtime. Deferred until D1 is
   validated.
5. **WebView is the UI layer**: The frontend is exactly the same HTML/CSS/JS
  served by `src/gui/static/` (from the GUI spec). No duplicate UI code. Once
  the WebView navigates to `http://127.0.0.1:<port>`, the Node GUI server owns
  document CSP enforcement and must send the policy in §5.2 as an HTTP
  `Content-Security-Policy` response header.
6. **IPC is minimal**: The Tauri Rust layer only does lifecycle management
   (discover binary, start server, open window at server URL, stop server on
   quit). All LLM routing, memory, harness, and backend logic remain in
   TypeScript/Node.js.
7. **No new runtime npm dependencies** for the desktop app. The Tauri build
   adds only Rust/Cargo dependencies (dev-time only). Node.js runtime deps
   stay at `cac`, `zod`, `systeminformation`.
8. **Platform targets**: macOS (arm64 + x86_64 universal), Linux (x86_64 AppImage
   + .deb), Windows (x86_64 NSIS installer + MSI). Windows ARM deferred.
9. **Code signing**: Spec defines the signing *contract* but defers the *secret
   wiring* (certificates, notarization tokens) to a shipping spec. Implementation
   must not hardcode credentials.
10. **Auto-update**: Built-in Tauri updater pointing to GitHub Releases. Update
    manifest served from the existing GitHub repository. Enabled but update
    delivery is out of scope for D1 (infrastructure TBD).

---

## 2. Objective

Provide a native desktop application for macOS, Linux, and Windows that wraps
`rigspark`'s browser GUI chat interface in a polished OS-native window — with
a system tray icon, native menus, automatic server lifecycle, and a dramatically
smaller footprint than an Electron equivalent.

### What the desktop app adds over `llmup gui` in a browser tab

| Feature | `llmup gui` (browser) | Desktop app |
|---|---|---|
| No manual terminal command needed | ✗ (must run CLI first) | ✓ (app manages lifecycle) |
| System tray icon + quick-open | ✗ | ✓ |
| Native window chrome + menus | ✗ | ✓ |
| App icon in dock/taskbar | ✗ | ✓ |
| OS notification on model ready | ✗ | ✓ |
| Distributable installer | ✗ | ✓ |
| Auto-update | ✗ | ✓ |
| Isolated from browser history/extensions | ✗ | ✓ |

### Target users

- Non-technical users who want a "just open the app" experience without a terminal.
- Developers who want the GUI always available without managing a background process.
- Users on macOS who want Spotlight launch + dock presence.
- Teams distributing `rigspark` to colleagues who should not need npm.

### Non-goals (v1 / Phase D1)

- Bundled/self-contained binary (deferred to D2).
- Multi-window, floating-panel, or picture-in-picture mode.
- Mobile (iOS/Android) or watchOS.
- Plugin marketplace or extension system.
- Custom model management UI (users use CLI for `up`, `down`, `switch`).
- Rewriting any TypeScript business logic in Rust.
- Telemetry, analytics, or usage reporting.
- Cross-compilation from a single build host (each platform builds on its own runner).

---

## 3. Architecture

### 3.1 Component map

```
┌─────────────────────────────────────────────────────────┐
│  Desktop App (Tauri v2)                                  │
│                                                          │
│  ┌───────────────┐     ┌────────────────────────────┐   │
│  │  Rust backend  │     │  WebView (OS-native)        │   │
│  │  src-tauri/    │     │  → http://127.0.0.1:<port> │   │
│  │                │     │                            │   │
│  │  - Lifecycle   │     │  Same HTML/CSS/JS          │   │
│  │    (spawn/stop │     │  as browser GUI            │   │
│  │    llmup gui)  │     │  (src/gui/static/)         │   │
│  │  - System tray │     │                            │   │
│  │  - Menus       │◄───►│  Tauri IPC (invoke)        │   │
│  │  - OS notify   │     │  for lifecycle only        │   │
│  │  - Auto-update │     │                            │   │
│  └───────┬───────┘     └────────────────────────────┘   │
│          │                                               │
│          │ child_process                                 │
│          ▼                                               │
│  ┌──────────────────────┐                               │
│  │  llmup CLI (Node.js) │  ← discovered at runtime     │
│  │  `llmup gui          │    from PATH or config        │
│  │    --port <auto>     │                               │
│  │    --no-open         │                               │
│  │    --json`           │                               │
│  └──────────────────────┘                               │
└─────────────────────────────────────────────────────────┘
```

The Tauri `security.csp` setting is injected into bundled app HTML only; it
does not protect the HTML document served from the loopback URL. The Node GUI
server therefore owns CSP enforcement for the navigated application document.
The GUI implementation prerequisite must provide the policy defined in §5.2
before desktop work begins.

### 3.2 Startup sequence

1. Tauri `setup()` hook runs.
2. Rust discovers the `llmup` binary (§3.3).
3. Rust probes for a free port starting at `4000`.
4. Rust spawns `llmup gui --port <port> --no-open --json` as a child process.
5. Rust reads stdout until a valid JSON line `{"url": "http://127.0.0.1:<port>", ...}` is received (5 s timeout).
6. The ready GUI server serves its application document with the §5.2 HTTP
  `Content-Security-Policy` response header.
7. Rust opens the main window at the parsed URL.
8. On parse failure or timeout → show error dialog + exit 1.

### 3.3 Binary discovery (Rust)

The `llmup` binary is located by checking in order:

1. `LLMUP_BIN` environment variable (test seam).
2. `~/.rigspark/bin/llmup` (future managed install path).
3. `which llmup` / `where llmup` (PATH lookup via `std::process::Command`).
4. Common homebrew paths: `/opt/homebrew/bin/llmup`, `/usr/local/bin/llmup`.

If steps 1–4 fail, transition directly to a friendly "Install rigspark first"
dialog with the npm install command, a copy button, and a link to the README.

`npx` is explicitly excluded from discovery: it is not a validated absolute
binary path and may resolve or download packages, introducing network and
supply-chain behavior at this security boundary. Any download-and-run flow is
out of scope and requires a separate security review before specification or
implementation.

### 3.4 Shutdown sequence

1. User closes the window or quits via menu/tray.
2. Tauri `on_window_event(CloseRequested)` fires.
3. Rust sends SIGTERM to the child `llmup gui` process.
4. Rust waits up to 3 s for the child to exit.
5. If still alive → SIGKILL.
6. Tauri exits 0.

System tray "quit" follows the same sequence.

### 3.5 Port management

- Auto-select: Rust binds a `TcpListener` on `127.0.0.1:0` to get an OS-assigned
  free port, then closes the listener and passes that port to `llmup gui`.
- Race window (bind → spawn) is tolerable: the `llmup gui` server reports its
  actual bound port in JSON; the Rust backend uses that URL, not the probed port.
- If `llmup gui` fails to bind → exit code 1 is captured → error dialog.

---

## 4. Project Structure

```
desktop/                        ← new top-level directory (not in src/)
  package.json                  ← Tauri CLI + frontend dev tooling only
  package-lock.json
  index.html                    ← thin shell; WebView src set at runtime to server URL
  src/
    main.ts                     ← Tauri frontend JS (IPC invoke wrappers only)
    styles.css                  ← desktop-specific overrides (title bar, tray hint)
  src-tauri/
    Cargo.toml
    Cargo.lock
    tauri.conf.json             ← app metadata, window config, CSP, allowlist
    build.rs
    icons/                      ← all required icon sizes (generated from master SVG)
      32x32.png
      128x128.png
      128x128@2x.png
      icon.icns
      icon.ico
    src/
      lib.rs                    ← Tauri app setup, plugin registration
      main.rs                   ← binary entry point
      lifecycle.rs              ← binary discovery, spawn, port probe, shutdown
      tray.rs                   ← system tray menu
      notify.rs                 ← OS notification helpers
      commands.rs               ← #[tauri::command] IPC handlers
      error.rs                  ← AppError type, serialization for IPC

.github/
  workflows/
    desktop-build.yml           ← matrix: macos-14 (arm64), macos-13 (x86_64),
                                   ubuntu-22.04, windows-2022

docs/specs/
  desktop-app-tauri.md          ← this file
```

The `desktop/` directory is fully self-contained. It shares no npm workspace with
the root `package.json`. The root `package.json` gains one optional script:

```json
"build:desktop": "npm --prefix desktop run tauri build"
```

---

## 5. Tauri Configuration (`tauri.conf.json`)

### 5.1 Window

```json
{
  "windows": [{
    "label": "main",
    "title": "rigspark",
    "width": 1024,
    "height": 768,
    "minWidth": 600,
    "minHeight": 500,
    "center": true,
    "resizable": true,
    "decorations": true,
    "url": "about:blank"
  }]
}
```

The initial URL is `about:blank`; the Rust setup hook navigates to the server
URL after the server starts (via `window.navigate(url)`).

### 5.2 Security / CSP

The canonical desktop policy is:

```text
default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src http://127.0.0.1:*; img-src 'self' data:; font-src 'self'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'; form-action 'none'
```

It must be enforced at both document boundaries:

1. Tauri `security.csp` must use this policy for bundled app HTML.
2. Every loopback GUI server response that serves the application HTML document
   must return this same policy in the HTTP `Content-Security-Policy` header.
   A `<meta http-equiv>` element is not an acceptable substitute.

```json
{
  "security": {
    "csp": "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src http://127.0.0.1:*; img-src 'self' data:; font-src 'self'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'; form-action 'none'"
  }
}
```

- `default-src 'none'` fails closed for any resource type without an explicit
  directive.
- `connect-src` is restricted to `http://127.0.0.1:*` — no external network
  from the WebView.
- `script-src 'self'` permits only same-origin script files; inline scripts and
  string-to-code evaluation are not permitted.
- Inline styles remain allowed for the existing UI. `frame-src 'none'` prevents
  the document from loading child frames, while `frame-ancestors 'none'`
  prevents any parent from embedding the document. Base URL changes, plugins,
  and form submissions are also disabled.
- Model-generated content is rendered as text, not HTML (enforced by the JS UI
  layer from the GUI spec).

### 5.3 Capability allowlist (Tauri v2)

```json
{
  "capabilities": [{
    "identifier": "main-capability",
    "description": "Lifecycle IPC only",
    "windows": ["main"],
    "permissions": [
      "core:default",
      "shell:allow-execute",
      "notification:default"
    ]
  }]
}
```

`shell:allow-execute` is **scoped** to the discovered, validated absolute
`llmup` binary path only — Tauri v2's shell scope prevents the WebView from
spawning arbitrary processes.

---

## 6. IPC Commands (`desktop/src-tauri/src/commands.rs`)

Only four IPC commands are exposed to the WebView. All business logic lives in
the Node process, not here.

```rust
// Returns the server URL once the GUI server is ready, or an error string.
#[tauri::command]
async fn get_server_url(state: State<'_, AppState>) -> Result<String, String>

// Returns the discovered llmup binary path (for display in the UI).
#[tauri::command]
async fn get_binary_path(state: State<'_, AppState>) -> Result<String, String>

// Triggers a graceful shutdown of the GUI server child process.
#[tauri::command]
async fn shutdown_server(state: State<'_, AppState>) -> Result<(), String>

// Returns the current version string.
#[tauri::command]
fn get_version() -> String
```

The frontend JavaScript (`desktop/src/main.ts`) calls these via:

```typescript
import { invoke } from "@tauri-apps/api/core";
const url = await invoke<string>("get_server_url");
```

All Tauri IPC responses are serialized as JSON. Errors are typed `Result<T, String>`
so the frontend can handle them in the `catch` path of `invoke()`.

---

## 7. System Tray (`desktop/src-tauri/src/tray.rs`)

Tray menu items:

| Label | Action |
|---|---|
| **rigspark** (title, non-clickable) | — |
| Open | Navigate window to server URL, bring to front |
| Status | Show OS notification with harness + model |
| Separator | — |
| Quit | Shutdown server → exit |

Tray icon: filled on app start, dimmed (desaturated) when server is not yet
ready. The icon uses the same SVG as the marketing site, resized per platform
requirements.

---

## 8. OS Notifications

Notifications are sent (via Tauri `notification` plugin) for:

| Event | Notification |
|---|---|
| Model server ready | "rigspark ready — <model> on <endpoint>" |
| Model server stopped unexpectedly | "rigspark: server stopped — open to restart" |
| Update available | "rigspark update available — click to install" |

Notifications are suppressed if the window is focused (avoid double feedback).

---

## 9. Auto-Update

Tauri's built-in updater plugin is configured to poll the GitHub Releases endpoint:

```json
{
  "plugins": {
    "updater": {
      "endpoints": [
        "https://github.com/shashankswe2020-ux/rigspark/releases/latest/download/latest.json"
      ],
      "dialog": true,
      "pubkey": "{{TAURI_SIGNING_PUBLIC_KEY}}"
    }
  }
}
```

- `pubkey` is injected at build time via env var — never hardcoded.
- Update check fires once on startup (after server is ready) and once per 24 h.
- User is shown a dialog; update is opt-in.
- The `latest.json` update manifest is generated by the CI workflow and uploaded
  as a release asset.

---

## 10. Build Pipeline

### 10.1 Prerequisites (developer machine)

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin  # macOS universal

# System deps (Linux only)
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev

# Tauri CLI
npm install --prefix desktop
```

### 10.2 Local build commands

```bash
# Development (hot-reload WebView against running llmup gui)
npm --prefix desktop run tauri dev

# Production build for current platform
npm --prefix desktop run tauri build

# macOS universal binary (arm64 + x86_64)
npm --prefix desktop run tauri build -- --target universal-apple-darwin
```

### 10.3 CI workflow (`.github/workflows/desktop-build.yml`)

```yaml
name: Desktop Build
on:
  push:
    tags: ['v*']
  workflow_dispatch:

permissions:
  contents: write

jobs:
  build:
    strategy:
      matrix:
        include:
          - platform: macos-14
            target: universal-apple-darwin
            artifact: '*.dmg'
          - platform: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
            artifact: '*.AppImage *.deb'
          - platform: windows-2022
            target: x86_64-pc-windows-msvc
            artifact: '*.msi *.exe'

    runs-on: ${{ matrix.platform }}
    steps:
      - uses: actions/checkout@<SHA>
      - uses: actions/setup-node@<SHA>
        with: { node-version: '22' }
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Install Linux deps
        if: matrix.platform == 'ubuntu-22.04'
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev
      - name: Install frontend deps
        run: npm ci --prefix desktop
      - name: Build
        run: npm --prefix desktop run tauri build -- --target ${{ matrix.target }}
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
      - name: Upload artifacts
        uses: actions/upload-artifact@<SHA>
        with:
          name: desktop-${{ matrix.platform }}
          path: desktop/src-tauri/target/*/release/bundle/${{ matrix.artifact }}
```

All `uses:` refs must be pinned to full 40-character SHA before merging (enforced
by `tests/workflows/workflow-policy.test.ts`).

---

## 11. Code Conventions

### 11.1 Rust

- `rustfmt` + `clippy` — enforced in CI (`cargo fmt --check`, `cargo clippy -- -D warnings`).
- `#[must_use]` on all functions that return `Result`.
- No `unwrap()` in production code — all errors propagate via `?` or are
  converted to user-facing strings via `AppError`.
- `Arc<Mutex<AppState>>` for shared mutable state across async commands.
- No business logic in `commands.rs` — lifecycle logic lives in `lifecycle.rs`.

### 11.2 TypeScript (desktop frontend)

- Same conventions as root project (kebab-case files, named exports, no `any`).
- `desktop/src/main.ts` only: thin IPC wrappers + DOM manipulation for the
  loading/error splash. No business logic.
- ESLint config extends root `eslint.config.js`.

---

## 12. Security Design

### 12.1 WebView isolation

- **CSP** is enforced independently for both document sources: Tauri injects
  the canonical policy into bundled app HTML, and the Node GUI server returns
  the same policy as an HTTP response header for its application HTML. This is
  required because Tauri's injected policy does not protect the document after
  navigation to `http://127.0.0.1:<port>`.
- **Connection policy** restricts `connect-src` to `127.0.0.1` — the WebView
  cannot reach cloud APIs directly. All cloud harness calls go through the
  Node.js server.
- **Script policy** is explicitly `script-src 'self'`; it does not inherit from
  `default-src`, permit inline scripts, or permit `eval()` / `Function()` in the
  WebView context.
- **Frame policy** uses `frame-src 'none'` to prohibit child frames and
  `frame-ancestors 'none'` to prohibit the GUI document from being embedded.
- **Tauri capability allowlist** restricts IPC to the four declared commands.
  The WebView cannot invoke shell, fs, or any other Tauri API not in the
  allowlist.
- **Content from model output** is rendered as text nodes (from the GUI spec) —
  injection into `innerHTML` is prohibited.

### 12.2 Binary execution

- The `llmup` binary path is validated before spawn:
  - On Windows, the first validation step rejects any raw path beginning with
    two backslashes (`\\`) unconditionally as a UNC path. This guard returns a
    typed binary-path validation `AppError` before path canonicalization, any
    filesystem metadata/existence lookup, or any other validation, so the app
    never resolves a network share.
  - Must be an absolute path (no relative path traversal).
    Absolute Windows drive paths such as `C:\Program Files\llmup\llmup.exe`
    remain permitted subject to all other checks.
  - Must exist and be executable (`std::fs::metadata` check).
  - Must not contain shell metacharacters (strict allowlist: `[a-zA-Z0-9._/\\ :-]`).
- Package runners such as `npx` are not discovery or execution fallbacks; they
  do not satisfy the validated absolute-path requirement and may perform
  package resolution or network downloads.
- The port argument is range-validated (1–65535) before being passed.
- No shell interpolation — the binary is spawned with `Command::new(path).arg(...)`
  directly (no `sh -c`).

### 12.3 Auto-update integrity

- All update artifacts are signed with `TAURI_SIGNING_PRIVATE_KEY` (Ed25519).
- The public key is embedded in the app at build time — updates that fail
  signature verification are rejected.
- The update endpoint is HTTPS only (Tauri enforces this).

### 12.4 Code signing

| Platform | Mechanism |
|---|---|
| macOS | `codesign` + Hardened Runtime + `notarytool` notarization |
| Windows | Authenticode signing via `signtool` |
| Linux | GPG-signed `.deb`; AppImage is unsigned in v1 |

Signing certificates and keys are stored as GitHub Actions secrets — never in
source control.

---

## 13. Testing Strategy

### 13.1 Rust unit tests

Located in `desktop/src-tauri/src/*.rs` as `#[cfg(test)]` modules.

Covered:
- `lifecycle::discover_binary()` — PATH lookup, env override, fallback order.
- `lifecycle::probe_free_port()` — returns a valid port.
- `lifecycle::parse_server_json()` — valid JSON, malformed JSON, timeout.
- `commands::get_version()` — returns a semver string.
- Binary path validation — Windows UNC rejection before filesystem lookup,
  metacharacter rejection, non-absolute rejection.

### 13.2 TypeScript IPC tests (desktop frontend)

Located in `desktop/src/tests/`.

Covered:
- `invoke("get_server_url")` happy path — resolves with `http://127.0.0.1:<port>`.
- `invoke("get_server_url")` error path — rejects with a typed error string.
- Loading/error splash DOM mutations.

These use Vitest with the Tauri API mocked (`vi.mock("@tauri-apps/api/core")`).

### 13.3 Integration smoke tests

A separate `desktop-smoke.test.ts` (run only in CI, gated by `TAURI_SMOKE=1`):
- Launches the app binary with `LLMUP_BIN=<fake-echo-server>`.
- Asserts the window title is "rigspark".
- Asserts the tray icon is registered.
- Fetches the loopback application document and asserts its HTTP
  `Content-Security-Policy` response header exactly matches the canonical §5.2
  policy, including `frame-ancestors 'none'`.
- After the WebView navigates to the loopback application document, attempts
  `eval()`, `Function()`, and inline-script injection and asserts the effective
  response-header CSP blocks every payload without executing it.
- Sends SIGTERM and asserts clean exit.

These require a display server (Xvfb on Linux CI).

### 13.4 Acceptance criteria

**D1 — Binary discovery:**
- [ ] App starts successfully when `llmup` is on PATH.
- [ ] App transitions directly to the install dialog when lookup steps 1–4 fail
  (no PATH, no homebrew, no env), without invoking `npx` or another package runner.
- [ ] `LLMUP_BIN=/path/to/custom/binary` overrides all discovery.

**D2 — Lifecycle:**
- [ ] `llmup gui --port <N> --no-open --json` is spawned on startup (no shell).
- [ ] JSON line is parsed; WebView navigates to the URL.
- [ ] Closing the window sends SIGTERM to the child and exits cleanly.
- [ ] Child crash is detected; notification is shown; tray icon dims.

**D3 — IPC:**
- [ ] `get_server_url` returns before the window is shown (blocks until ready).
- [ ] `shutdown_server` exits the child without zombie processes.
- [ ] Unknown IPC command returns a typed error (not panic).

**D4 — Security:**
- [ ] WebView `connect-src` allows `127.0.0.1:*` only.
- [ ] The loopback application HTML response includes an HTTP
  `Content-Security-Policy` header exactly matching the canonical §5.2 policy,
  including `default-src 'none'`, `script-src 'self'`, and
  `frame-ancestors 'none'`.
- [ ] On Windows, a UNC binary path beginning with `\\` is rejected before any
  filesystem lookup or spawn; absolute local drive paths remain eligible.
- [ ] Binary path with `../` is rejected before spawn.
- [ ] Binary path with `;` or `&&` metacharacters is rejected before spawn.
- [ ] After navigation to the localhost GUI document, runtime CSP tests prove
  that `eval()` and `Function()` are blocked in the WebView.
- [ ] After navigation to the localhost GUI document, runtime CSP tests prove
  that inline scripts are blocked and the effective `script-src` is exactly
  `'self'`, with no inline-script or eval exceptions. `'unsafe-inline'` appears
  only in `style-src`, and `'unsafe-eval'` is absent from the entire policy.

**D5 — Platform:**
- [ ] `.dmg` installer mounts and installs on macOS arm64.
- [ ] `.AppImage` runs on Ubuntu 22.04 without extra deps.
- [ ] `.msi` installs and uninstalls cleanly on Windows 10+.

**D6 — Auto-update:**
- [ ] Update dialog appears when a newer version is in the update manifest.
- [ ] Update is rejected if the signature does not verify.
- [ ] Update check does not block startup.

---

## 14. Implementation Plan (D-series tasks)

### Dependency graph

```
GUI spec G1–G6 (prerequisite, must be implemented first)
  → GUI server sends the canonical §5.2 CSP response header for app HTML
  → D1 Tauri project scaffold + CI skeleton
     → D2 Binary discovery + lifecycle (Rust)
        → D3 IPC commands + desktop frontend
           → D4 System tray + OS notifications
              → D5 Auto-update wiring
                 → D6 Platform smoke + code signing
```

### D1 — Project scaffold and CI skeleton

**New:** `desktop/` directory, `Cargo.toml`, `tauri.conf.json`, `package.json`,
`.github/workflows/desktop-build.yml` (SHA-pinned, no artifact upload yet).

Deliverables:
- `tauri dev` opens a window at `about:blank` on macOS (proof of render).
- `cargo clippy` and `cargo fmt` pass with zero warnings.
- CI matrix builds succeed on all three platforms (no signing yet).
- `tests/workflows/workflow-policy.test.ts` updated to assert `desktop-build.yml`
  has SHA-pinned actions and no hardcoded secrets.

### D2 — Binary discovery and lifecycle

**Files:** `desktop/src-tauri/src/lifecycle.rs`, `commands.rs`, `lib.rs`

Deliverables:
- `discover_binary()` — all four discovery paths and direct install-dialog
  failure transition tested; package runners are never invoked.
- `spawn_gui_server(port)` — spawns `llmup gui --port --no-open --json`,
  reads stdout until JSON, returns URL. Times out at 5 s.
- `shutdown_server()` — SIGTERM → wait 3 s → SIGKILL.
- Install dialog on discovery failure with npm install command + copy button.
- Rust unit tests: 8 tests covering discovery + parse + timeout.

### D3 — IPC commands and desktop frontend

**Files:** `desktop/src-tauri/src/commands.rs`,
`desktop/src/main.ts`, `desktop/index.html`

Deliverables:
- Four IPC commands registered and callable.
- Loading splash (`about:blank` → spinner) while server starts.
- Error splash on startup failure with "Retry" and "Quit" buttons.
- Successful start → navigate WebView to the server URL. The prerequisite GUI
  implementation owns the application document's §5.2 response header.
- TypeScript frontend tests (Vitest, mocked `@tauri-apps/api/core`).

### D4 — System tray and OS notifications

**Files:** `desktop/src-tauri/src/tray.rs`, `notify.rs`

Deliverables:
- Tray icon: filled (ready) and dimmed (starting/stopped) states.
- Tray menu: Open, Status, Separator, Quit.
- OS notification on server ready and unexpected stop.
- Notification suppressed when window is focused.

### D5 — Auto-update wiring

**Files:** `desktop/src-tauri/tauri.conf.json` (updater block), CI workflow update.

Deliverables:
- `latest.json` manifest generated and uploaded as CI release artifact.
- Startup update check (non-blocking async).
- 24 h background poll.
- Signature verification proven by test: a manifest with wrong pubkey is rejected.

### D6 — Platform smoke, signing, and release polish

**Files:** `desktop-smoke.test.ts`, CI signing steps.

Deliverables:
- Smoke test (`TAURI_SMOKE=1`) passes on macOS and Linux CI, including the D4
  loopback response-header assertion and post-navigation runtime CSP checks.
- macOS `.dmg` is code-signed and notarized (secrets injected from GitHub env).
- Windows `.msi` is Authenticode-signed.
- All platform acceptance criteria D5 pass.
- `CHANGELOG.md` entry for first desktop release.

---

## 15. Open Questions

| # | Question | Recommended default |
|---|---|---|
| OQ1 | Should the desktop app ship its own bundled `llmup` binary (self-contained) in v1 or v2? | v2. D1 keeps the discovery approach. |
| OQ2 | Should the app window remember position/size between launches? | Yes — Tauri `window-state` plugin saves it automatically. Add to D3. |
| OQ3 | Should the system tray icon persist after the window is closed (keep server running)? | Yes — "close window but keep serving" is a common pattern for chat apps. Add quit-only-from-tray behavior. |
| OQ4 | Should the desktop app include a model browser (`recommend` output) beyond the chat UI? | Not in v1. A future D7 task can add a panel. |
| OQ5 | Auto-update: opt-in or automatic install? | Dialog prompt (opt-in). Silent auto-install deferred. |
| OQ6 | Should the desktop app support multiple concurrent chat windows (one per harness)? | No in v1. Documented non-goal. |

---

## 16. Boundaries

**Always:**
- Validate the `llmup` binary path before every spawn (reject Windows UNC paths
  before filesystem lookup; require an absolute, executable, metacharacter-free
  local path).
- Use `Command::new(path)` with discrete args — never `sh -c` interpolation.
- Enforce the WebView CSP — no eval or inline-script exceptions; `connect-src`
  loopback only.
- Pin all GitHub Actions `uses:` to full 40-character SHAs.
- Never store code-signing credentials in source code.

**Ask first:**
- Moving to self-contained bundled binary (D2 → requires Node SEA discussion).
- Adding new Tauri capabilities to the allowlist.
- Exposing the GUI server to non-loopback addresses.
- New Rust crate dependencies.
- Platform support beyond the three defined targets.

**Never:**
- Use `shell: true` or string interpolation when spawning the `llmup` binary.
- Use `npx` or another package runner as a discovery fallback, or download and
  run a binary without a separately security-reviewed design.
- Bundle or log API keys, signing keys, or tokens.
- Disable CSP or add eval or inline-script exceptions.
- Ship a release binary that is not code-signed on macOS or Windows.
- Access cloud APIs directly from the Tauri Rust backend.
