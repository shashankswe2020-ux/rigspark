document.addEventListener("DOMContentLoaded", () => {
  const form = document.querySelector("#chat-form");
  const prompt = document.querySelector("#prompt");
  const messages = document.querySelector("#messages");
  const a11yStatus = document.querySelector("#a11y-status");
  const modelSelect = document.querySelector("#model-select");
  const harnessSelect = document.querySelector("#harness-select");
  const runtimeSelect = document.querySelector("#runtime-select");
  const runtimeStatus = document.querySelector("#runtime-status");
  const stageTitle = document.querySelector("#stage-title");
  const backendStatus = document.querySelector("#backend-status");
  const endpointStatus = document.querySelector("#endpoint-status");
  const turnCount = document.querySelector("#turn-count");
  const contextUsage = document.querySelector("#context-usage");
  const modelCard = document.querySelector("#model-card");
  const refreshStatus = document.querySelector("#refresh-status");
  const updateLink = document.querySelector("#update-link");
  const activeOwnership = document.querySelector("#active-ownership");
  const recommendedList = document.querySelector("#recommended-list");
  const modelError = document.querySelector("#model-error");
  const activeBanner = document.querySelector("#active-model-banner");
  const refreshModels = document.querySelector("#refresh-models");
  const contextWindow = document.querySelector("#context-window");
  const modelSource = document.querySelector("#model-source");
  const contextTokens = document.querySelector("#context-tokens");
  const contextTokensField = document.querySelector("#context-tokens-field");
  const installedPort = document.querySelector("#installed-port");
  const installedPortField = document.querySelector("#installed-port-field");
  const modelsFitOnly = document.querySelector("#models-fit-only");
  const modelsRecency = document.querySelector("#models-recency");
  const modelBypass = document.querySelector("#model-bypass");
  const kvCache = document.querySelector("#kv-cache");
  const kvCacheField = document.querySelector("#kv-cache-field");
  const modelCatalogPanel = document.querySelector("#model-catalog-panel");
  const modelDetail = document.querySelector("#model-detail");
  const modelDetailBack = document.querySelector("#model-detail-back");
  const modelDetailContent = document.querySelector("#model-detail-content");
  const sessionCurrent = document.querySelector("#session-current");
  const sessionTurns = document.querySelector("#session-turns");
  const sessionNew = document.querySelector("#session-new");
  const sessionSearch = document.querySelector("#session-search");
  const sessionList = document.querySelector("#session-list");
  const contextBar = document.querySelector("#context-bar");
  const contextAdd = document.querySelector("#context-add");
  const contextEmpty = document.querySelector("#context-empty");
  const contextChips = document.querySelector("#context-chips");
  const contextPicker = document.querySelector("#context-picker");
  const contextRoot = document.querySelector("#context-root");
  const contextRootPath = document.querySelector("#context-root-path");
  const contextRootAdd = document.querySelector("#context-root-add");
  const contextRootCreate = document.querySelector("#context-root-create");
  const contextRootBrowse = document.querySelector("#context-root-browse");
  const contextRootError = document.querySelector("#context-root-error");
  const contextBrowse = document.querySelector("#context-browse");
  const contextSearch = document.querySelector("#context-search");
  const contextResults = document.querySelector("#context-results");
  const contextStart = document.querySelector("#context-start");
  const contextEnd = document.querySelector("#context-end");
  const contextClose = document.querySelector("#context-close");
  const contextGit = document.querySelector("#context-git");
  const contextGitStatus = document.querySelector("#context-git-status");
  const contextGitDiff = document.querySelector("#context-git-diff");
  const contextGitNote = document.querySelector("#context-git-note");
  const workspaceActions = document.querySelector("#workspace-actions");
  const calculatorCreate = document.querySelector("#calculator-create");
  const calculatorPreview = document.querySelector("#calculator-preview");
  const calculatorStatus = document.querySelector("#calculator-status");
  const contextPasteText = document.querySelector("#context-paste-text");
  const contextPasteKind = document.querySelector("#context-paste-kind");
  const contextPasteAdd = document.querySelector("#context-paste-add");
  const connectorForm = document.querySelector("#connector-form");
  const connectorTransport = document.querySelector("#connector-transport");
  const connectorStdioFields = document.querySelector("#connector-stdio-fields");
  const connectorHttpFields = document.querySelector("#connector-http-fields");
  const connectorList = document.querySelector("#connector-list");
  const connectorError = document.querySelector("#connector-error");
  const refreshConnectors = document.querySelector("#refresh-connectors");
  const connectorJson = document.querySelector("#connector-json");
  const connectorJsonReload = document.querySelector("#connector-json-reload");
  const connectorJsonApply = document.querySelector("#connector-json-apply");
  const connectorJsonStatus = document.querySelector("#connector-json-status");
  const hardwareStats = document.querySelector("#hardware-stats");
  const hardwareCard = document.querySelector("#hardware-card");
  const runtimeToggles = document.querySelector("#runtime-toggles");
  const runtimeError = document.querySelector("#runtime-error");
  const refreshRuntime = document.querySelector("#refresh-runtime");
  const agentSelect = document.querySelector("#agent-select");
  const skillChips = document.querySelector("#skill-chips");
  const agentSkillsPicker = document.querySelector("#agent-skills");
  const refreshLibrary = document.querySelector("#refresh-library");
  const agentForm = document.querySelector("#agent-form");
  const agentList = document.querySelector("#agent-list");
  const agentError = document.querySelector("#agent-error");
  const agentNew = document.querySelector("#agent-new");
  const agentCancel = document.querySelector("#agent-cancel");
  const skillForm = document.querySelector("#skill-form");
  const skillList = document.querySelector("#skill-list");
  const skillError = document.querySelector("#skill-error");
  const skillNew = document.querySelector("#skill-new");
  const skillCancel = document.querySelector("#skill-cancel");
  const navItems = document.querySelectorAll(".rail-item[data-view]");
  const views = document.querySelectorAll(".view");
  let modelsRequestSequence = 0;

  if (!form || !prompt || !messages) {
    return;
  }

  // Announce discrete state changes to assistive tech via a polite region.
  // Streaming tokens are NOT announced (the message log is not a live region),
  // so a screen reader is not flooded token-by-token.
  function announce(text) {
    if (!a11yStatus) {
      return;
    }
    a11yStatus.textContent = "";
    globalThis.setTimeout(() => {
      a11yStatus.textContent = text;
    }, 30);
  }

  function roleLabel(role) {
    if (role === "user") {
      return "You";
    }
    if (role === "assistant") {
      return "Assistant";
    }
    return "System";
  }

  function renderAssistantBody(body, text, streaming) {
    globalThis.GuiMarkdown.renderAssistantMarkdown(body, text);
    const row = body.closest(".message");
    if (row) {
      row.classList.toggle("streaming", Boolean(streaming));
    }
    if (!streaming) {
      globalThis.GuiMarkdown.decorateCodeBlocks(body, { onPreview: openArtifact });
    }
  }

  function addMessage(role, content, options) {
    const empty = messages.querySelector(".messages-empty");
    if (empty) {
      empty.remove();
    }
    const row = document.createElement("div");
    row.className = `message ${role}`;

    const label = document.createElement("div");
    label.className = "message-role";
    label.textContent = roleLabel(role);

    const body = document.createElement("div");
    body.className = "message-body";
    if (role === "user") {
      body.textContent = content;
    } else {
      renderAssistantBody(body, content, options?.streaming === true);
    }

    const messageContent = document.createElement("div");
    messageContent.className = "message-content";
    messageContent.append(label, body);
    if (role === "assistant" || role === "user") {
      messageContent.appendChild(buildMessageActions(body, role));
    }
    if (role === "assistant") {
      row.append(sparkyAvatar(), messageContent);
    } else {
      row.appendChild(messageContent);
    }
    messages.appendChild(row);
    updateContextUsage();
    return body;
  }

  function sparkyAvatar() {
    if (globalThis.RigSparkUI) {
      return globalThis.RigSparkUI.faceImage("yes", "message-avatar");
    }
    const avatar = document.createElement("img");
    avatar.className = "message-avatar";
    avatar.src = "/static/brand/sparky-yes-dark.svg";
    avatar.alt = "";
    return avatar;
  }

  // A rough, clearly-labelled estimate of tokens held in the current thread.
  // Real wall-clock timings are measured elsewhere; token counts use ~4 chars/token.
  function updateContextUsage() {
    if (!contextUsage) {
      return;
    }
    let chars = 0;
    for (const el of messages.querySelectorAll(".message-body")) {
      chars += (el.textContent || "").length;
    }
    const tokens = Math.round(chars / 4);
    contextUsage.textContent =
      tokens >= 1000 ? `~${(tokens / 1000).toFixed(1)}k tok` : `~${tokens} tok`;
  }

  function msgActionButton(label, handler) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "msg-action";
    btn.textContent = label;
    btn.addEventListener("click", handler);
    return btn;
  }

  // A hover action bar; Copy reads the body text at click time so it works for
  // streamed replies too. User rows can edit-and-resend; assistant rows regenerate.
  function buildMessageActions(body, role) {
    const actions = document.createElement("div");
    actions.className = "msg-actions";
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "msg-action";
    copy.textContent = "Copy";
    copy.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(body.textContent || "");
        copy.textContent = "Copied";
        setTimeout(() => {
          copy.textContent = "Copy";
        }, 1200);
      } catch {
        copy.textContent = "Copy failed";
        setTimeout(() => {
          copy.textContent = "Copy";
        }, 1200);
      }
    });
    actions.appendChild(copy);

    if (role === "user") {
      actions.appendChild(
        msgActionButton("Edit", () => {
          prompt.value = body.textContent || "";
          prompt.dispatchEvent(new globalThis.Event("input", { bubbles: true }));
          prompt.focus();
        }),
      );
    } else if (role === "assistant") {
      actions.appendChild(
        msgActionButton("Regenerate", () => {
          if (activeRun || !lastPrompt) {
            return;
          }
          const row = body.closest(".message");
          if (row) {
            row.remove();
            updateContextUsage();
          }
          sendPrompt(lastPrompt, { retry: true });
        }),
      );
    }
    return actions;
  }

  // A transient "generating" indicator shown from submit until the first
  // streamed content (delta / tool / terminal) arrives.
  function buildThinking() {
    const row = document.createElement("div");
    row.className = "run-thinking";
    row.setAttribute("role", "status");
    const avatar = sparkyAvatar();
    const dots = document.createElement("span");
    dots.className = "run-thinking-dots";
    for (let i = 0; i < 3; i += 1) {
      dots.appendChild(document.createElement("i"));
    }
    const text = document.createElement("span");
    text.className = "run-thinking-text";
    text.textContent = "Thinking…";
    row.append(avatar, dots, text);
    return row;
  }

  function selectedRuntime() {
    return runtimeSelect && runtimeSelect.value ? runtimeSelect.value : "";
  }

  function setStatusPanel(status) {
    if (backendStatus) {
      backendStatus.textContent = status.harness ?? "local";
    }
    if (endpointStatus && status.endpoint) {
      endpointStatus.textContent = String(status.endpoint).replace(/^https?:\/\//, "");
    }
    const turns = status.memory?.turns ?? 0;
    if (turnCount) {
      turnCount.textContent = String(turns);
    }
    if (sessionTurns) {
      sessionTurns.textContent = `${turns} ${turns === 1 ? "turn" : "turns"}`;
    }
  }

  async function loadStatus() {
    try {
      const response = await globalThis.fetch("/api/status");
      if (!response.ok) {
        throw new Error(`status ${response.status}`);
      }
      const status = await response.json();
      setStatusPanel(status);
    } catch {
      if (backendStatus) {
        backendStatus.textContent = "offline";
      }
      if (endpointStatus) {
        endpointStatus.textContent = "127.0.0.1:11434";
      }
    }
  }

  async function loadUpdateStatus() {
    if (!updateLink) {
      return;
    }
    updateLink.hidden = true;
    try {
      const response = await globalThis.fetch("/api/update");
      if (!response.ok) {
        return;
      }
      const status = await response.json();
      const trustedUrl = "https://github.com/shashankswe2020-ux/rigspark/releases";
      if (
        status.state !== "update-available" ||
        typeof status.latestVersion !== "string" ||
        status.releaseUrl !== trustedUrl
      ) {
        return;
      }
      updateLink.href = trustedUrl;
      updateLink.textContent = `Update to ${status.latestVersion}`;
      updateLink.hidden = false;
    } catch {
      // Update discovery is optional; offline use must remain unaffected.
    }
  }

  async function loadHarnesses() {
    if (!harnessSelect) {
      return;
    }

    try {
      const response = await globalThis.fetch("/api/harnesses");
      if (!response.ok) {
        return;
      }
      const data = await response.json();
      const harnesses = Array.isArray(data.harnesses) ? data.harnesses : [];
      harnessSelect.innerHTML = "";
      for (const harness of harnesses) {
        const option = document.createElement("option");
        option.value = harness;
        option.textContent = harness;
        harnessSelect.appendChild(option);
      }
      const active = document.querySelector("#backend-status");
      if (active && active.textContent) {
        harnessSelect.value = active.textContent;
      }
    } catch {
      // Ignore harness-load failures; local mode remains a valid fallback.
    }
  }

  async function loadRuntimes() {
    if (!runtimeSelect) {
      return;
    }
    try {
      const response = await globalThis.fetch("/api/runtimes");
      if (!response.ok) {
        return;
      }
      const data = await response.json();
      const runtimes = Array.isArray(data.runtimes) ? data.runtimes : [];
      const previous = runtimeSelect.value;
      runtimeSelect.innerHTML = "";
      for (const runtime of runtimes) {
        const option = document.createElement("option");
        option.value = runtime;
        option.textContent = runtime;
        runtimeSelect.appendChild(option);
      }
      if (previous && runtimes.includes(previous)) {
        runtimeSelect.value = previous;
      }
      if (runtimeStatus && runtimeSelect.value) {
        runtimeStatus.textContent = runtimeSelect.value;
      }
    } catch {
      // Ignore runtime-load failures; recommendations fall back to the default runtime.
    }
  }

  function statRow(label, value) {
    const row = document.createElement("li");
    row.className = "stat-row";
    const key = document.createElement("span");
    key.className = "stat-label";
    key.textContent = label;
    const val = document.createElement("span");
    val.className = "stat-value";
    val.textContent = value;
    row.appendChild(key);
    row.appendChild(val);
    return row;
  }

  function gpuSummary(gpus) {
    if (!Array.isArray(gpus) || !gpus.length) {
      return "none detected";
    }
    return gpus
      .map((gpu) => {
        const vram = Number.isFinite(gpu.vramBytes) && gpu.vramBytes > 0 ? ` · ${formatSize(gpu.vramBytes)}` : "";
        return `${gpu.vendor}${vram}`;
      })
      .join(", ");
  }

  function renderHardware(hw) {
    const rows = [
      ["Platform", `${hw.platform} · ${hw.arch}`],
      ["RAM", `${formatSize(hw.freeRamBytes)} free / ${formatSize(hw.totalRamBytes)}`],
      ["GPU", gpuSummary(hw.gpu)],
      ["Disk free", formatSize(hw.freeDiskBytes)],
    ];
    for (const target of [hardwareStats, hardwareCard]) {
      if (!target) {
        continue;
      }
      target.innerHTML = "";
      for (const [label, value] of rows) {
        target.appendChild(statRow(label, value));
      }
    }
  }

  async function loadHardware() {
    if (!hardwareStats && !hardwareCard) {
      return;
    }
    try {
      const response = await globalThis.fetch("/api/hardware");
      if (!response.ok) {
        throw new Error(`request failed (${response.status})`);
      }
      const data = await response.json();
      if (data.hardware) {
        renderHardware(data.hardware);
      }
    } catch (error) {
      for (const target of [hardwareStats, hardwareCard]) {
        if (target) {
          target.innerHTML = "";
          target.appendChild(statRow("Hardware", `unavailable (${error.message})`));
        }
      }
    }
  }

  function runtimeStateLabel(runtime) {
    if (!runtime.installed) {
      return "Not installed";
    }
    if (runtime.running) {
      return runtime.ownedByUs ? "Running" : "Running (external)";
    }
    return "Stopped";
  }

  async function toggleRuntime(name, action, toggle) {
    if (runtimeError) {
      runtimeError.hidden = true;
    }
    toggle.setAttribute("aria-disabled", "true");
    toggle.classList.add("pending");
    const label = toggle.querySelector(".toggle-label");
    if (label) {
      label.textContent = action === "start" ? "Starting…" : "Stopping…";
    }
    try {
      const response = await globalThis.fetch(`/api/runtimes/${encodeURIComponent(name)}/${action}`, {
        method: "POST",
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `request failed (${response.status})`);
      }
      await loadRuntimeStatus();
    } catch (error) {
      if (runtimeError) {
        runtimeError.hidden = false;
        runtimeError.textContent = `Could not ${action} ${name}: ${error.message}`;
      }
      await loadRuntimeStatus();
    }
  }

  function renderRuntimeStatus(runtimes) {
    if (!runtimeToggles) {
      return;
    }
    runtimeToggles.innerHTML = "";
    if (!runtimes.length) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = "No runtimes detected.";
      runtimeToggles.appendChild(empty);
      return;
    }
    for (const runtime of runtimes) {
      const row = document.createElement("article");
      row.className = "runtime-row";

      const info = document.createElement("div");
      info.className = "runtime-info";
      const title = document.createElement("div");
      title.className = "runtime-name";
      title.textContent = runtime.name;
      const meta = document.createElement("div");
      meta.className = "runtime-meta";
      const endpoint = runtime.endpoint ? ` · ${String(runtime.endpoint).replace(/^https?:\/\//, "")}` : "";
      meta.textContent = `${runtimeStateLabel(runtime)}${endpoint}`;
      info.appendChild(title);
      info.appendChild(meta);
      if (runtime.detail) {
        const detail = document.createElement("div");
        detail.className = "runtime-detail";
        detail.textContent = runtime.detail;
        info.appendChild(detail);
      }

      const running = Boolean(runtime.running);
      // A running daemon can be toggled off (including a foreign one the server
      // stops after verifying ownership); a startable runtime can be toggled on.
      const interactive = running ? Boolean(runtime.canStop) : runtime.canStart && runtime.installed;
      const toggle = document.createElement("div");
      toggle.className = `toggle${running ? " on" : ""}${interactive ? "" : " disabled"}`;
      toggle.setAttribute("role", "switch");
      toggle.setAttribute("aria-checked", running ? "true" : "false");
      if (!interactive) {
        toggle.setAttribute("aria-disabled", "true");
      }
      toggle.tabIndex = interactive ? 0 : -1;

      const track = document.createElement("span");
      track.className = "toggle-track";
      const thumb = document.createElement("span");
      thumb.className = "toggle-thumb";
      track.appendChild(thumb);
      const toggleLabel = document.createElement("span");
      toggleLabel.className = "toggle-label";
      toggleLabel.textContent = running ? "On" : "Off";
      toggle.appendChild(track);
      toggle.appendChild(toggleLabel);

      const fire = () => {
        if (toggle.getAttribute("aria-disabled") === "true") {
          return;
        }
        toggleRuntime(runtime.name, running ? "stop" : "start", toggle);
      };
      if (interactive) {
        toggle.addEventListener("click", fire);
        toggle.addEventListener("keydown", (event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            fire();
          }
        });
      }

      row.appendChild(info);
      row.appendChild(toggle);
      runtimeToggles.appendChild(row);
    }
  }

  async function loadRuntimeStatus() {
    if (!runtimeToggles) {
      return;
    }
    try {
      const response = await globalThis.fetch("/api/runtimes/status");
      if (!response.ok) {
        throw new Error(`request failed (${response.status})`);
      }
      const data = await response.json();
      renderRuntimeStatus(Array.isArray(data.runtimes) ? data.runtimes : []);
    } catch (error) {
      runtimeToggles.innerHTML = "";
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = `Could not load runtimes: ${error.message}`;
      runtimeToggles.appendChild(empty);
    }
  }

  async function loadHistory() {
    try {
      const response = await globalThis.fetch("/api/history");
      if (!response.ok) {
        return;
      }
      const data = await response.json();
      const history = Array.isArray(data.history) ? data.history : [];
      messages.innerHTML = "";
      for (const item of history) {
        if (item && item.role && item.content) {
          addMessage(item.role, item.content);
        }
      }
      if (!messages.querySelector(".message")) {
        showEmptyMessages();
      }
    } catch {
      // Ignore stale history failures.
    }
  }

  const sendBtn = form.querySelector(".send-btn");
  const runReducer = globalThis.GuiRunReducer || null;
  let activeRun = null;
  let lastPrompt = "";

  function setComposerState(mode) {
    if (!sendBtn) {
      return;
    }
    if (mode === "running") {
      sendBtn.textContent = "Stop";
      sendBtn.setAttribute("aria-label", "Stop response");
      sendBtn.classList.add("is-stop");
      sendBtn.disabled = false;
    } else if (mode === "stopping") {
      sendBtn.textContent = "Stopping…";
      sendBtn.setAttribute("aria-label", "Stopping");
      sendBtn.disabled = true;
    } else {
      sendBtn.textContent = "Send";
      sendBtn.setAttribute("aria-label", "Send");
      sendBtn.classList.remove("is-stop");
      sendBtn.disabled = false;
    }
  }

  function renderComposer(phase) {
    if (phase === "stopping") {
      setComposerState("stopping");
    } else if (phase === "sending" || phase === "running") {
      setComposerState("running");
    } else {
      setComposerState("idle");
    }
  }

  function clearRunNotices() {
    for (const notice of messages.querySelectorAll(".run-notice, .disclosure-notice")) {
      notice.remove();
    }
  }

  // A recoverable end-of-run message with an optional Retry that reuses the same
  // prompt without adding a second user bubble.
  function addRunNotice(text, canRetry) {
    const follow = nearBottom();
    const row = document.createElement("div");
    row.className = "run-notice";
    const label = document.createElement("span");
    label.className = "run-notice-text";
    label.textContent = text;
    row.appendChild(label);
    if (canRetry) {
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "retry-btn";
      retry.textContent = "Retry";
      retry.addEventListener("click", () => {
        row.remove();
        sendPrompt(lastPrompt, { retry: true });
      });
      row.appendChild(retry);
    }
    messages.appendChild(row);
    if (follow) {
      autoScroll(true);
    }
  }

  function formatBytes(bytes) {
    if (bytes < 1024) {
      return `${bytes} B`;
    }
    if (bytes < 1024 * 1024) {
      return `${(bytes / 1024).toFixed(1)} KiB`;
    }
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  }

  // A blocking data-boundary prompt: workspace context is about to leave the
  // machine for a cloud provider. Nothing was sent; the user must confirm.
  function addDisclosureNotice(info) {
    if (!info) {
      return;
    }
    const follow = nearBottom();
    const row = document.createElement("div");
    row.className = "disclosure-notice";
    const head = document.createElement("div");
    head.className = "disclosure-head";
    head.textContent = `Send workspace context to ${info.provider}?`;
    const sub = document.createElement("div");
    sub.className = "disclosure-sub";
    const included = (info.items || []).filter((i) => i.included);
    sub.textContent = `${included.length} item${included.length === 1 ? "" : "s"} · ${formatBytes(info.totalBytes || 0)} → ${info.provider} (${info.model}). This context will leave your machine.`;
    const list = document.createElement("ul");
    list.className = "disclosure-list";
    for (const item of info.items || []) {
      const li = document.createElement("li");
      li.className = item.included ? "disclosure-item" : "disclosure-item excluded";
      const name = item.label || item.path || "context";
      const status = item.included ? "" : " — excluded (context budget)";
      li.textContent = `${name}${status}`;
      list.appendChild(li);
    }
    const actions = document.createElement("div");
    actions.className = "disclosure-actions";
    const confirm = document.createElement("button");
    confirm.type = "button";
    confirm.className = "disclosure-confirm";
    confirm.textContent = `Send to ${info.provider}`;
    confirm.addEventListener("click", () => {
      row.remove();
      sendPrompt(lastPrompt, { retry: true, disclosureAck: true });
    });
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "disclosure-cancel";
    cancel.textContent = "Keep local";
    cancel.addEventListener("click", () => row.remove());
    actions.append(confirm, cancel);
    row.append(head, sub, list, actions);
    messages.appendChild(row);
    if (follow) {
      autoScroll(true);
    }
  }

  function messageScroller() {
    if (messages.scrollHeight > messages.clientHeight + 1) {
      return messages;
    }
    return document.scrollingElement || messages;
  }

  function nearBottom() {
    const scroller = messageScroller();
    return scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 120;
  }

  function autoScroll(force) {
    if (force || nearBottom()) {
      const scroller = messageScroller();
      scroller.scrollTop = scroller.scrollHeight;
    }
  }

  async function stopRun() {
    if (!activeRun || activeRun.stopping) {
      return;
    }
    // Capture the run: it may finalize (activeRun → null) during the await below.
    const run = activeRun;
    run.stopping = true;
    run.dispatch({ type: "request-stop" });
    renderComposer(run.state.phase);
    announce("Stopping…");
    try {
      await globalThis.fetch("/api/chat/cancel", { method: "POST" });
    } catch {
      // Best-effort: the client abort below still ends the local read loop.
    }
    run.controller.abort();
  }

  async function sendPrompt(value, options) {
    const opts = options || {};
    if (activeRun || !value) {
      return;
    }
    let followOnNextPaint = nearBottom();
    clearRunNotices();
    if (!opts.retry) {
      addMessage("user", value);
    }
    lastPrompt = value;

    const controller = new globalThis.AbortController();
    const run = {
      controller,
      stopping: false,
      state: runReducer ? runReducer.initialRunState() : { phase: "sending", reply: "", error: null },
      dispatch(action) {
        if (runReducer) {
          this.state = runReducer.reduceRun(this.state, action);
        }
      },
    };
    run.dispatch({ type: "submit", prompt: value });
    activeRun = run;
    renderComposer(run.state.phase);
    announce("Sending message.");

    let reply = "";
    let assistantBody = null;
    let terminal = null; // "done" | "error" | "disclosure"
    let errorMessage = "";
    let disclosureInfo = null;
    const toolCards = new Map();
    const assistantRenderScheduler = globalThis.GuiMarkdown.createRenderScheduler(
      (body, text, streaming) => {
        const follow = followOnNextPaint || nearBottom();
        followOnNextPaint = false;
        renderAssistantBody(body, text, streaming);
        if (follow) {
          autoScroll(true);
        }
      },
    );
    // Real wall-clock timings for the per-message metrics line.
    const startedAt = globalThis.performance.now();
    let firstDeltaAt = null;
    let thinkingRow = buildThinking();
    {
      const empty = messages.querySelector(".messages-empty");
      if (empty) {
        empty.remove();
      }
      messages.appendChild(thinkingRow);
      if (followOnNextPaint) {
        autoScroll(true);
      }
    }

    function clearThinking() {
      if (thinkingRow) {
        thinkingRow.remove();
        thinkingRow = null;
      }
    }

    function riskLabel(risk) {
      if (risk === "workspace-mutation") {
        return "workspace write";
      }
      if (risk === "process-network") {
        return "process / network";
      }
      if (risk === "read-only") {
        return "read-only";
      }
      return "unknown risk";
    }

    function summarizeArgs(args) {
      try {
        const json = JSON.stringify(args);
        return json.length > 200 ? `${json.slice(0, 200)}…` : json;
      } catch {
        return "";
      }
    }

    function toolActionButton(label, handler) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "tool-action";
      button.textContent = label;
      button.addEventListener("click", handler);
      return button;
    }

    function decideTool(callId, decision, actions) {
      actions.hidden = true;
      globalThis
        .fetch("/api/chat/tool-decision", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ callId, decision }),
        })
        .catch(() => {});
    }

    function toolCard(callId) {
      const existing = toolCards.get(callId);
      if (existing) {
        return existing;
      }
      const empty = messages.querySelector(".messages-empty");
      if (empty) {
        empty.remove();
      }
      const row = document.createElement("div");
      row.className = "tool-card proposed";
      const head = document.createElement("div");
      head.className = "tool-card-head";
      const dot = document.createElement("span");
      dot.className = "tool-dot";
      const title = document.createElement("span");
      title.className = "tool-card-title";
      const badge = document.createElement("span");
      badge.className = "tool-card-badge";
      head.append(dot, title, badge);
      const meta = document.createElement("div");
      meta.className = "tool-card-meta";
      const body = document.createElement("div");
      body.className = "tool-card-body";
      const actions = document.createElement("div");
      actions.className = "tool-card-actions";
      actions.hidden = true;
      row.append(head, meta, body, actions);
      messages.appendChild(row);
      const card = { row, title, badge, meta, body, actions };
      toolCards.set(callId, card);
      return card;
    }

    // Render/refresh one tool card across its lifecycle: proposed →
    // (approval-required → decision) → start → done | denied.
    function updateToolCard(event) {
      const card = toolCard(event.callId);
      if (event.risk) {
        card.badge.textContent = riskLabel(event.risk);
        card.badge.dataset.risk = event.risk;
      }
      const connector = event.connector ? ` · ${event.connector}` : "";
      if (event.phase === "proposed") {
        card.row.className = "tool-card proposed";
        card.title.textContent = `${event.name}${connector}`;
        if (event.arguments && Object.keys(event.arguments).length) {
          card.meta.textContent = summarizeArgs(event.arguments);
        }
      } else if (event.phase === "approval-required") {
        card.row.className = "tool-card awaiting";
        card.title.textContent = `${event.name}${connector} — approve?`;
        card.actions.hidden = false;
        card.actions.innerHTML = "";
        announce(`Tool approval needed: ${event.name}.`);
        const approve = toolActionButton("Approve", () =>
          decideTool(event.callId, "approve-once", card.actions),
        );
        const session = toolActionButton("Allow for session", () =>
          decideTool(event.callId, "allow-session", card.actions),
        );
        const deny = toolActionButton("Deny", () => decideTool(event.callId, "deny", card.actions));
        deny.classList.add("tool-deny");
        card.actions.append(approve, session, deny);
      } else if (event.phase === "start") {
        card.row.className = "tool-card running";
        card.title.textContent = `Calling ${event.name}${connector}`;
        card.actions.hidden = true;
      } else if (event.phase === "denied") {
        card.row.className = "tool-card denied";
        card.title.textContent = `${event.name} denied`;
        card.actions.hidden = true;
      } else if (event.phase === "done") {
        card.row.className = event.isError ? "tool-card error" : "tool-card done";
        const ms = typeof event.durationMs === "number" ? ` · ${event.durationMs} ms` : "";
        card.title.textContent = event.isError
          ? `${event.name} failed${ms}`
          : `Used ${event.name}${ms}`;
        card.actions.hidden = true;
        if (event.result) {
          card.body.textContent = `${event.result}${event.resultTruncated ? " …" : ""}`;
        }
      }
      autoScroll(false);
    }

    // Handle one decoded SSE event object. Returns true when the run is over.
    function handleEvent(event) {
      clearThinking();
      run.dispatch({ type: "stream-event", event });
      if (event.type === "tool") {
        updateToolCard(event);
        return false;
      }
      if (event.type === "context") {
        addContextLedger(event.attachments || []);
        return false;
      }
      if (event.type === "edit") {
        renderEditReview(event.review, event.workspaceId, event.operations);
        return false;
      }
      if (event.type === "disclosure-required") {
        terminal = "disclosure";
        disclosureInfo = event;
        return true;
      }
      if (event.type === "delta") {
        reply += event.content;
        if (firstDeltaAt === null) {
          firstDeltaAt = globalThis.performance.now();
        }
        if (!assistantBody) {
          assistantBody = addMessage("assistant", "", { streaming: true });
        }
        assistantRenderScheduler.update(assistantBody, reply);
        renderComposer(run.state.phase);
        return false;
      }
      if (event.type === "done") {
        terminal = "done";
        return true;
      }
      if (event.type === "error") {
        terminal = "error";
        errorMessage = event.message;
        return true;
      }
      return false;
    }

    // Drain buffered frames; resolve the run when a terminal event arrives.
    async function consume(frames) {
      for (const frame of frames) {
        if (frame.error !== undefined) {
          continue;
        }
        if (handleEvent(frame.event)) {
          return true;
        }
      }
      return false;
    }

    // Attach a per-message metrics line: browser-measured wall-clock timings
    // (exact) plus a clearly-estimated tok/s (~4 chars/token).
    function renderRunMetrics() {
      updateContextUsage();
      if (!assistantBody || firstDeltaAt === null) {
        return;
      }
      const row = assistantBody.closest(".message");
      if (!row || row.querySelector(".msg-metrics")) {
        return;
      }
      const totalMs = globalThis.performance.now() - startedAt;
      const ttftMs = firstDeltaAt - startedAt;
      const chars = reply.length;
      const secs = totalMs / 1000;
      const tokens = Math.round(chars / 4);
      const decodeSecs = (totalMs - ttftMs) / 1000;
      // Too few tokens or too short a decode window cannot support a rate estimate.
      const rate = tokens >= 8 && decodeSecs >= 0.25
        ? ` · ~${(tokens / decodeSecs).toFixed(tokens / decodeSecs < 10 ? 1 : 0)} tok/s`
        : "";
      const metrics = document.createElement("div");
      metrics.className = "msg-metrics";
      metrics.title = `Wall-clock timing measured in the browser. Token count is estimated at ~4 chars/token (${chars} chars); the rate covers decode time after the first token.`;
      metrics.textContent = `${(ttftMs / 1000).toFixed(2)}s to first token · ${secs.toFixed(1)}s total${rate}`;
      row.appendChild(metrics);
    }

    // Settle the run: finalize the partial answer and show a recoverable notice
    // for anything other than a clean completion.
    function finalize() {
      clearThinking();
      if (assistantBody) {
        assistantRenderScheduler.finalize(assistantBody, reply);
      }
      const wasStopping = run.stopping;
      activeRun = null;
      if (terminal === "done") {
        renderRunMetrics();
        renderComposer("completed");
        announce("Response ready.");
        return;
      }
      if (terminal === "disclosure") {
        renderComposer("completed");
        addDisclosureNotice(disclosureInfo);
        announce("Confirmation needed before sending workspace context.");
        return;
      }
      if (terminal === "error") {
        renderComposer("failed");
        addRunNotice(`Error: ${errorMessage}`, true);
        announce("Request failed.");
        return;
      }
      // Stream ended with no terminal event: an explicit stop or a dropped link.
      if (wasStopping) {
        run.dispatch({ type: "cancelled" });
        renderComposer(run.state.phase);
        addRunNotice("Stopped.", true);
        announce("Stopped.");
      } else {
        run.dispatch({ type: "stream-error", message: "connection lost" });
        renderComposer(run.state.phase);
        addRunNotice("Connection lost.", true);
        announce("Connection lost.");
      }
    }

    let response;
    try {
      response = await globalThis.fetch("/api/chat", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        signal: controller.signal,
        body: JSON.stringify({
          model: modelSelect && modelSelect.value ? modelSelect.value : "demo-model",
          messages: [{ role: "user", content: value }],
          ...(agentSelect && agentSelect.value ? { agentId: agentSelect.value } : {}),
          ...(selectedSkills.size > 0 ? { skillIds: Array.from(selectedSkills) } : {}),
          ...(systemPromptValue() ? { systemPrompt: systemPromptValue() } : {}),
          ...(temperatureValue() !== undefined ? { temperature: temperatureValue() } : {}),
          ...(attachmentRefs().length > 0 ? { attachments: attachmentRefs() } : {}),
          ...(contextSourceRefs().length > 0 ? { contextSources: contextSourceRefs() } : {}),
          ...(opts.disclosureAck ? { disclosureAck: true } : {}),
        }),
      });
    } catch {
      finalize();
      return;
    }

    if (response.status === 409) {
      activeRun = null;
      renderComposer("completed");
      addRunNotice("A response is already in progress.", false);
      return;
    }
    if (!response.ok || !response.body) {
      terminal = "error";
      errorMessage = "Request failed.";
      finalize();
      return;
    }

    const reader = response.body.getReader();
    const sseBuffer =
      globalThis.GuiSse && globalThis.GuiSse.SseFrameBuffer
        ? new globalThis.GuiSse.SseFrameBuffer()
        : null;
    const fallbackDecoder = sseBuffer ? null : new globalThis.TextDecoder();

    try {
      let stop = false;
      while (!stop) {
        const { done, value: chunk } = await reader.read();
        if (done) {
          if (sseBuffer) {
            await consume(sseBuffer.flush());
          }
          break;
        }
        if (sseBuffer) {
          stop = await consume(sseBuffer.push(chunk));
          continue;
        }
        // Fallback path when the buffered parser is unavailable.
        const text = fallbackDecoder.decode(chunk, { stream: true });
        for (const line of text.split("\n\n")) {
          if (!line.startsWith("data:")) {
            continue;
          }
          const payload = line.slice("data:".length).trim();
          if (!payload) {
            continue;
          }
          if (await consume([{ event: JSON.parse(payload) }])) {
            stop = true;
            break;
          }
        }
      }
    } catch {
      // Reader aborted (Stop) or the connection dropped; finalize decides which.
    }

    if (terminal === "done") {
      await loadStatus();
      await refreshSessionsIfEnabled();
    }
    finalize();
  }

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (activeRun) {
      stopRun();
      return;
    }
    const value = prompt.value.trim();
    if (!value) {
      return;
    }
    prompt.value = "";
    prompt.style.height = "";
    sendPrompt(value, { retry: false });
  });

  if (harnessSelect) {
    harnessSelect.addEventListener("change", async () => {
      const value = harnessSelect.value;
      if (!value) {
        return;
      }
      await globalThis.fetch("/api/harness", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ harness: value }),
      });
      await loadStatus();
    });
  }

  if (runtimeSelect) {
    runtimeSelect.addEventListener("change", async () => {
      if (runtimeStatus && runtimeSelect.value) {
        runtimeStatus.textContent = runtimeSelect.value;
      }
      await loadModels();
    });
  }

  if (refreshStatus) {
    refreshStatus.addEventListener("click", async () => {
      await loadStatus();
      await loadUpdateStatus();
      await loadActive();
      await loadHistory();
      await loadHarnesses();
      await loadRuntimes();
      await loadLibrary();
    });
  }

  if (sessionCurrent) {
    sessionCurrent.addEventListener("click", async () => {
      switchView("chat");
      await loadHistory();
      await loadStatus();
    });
  }

  function switchView(view) {
    const titles = { chat: "Chat", models: "Models", create: "Create", connectors: "Connectors", library: "Library", tools: "Runtime" };
    for (const item of navItems) {
      item.classList.toggle("active", item.dataset.view === view);
    }
    for (const section of views) {
      section.classList.toggle("active", section.id === `view-${view}`);
    }
    if (stageTitle && titles[view]) {
      stageTitle.textContent = titles[view];
    }
    if (view === "models") {
      showModelCatalog();
      loadModels();
    }
    if (view === "connectors") {
      loadConnectors();
    }
    if (view === "library") {
      loadLibrary();
    }
    if (view === "tools") {
      loadHardware();
      loadRuntimeStatus();
    }
  }

  for (const item of navItems) {
    item.addEventListener("click", () => switchView(item.dataset.view));
  }

  function formatSize(bytes) {
    if (!Number.isFinite(bytes) || bytes <= 0) {
      return "unknown";
    }
    const gb = bytes / 1024 ** 3;
    if (gb >= 1) {
      return `${gb.toFixed(1)} GiB`;
    }
    const mb = bytes / 1024 ** 2;
    if (mb >= 1) {
      return `${Math.round(mb)} MiB`;
    }
    const kb = bytes / 1024;
    return kb >= 1 ? `${Math.round(kb)} KiB` : `${Math.round(bytes)} bytes`;
  }

  function formatThroughput(throughput) {
    if (!throughput || !throughput.known) {
      return "unknown";
    }
    return `${throughput.lowTokPerSec}–${throughput.highTokPerSec} tok/s`;
  }

  function formatTokens(tokens) {
    return Number.isInteger(tokens) && tokens > 0 ? tokens.toLocaleString() : "unknown";
  }

  function detailElement(tag, className, text) {
    const node = document.createElement(tag);
    if (className) {
      node.className = className;
    }
    if (text !== undefined) {
      node.textContent = text;
    }
    return node;
  }

  function detailMetric(label, value, note) {
    const metric = detailElement("div", "model-detail-metric");
    metric.appendChild(detailElement("span", "model-detail-label", label));
    metric.appendChild(detailElement("strong", "model-detail-value", value));
    if (note) {
      metric.appendChild(detailElement("span", "model-detail-note", note));
    }
    return metric;
  }

  function showModelCatalog() {
    if (modelCatalogPanel) {
      modelCatalogPanel.hidden = false;
    }
    if (modelDetail) {
      modelDetail.hidden = true;
    }
    if (stageTitle) {
      stageTitle.textContent = "Models";
    }
  }

  function sourceRows(source) {
    const rows = [];
    if (source && source.ollama) rows.push(["Ollama", source.ollama]);
    if (source && source.hf) rows.push(["Hugging Face", source.hf]);
    if (source && source.gguf) {
      rows.push(["GGUF", `${source.gguf.repo} · ${source.gguf.file}`]);
    }
    if (source && source.mlx) rows.push(["MLX", source.mlx.repo]);
    return rows;
  }

  function renderModelDetail(model, active) {
    if (!modelDetail || !modelDetailContent || !modelCatalogPanel) {
      return;
    }
    modelDetailContent.innerHTML = "";

    const header = detailElement("header", "model-detail-header");
    const identity = detailElement("div", "model-detail-identity");
    identity.appendChild(
      detailElement(
        "div",
        "model-detail-kicker",
        model.releaseDate
          ? `${model.family} · released ${model.releaseDate}`
          : model.addedAt
            ? `${model.family} · added ${model.addedAt} · auto-sourced`
            : `${model.family} · released unknown`,
      ),
    );
    const titleRow = detailElement("div", "model-detail-title-row");
    const title = detailElement("h2", "model-detail-title", model.id);
    title.id = "model-detail-title";
    const badge = detailElement(
      "span",
      `verdict-badge verdict-${model.verdict}`,
      verdictLabel(model.verdict, model.throughput),
    );
    titleRow.appendChild(title);
    titleRow.appendChild(badge);
    identity.appendChild(titleRow);
    identity.appendChild(
      detailElement(
        "p",
        "model-detail-summary",
        `${model.params}${model.activeParams ? ` · ${model.activeParams} active` : ""} · ${model.architecture} · ${model.license}`,
      ),
    );
    header.appendChild(identity);

    const actions = detailElement("div", "model-detail-actions");
    const backends = Array.isArray(model.backends) ? model.backends : [];
    let backendSelect = null;
    if (backends.length > 1) {
      backendSelect = detailElement("select", "model-backend-select");
      backendSelect.setAttribute("aria-label", `Runtime for ${model.id}`);
      const auto = detailElement("option", "", "Auto runtime");
      auto.value = "";
      backendSelect.appendChild(auto);
      for (const backend of backends) {
        const option = detailElement("option", "", backend);
        option.value = backend;
        backendSelect.appendChild(option);
      }
      actions.appendChild(backendSelect);
    }
    const start = detailElement(
      "button",
      "accent-btn",
      active && active.modelId === model.id
        ? "Running"
        : model.verdict === "no"
          ? "Start anyway"
          : "Start model",
    );
    start.type = "button";
    if (active && active.modelId === model.id) {
      start.disabled = true;
    } else {
      start.addEventListener("click", () =>
        startModel(model.id, start, backendSelect ? backendSelect.value : "", model.contextTokens !== undefined ? { context: model.contextTokens } : {}),
      );
    }
    actions.appendChild(start);
    header.appendChild(actions);
    modelDetailContent.appendChild(header);

    const scoreSection = detailElement("section", "model-detail-section");
    scoreSection.appendChild(detailElement("h3", "model-detail-section-title", "Recommendation score"));
    const scoreLead = detailElement("div", "model-score-lead");
    scoreLead.appendChild(detailElement("strong", "model-score-total", `${Math.round(model.score * 100)}`));
    scoreLead.appendChild(detailElement("span", "model-score-caption", "overall / 100"));
    scoreSection.appendChild(scoreLead);
    const scoreGrid = detailElement("div", "model-score-grid");
    for (const [key, label] of [
      ["quality", "Quality"],
      ["fit", "Hardware fit"],
      ["speed", "Speed"],
      ["recency", "Recency"],
      ["capability", "Capability"],
    ]) {
      const value = Number(model.scores && model.scores[key]);
      const percent = Number.isFinite(value) ? Math.round(value * 100) : null;
      const row = detailElement("div", "model-score-row");
      row.appendChild(detailElement("span", "model-score-name", label));
      const track = detailElement("span", "model-score-track");
      const fill = detailElement("span", "model-score-fill");
      fill.style.width = `${percent ?? 0}%`;
      track.appendChild(fill);
      row.appendChild(track);
      row.appendChild(detailElement("strong", "model-score-value", percent === null ? "unknown" : `${percent}`));
      scoreGrid.appendChild(row);
    }
    scoreSection.appendChild(scoreGrid);
    modelDetailContent.appendChild(scoreSection);

    const evidenceGrid = detailElement("div", "model-detail-columns");
    const performance = detailElement("section", "model-detail-section");
    performance.appendChild(detailElement("h3", "model-detail-section-title", "Performance & fit"));
    const utilization =
      Number.isFinite(model.requiredBytes) && Number.isFinite(model.usableBytes) && model.usableBytes > 0
        ? `${Math.round((model.requiredBytes / model.usableBytes) * 100)}%`
        : "unknown";
    const metricGrid = detailElement("div", "model-detail-metrics");
    metricGrid.appendChild(
      detailMetric(
        "Decode speed",
        formatThroughput(model.throughput),
        model.throughput && model.throughput.known
          ? `Offline estimate · ${model.throughputEvidence.backend}`
          : model.throughputEvidence.unknownReason || "No sourced performance profile",
      ),
    );
    metricGrid.appendChild(detailMetric("Selected quant", model.quant, `${formatSize(model.diskBytes)} on disk`));
    metricGrid.appendChild(detailMetric("Memory required", formatSize(model.requiredBytes), `${utilization} of usable memory`));
    metricGrid.appendChild(detailMetric("Usable memory", formatSize(model.usableBytes), "Detected with safety headroom"));
    metricGrid.appendChild(
      detailMetric(
        "Requested context",
        formatTokens(model.contextSizing && model.contextSizing.tokens),
        model.contextSizing && model.contextSizing.kvCacheBytes !== null
          ? `${formatSize(model.contextSizing.kvCacheBytes)} KV cache${model.kvPrecision ? ` (${model.kvPrecision})` : ""}`
          : "KV cost unknown: attention geometry is not sourced",
      ),
    );
    metricGrid.appendChild(detailMetric("Native context", formatTokens(model.contextLength), `${formatSize(model.kvBytesPerToken)} KV per token`));
    performance.appendChild(metricGrid);
    evidenceGrid.appendChild(performance);

    const profile = detailElement("section", "model-detail-section");
    profile.appendChild(detailElement("h3", "model-detail-section-title", "Model profile"));
    const facts = detailElement("dl", "model-detail-facts");
    for (const [label, value] of [
      ["Architecture", model.architecture || "unknown"],
      ["Parameters", model.params || "unknown"],
      ["Active parameters", model.activeParams || "not applicable"],
      ["License", model.license || "unknown"],
      ["Weights", model.openWeight ? "Open weight" : "restricted"],
      ["Quality proxy", Number.isFinite(model.benchmarkProxy) ? model.benchmarkProxy.toFixed(2) : "unknown"],
      ["Runtimes", backends.length ? backends.join(", ") : "none available"],
    ]) {
      facts.appendChild(detailElement("dt", "", label));
      facts.appendChild(detailElement("dd", "", value));
    }
    profile.appendChild(facts);
    const chips = detailElement("div", "model-capability-list");
    for (const capability of Array.isArray(model.capabilities) ? model.capabilities : []) {
      chips.appendChild(detailElement("span", "model-capability", capability));
    }
    profile.appendChild(chips);
    evidenceGrid.appendChild(profile);
    modelDetailContent.appendChild(evidenceGrid);

    const quantSection = detailElement("section", "model-detail-section");
    quantSection.appendChild(detailElement("h3", "model-detail-section-title", "Quantization options"));
    const tableWrap = detailElement("div", "model-quant-table-wrap");
    const table = detailElement("table", "model-quant-table");
    const thead = document.createElement("thead");
    const headerRow = document.createElement("tr");
    for (const label of ["Quant", "Disk", "Min RAM", "Min VRAM", "Integrity"]) {
      headerRow.appendChild(detailElement("th", "", label));
    }
    thead.appendChild(headerRow);
    table.appendChild(thead);
    const tbody = document.createElement("tbody");
    for (const quantization of Array.isArray(model.quantizations) ? model.quantizations : []) {
      const row = document.createElement("tr");
      const integrity = quantization.sha256
        ? quantization.digestVerified === false
          ? "Digest pending"
          : "SHA-256 cataloged"
        : "Size-floor fallback";
      for (const value of [
        quantization.name,
        formatSize(quantization.diskBytes),
        formatSize(quantization.minRamBytes),
        formatSize(quantization.minVramBytes),
        integrity,
      ]) {
        row.appendChild(detailElement("td", "", value));
      }
      tbody.appendChild(row);
    }
    table.appendChild(tbody);
    tableWrap.appendChild(table);
    quantSection.appendChild(tableWrap);
    modelDetailContent.appendChild(quantSection);

    const sourceSection = detailElement("section", "model-detail-section");
    sourceSection.appendChild(detailElement("h3", "model-detail-section-title", "Catalog evidence"));
    const sources = detailElement("div", "model-source-list");
    const rows = sourceRows(model.source);
    if (rows.length === 0) {
      sources.appendChild(detailElement("p", "model-detail-note", "No runtime source is cataloged."));
    } else {
      for (const [label, value] of rows) {
        const row = detailElement("div", "model-source-row");
        row.appendChild(detailElement("span", "model-detail-label", label));
        row.appendChild(detailElement("code", "", value));
        sources.appendChild(row);
      }
    }
    sourceSection.appendChild(sources);
    sourceSection.appendChild(
      detailElement(
        "p",
        "model-evidence-note",
        "Scores and throughput are deterministic offline estimates from the bundled catalog and detected hardware. Unknown inputs remain unknown; no benchmark result is implied.",
      ),
    );
    modelDetailContent.appendChild(sourceSection);

    modelCatalogPanel.hidden = true;
    modelDetail.hidden = false;
    if (stageTitle) {
      stageTitle.textContent = "Model details";
    }
    if (modelDetailBack) {
      modelDetailBack.focus();
    }
  }

  function ensureModelOption(id) {
    if (!modelSelect || !id) {
      return;
    }
    const exists = Array.from(modelSelect.options).some((option) => option.value === id);
    if (!exists) {
      const option = document.createElement("option");
      option.value = id;
      option.textContent = id;
      modelSelect.appendChild(option);
    }
  }

  function renderActive(active) {
    if (modelCard) {
      modelCard.textContent = active ? active.modelId : "No model running";
    }
    if (activeOwnership) {
      if (active) {
        activeOwnership.hidden = false;
        activeOwnership.textContent = active.ownership === "owned" ? "started by workspace" : "attached";
        activeOwnership.dataset.ownership = active.ownership;
      } else {
        activeOwnership.hidden = true;
      }
    }
    if (activeBanner) {
      if (active) {
        activeBanner.hidden = false;
        const strong = document.createElement("b");
        strong.textContent = active.modelId;
        const detail = document.createElement("span");
        detail.className = "running-banner-detail";
        detail.textContent = ` on ${active.backend} at ${active.endpoint}${active.runtimeModelId ? ` · Runtime model: ${active.runtimeModelId}` : ""}${active.context ? ` · Context: ${active.context}` : ""}${active.cache ? ` · Cache: ${cacheLabel(active.cache)}` : ""}`;
        const text = document.createElement("span");
        text.className = "running-banner-text";
        text.append(strong, " is running", detail);
        activeBanner.replaceChildren(modelFace("yes"), text);
      } else {
        activeBanner.hidden = true;
      }
    }
    if (active && endpointStatus && active.endpoint) {
      endpointStatus.textContent = String(active.endpoint).replace(/^https?:\/\//, "");
    }
    if (active && active.backend) {
      if (runtimeStatus) {
        runtimeStatus.textContent = active.backend;
      }
      if (runtimeSelect && Array.from(runtimeSelect.options).some((option) => option.value === active.backend)) {
        runtimeSelect.value = active.backend;
      }
    }
    if (active) {
      ensureModelOption(active.modelId);
      if (modelSelect) {
        modelSelect.value = active.modelId;
      }
    }
    globalThis.dispatchEvent(new CustomEvent("rigspark:text-model-active", {
      detail: { modelId: active?.modelId || null },
    }));
  }

  function verdictLabel(verdict, throughput) {
    if (verdict !== "no" && throughput?.known === false) {
      return "Speed unknown";
    }
    if (verdict === "yes") {
      return "Runs well";
    }
    if (verdict === "slow") {
      return "Runs slowly";
    }
    return "Won't fit";
  }

  function selectedContext() {
    if (contextWindow?.value === "65536") return 65536;
    if (contextWindow?.value !== "custom") return undefined;
    if (!contextTokens?.reportValidity()) throw new Error("Invalid context size");
    const tokens = Number(contextTokens.value);
    if (!Number.isInteger(tokens) || tokens < 1 || tokens > 10000000) throw new Error("Invalid context size");
    return tokens;
  }

  // Installed models attach to a running daemon, whose cache settings rigspark cannot change.
  function selectedKvCache() {
    if (modelSource?.value === "installed") return undefined;
    const value = kvCache?.value;
    return value && value !== "f16" ? value : undefined;
  }

  function cacheLabel(cache) {
    const kv = cache.kvK === cache.kvV ? cache.kvK : `K ${cache.kvK} / V ${cache.kvV}`;
    return `KV ${kv}, flash attention ${cache.flashAttention}`;
  }

  async function startModel(id, button, backend, extra = {}) {
    if (modelError) {
      modelError.hidden = true;
    }
    const original = button.textContent;
    button.disabled = true;
    button.textContent = "Starting…";
    try {
      const payload = backend ? { model: id, backend } : { model: id };
      Object.assign(payload, extra);
      const context = selectedContext() ?? extra.context;
      if (context !== undefined) payload.context = context;
      if (modelBypass?.checked) payload.bypass = true;
      if (!payload.installed) {
        const kv = selectedKvCache();
        if (kv) payload.kvCache = kv;
      }
      if (payload.installed) {
        if (!installedPort?.reportValidity()) throw new Error("Invalid Ollama port");
        payload.port = Number(installedPort.value);
        if (!payload.bypass) throw new Error("Installed models require explicit bypass consent");
      }
      if ((payload.bypass || payload.context !== undefined || payload.kvCache) && !globalThis.confirm(
        `Start ${id} at ${payload.context ?? "default"} context?${payload.bypass ? " Estimated fit may be wrong or unknown; memory exhaustion or CPU offload is possible." : ""}${payload.installed ? " Local manifest integrity will be checked, not catalog provenance." : " Weight integrity checks remain enabled."}${payload.context !== undefined && payload.backend === "ollama" ? " A separate runtime model tag will be created; the source tag stays unchanged." : ""}${payload.kvCache ? ` KV cache ${payload.kvCache} applies only to a runtime rigspark starts; an already running server is refused, not changed.` : ""}`,
      )) return;
      const response = await globalThis.fetch("/api/models/up", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `request failed (${response.status})`);
      }
      renderActive(data.active);
      switchView("chat");
      await loadStatus();
    } catch (error) {
      if (modelError) {
        modelError.hidden = false;
        modelError.textContent = `Could not start ${id}: ${error.message}`;
      }
    } finally {
      button.disabled = false;
      button.textContent = original;
    }
  }

  function renderRecommended(models, active) {
    if (!recommendedList) {
      return;
    }
    recommendedList.innerHTML = "";
    if (modelsFitOnly?.checked) models = models.filter((model) => model.verdict !== "no" && model.contextFitKnown !== false);
    if (!models.length) {
      const empty = document.createElement("div");
      empty.className = "recommended-empty";
      empty.textContent = modelsRecency?.value
        ? `No models released or added in the last ${modelsRecency.value === "1" ? "month" : `${modelsRecency.value} months`}.`
        : "No recommended models for this machine.";
      recommendedList.appendChild(empty);
      return;
    }
    for (const model of models) {
      ensureModelOption(model.id);
      const isActive = active && active.modelId === model.id;
      const card = document.createElement("article");
      card.className = "model-card-item";
      card.dataset.verdict = model.verdict;

      const open = document.createElement("button");
      open.type = "button";
      open.className = "model-card-open";
      open.setAttribute("aria-label", `View performance details for ${model.id}`);
      open.addEventListener("click", () => renderModelDetail(model, active));

      const face = modelFace(model.verdict === "yes" ? "yes" : model.verdict === "slow" ? "slow" : "no");

      const head = document.createElement("span");
      head.className = "model-card-head";
      const title = document.createElement("span");
      title.className = "model-card-title";
      title.textContent = model.id;
      const meta = document.createElement("span");
      meta.className = "model-card-meta";
      const context = model.contextTokens
        ? model.contextFitKnown === false
          ? ` · ${formatTokens(model.contextTokens)} context tokens · context fit unknown`
          : ` · ${formatTokens(model.contextTokens)} context tokens${model.kvPrecision && model.kvPrecision !== "fp16" ? ` · KV ${model.kvPrecision}` : ""}`
        : "";
      const origin = model.provenance === "auto" ? " · auto-sourced" : "";
      meta.textContent = `${model.params} · ${model.quant} · ${formatSize(model.diskBytes)}${context}${origin}`;
      head.append(title, meta);

      const fit = document.createElement("span");
      fit.className = "model-card-fit";
      const badge = document.createElement("span");
      badge.className = `verdict-badge verdict-${model.verdict}`;
      badge.textContent = verdictLabel(model.verdict, model.throughput);
      fit.appendChild(badge);
      // Only draw the memory bar when both figures are sourced; never guess a fill level.
      if (Number.isFinite(model.requiredBytes) && Number.isFinite(model.usableBytes) && model.usableBytes > 0) {
        const share = model.requiredBytes / model.usableBytes;
        const bar = document.createElement("span");
        bar.className = "fit-bar";
        bar.title = `${Math.round(share * 100)}% of usable memory`;
        const fill = document.createElement("i");
        fill.style.width = `${Math.min(Math.max(share, 0), 1) * 100}%`;
        bar.appendChild(fill);
        fit.appendChild(bar);
      }

      const speed = document.createElement("span");
      speed.className = "model-card-speed";
      speed.textContent = formatThroughput(model.throughput);

      const actions = document.createElement("div");
      actions.className = "model-card-actions";
      const backends = Array.isArray(model.backends) ? model.backends : [];
      let backendSelect = null;
      if (backends.length > 1) {
        backendSelect = document.createElement("select");
        backendSelect.className = "model-backend-select";
        backendSelect.setAttribute("aria-label", `Runtime for ${model.id}`);
        const auto = document.createElement("option");
        auto.value = "";
        auto.textContent = "Auto";
        backendSelect.appendChild(auto);
        for (const name of backends) {
          const option = document.createElement("option");
          option.value = name;
          option.textContent = name;
          backendSelect.appendChild(option);
        }
        actions.appendChild(backendSelect);
      }
      const button = document.createElement("button");
      button.type = "button";
      if (isActive && model.contextTokens === undefined && selectedContext() === undefined && (active.cache ? active.cache.kvK : "f16") === (selectedKvCache() ?? "f16")) {
        button.textContent = "Running";
        button.className = "is-running";
        button.disabled = true;
      } else {
        button.textContent = model.verdict === "no" ? "Start anyway" : "Start";
        button.addEventListener("click", () =>
          startModel(model.id, button, backendSelect ? backendSelect.value : "", model.contextTokens !== undefined ? { context: model.contextTokens } : {}),
        );
      }
      actions.appendChild(button);

      open.append(face, head, fit, speed);
      card.appendChild(open);
      card.appendChild(actions);
      recommendedList.appendChild(card);
    }
  }

  function modelFace(name) {
    if (globalThis.RigSparkUI) {
      return globalThis.RigSparkUI.faceImage(name, "model-face");
    }
    const image = document.createElement("img");
    image.className = "model-face";
    image.src = `/static/brand/sparky-${name}-dark.svg`;
    image.alt = "";
    return image;
  }

  if (modelDetailBack) {
    modelDetailBack.addEventListener("click", showModelCatalog);
  }

  async function loadActive(render = true) {
    try {
      const response = await globalThis.fetch("/api/models/active");
      if (!response.ok) {
        return undefined;
      }
      const data = await response.json();
      if (render) {
        renderActive(data.active);
      }
      return data.active;
    } catch {
      return undefined;
    }
  }

  async function loadModels() {
    void loadCatalogStatus();
    const requestSequence = ++modelsRequestSequence;
    if (modelError) {
      modelError.hidden = true;
    }
    const active = await loadActive(false);
    if (requestSequence !== modelsRequestSequence) {
      return;
    }
    if (active === undefined) {
      if (modelError) {
        modelError.hidden = false;
        modelError.textContent = "Could not load active model status.";
      }
      return;
    }
    renderActive(active);
    if (!recommendedList) {
      return;
    }
    try {
      const runtime = selectedRuntime();
      const params = new globalThis.URLSearchParams();
      const installed = modelSource?.value === "installed";
      if (!installed) params.set("limit", "100");
      if (!installed && modelsRecency?.value) params.set("month", modelsRecency.value);
      if (runtime && !installed) {
        params.set("runtime", runtime);
      }
      const context = selectedContext();
      if (context !== undefined) {
        params.set("tokens", String(context));
      } else if (contextWindow && contextWindow.value && !installed) {
        params.set("context", contextWindow.value);
      }
      const kv = selectedKvCache();
      if (kv) params.set("kvCache", kv);
      if (installed) {
        if (!installedPort?.reportValidity()) throw new Error("Invalid Ollama port");
        params.set("port", installedPort.value);
      }
      const response = await globalThis.fetch(`/api/models/${installed ? "installed" : "recommended"}?${params.toString()}`);
      if (!response.ok) {
        throw new Error(`request failed (${response.status})`);
      }
      const data = await response.json();
      if (requestSequence !== modelsRequestSequence) {
        return;
      }
      const models = Array.isArray(data.models) ? data.models : [];
      if (installed) renderInstalled(models);
      else renderRecommended(models, active);
    } catch (error) {
      if (requestSequence !== modelsRequestSequence) {
        return;
      }
      recommendedList.innerHTML = "";
      const empty = document.createElement("div");
      empty.className = "recommended-empty";
      empty.textContent = `Could not load recommendations: ${error.message}`;
      recommendedList.appendChild(empty);
    }
  }

  if (refreshModels) {
    refreshModels.addEventListener("click", () => loadModels());
  }

  const catalogStatus = document.querySelector("#catalog-status");
  const updateCatalog = document.querySelector("#update-catalog");
  const catalogUpdateError = document.querySelector("#catalog-update-error");
  const catalogUpdateResult = document.querySelector("#catalog-update-result");
  let catalogUpdating = false;
  let catalogStatusSequence = 0;
  let currentCatalogStatus = null;

  function renderCatalogStatus(status) {
    if (!catalogStatus || !updateCatalog) return;
    currentCatalogStatus = status;
    const date = status.publishedAt || status.generatedAt;
    const revision = status.revision ? `, revision ${status.revision}` : "";
    catalogStatus.textContent = `Catalog: ${status.source}, ${String(date || "unknown").slice(0, 10)}${revision} (${status.modelCount} models)`;
    catalogStatus.title = `Snapshot digest: ${status.digest}`;
    updateCatalog.disabled = catalogUpdating || !status.updatesConfigured;
    updateCatalog.title = status.updatesConfigured ? "Download the latest signed catalog" : "Catalog updates unavailable: production signing key not provisioned";
    if (Array.isArray(status.warnings) && status.warnings.length) {
      catalogUpdateError.textContent = status.warnings.join(" ");
      catalogUpdateError.hidden = false;
    }
  }

  async function loadCatalogStatus() {
    if (!catalogStatus || catalogUpdating) return;
    const sequence = ++catalogStatusSequence;
    try {
      const response = await globalThis.fetch("/api/catalog/status");
      if (!response.ok) throw new Error("Catalog status unavailable");
      const data = await response.json();
      if (sequence === catalogStatusSequence && !catalogUpdating) renderCatalogStatus(data.catalog);
    } catch {
      if (sequence === catalogStatusSequence && !catalogUpdating) catalogStatus.textContent = "Catalog status unavailable";
    }
  }

  updateCatalog?.addEventListener("click", async () => {
    if (catalogUpdating) return;
    const previousCatalog = currentCatalogStatus;
    catalogUpdating = true;
    ++catalogStatusSequence;
    updateCatalog.disabled = true;
    updateCatalog.textContent = "Updating...";
    catalogUpdateError.hidden = true;
    catalogUpdateResult.hidden = true;
    catalogStatus.textContent = "Downloading and verifying catalog...";
    try {
      const response = await globalThis.fetch("/api/catalog/update", {
        method: "POST",
        headers: workspaceHeaders({ "Content-Type": "application/json" }),
        body: "{}",
      });
      const data = await response.json();
      if (!response.ok) throw new Error(typeof data.error === "string" ? data.error : "Catalog update failed");
      renderCatalogStatus(data.catalog);
      await loadModels();
      const unchanged = previousCatalog?.source === data.catalog.source
        && previousCatalog?.revision === data.catalog.revision
        && previousCatalog?.digest === data.catalog.digest;
      catalogUpdateResult.textContent = unchanged
        ? `Already using the latest published catalog: revision ${data.catalog.revision} (${data.catalog.modelCount} models).`
        : `Catalog updated to revision ${data.catalog.revision} (${data.catalog.modelCount} models).`;
      catalogUpdateResult.hidden = false;
    } catch (error) {
      catalogUpdateError.textContent = error.message;
      catalogUpdateError.hidden = false;
    } finally {
      catalogUpdating = false;
      updateCatalog.textContent = "Update catalog";
      updateCatalog.disabled = false;
      await loadCatalogStatus();
    }
  });

  if (contextWindow) {
    contextWindow.addEventListener("change", () => {
      if (contextTokensField) contextTokensField.hidden = contextWindow.value !== "custom";
      loadModels();
    });
  }

  function renderInstalled(models) {
    recommendedList.replaceChildren();
    const visible = modelsFitOnly?.checked ? models.filter((model) => model.fit === "yes") : models;
    for (const model of visible) {
      const row = document.createElement("article");
      row.className = "model-card-item model-card-installed";
      if (model.fit === "yes" || model.fit === "no") row.dataset.verdict = model.fit;
      const head = document.createElement("div");
      head.className = "model-card-head";
      const title = document.createElement("h3");
      title.className = "model-card-title";
      title.textContent = model.id;
      const details = document.createElement("p");
      details.className = "model-card-meta";
      const fit = model.fit === "unknown" ? "Context fit unknown" : model.fit === "yes" ? "Estimated context fits" : "Estimated context exceeds limits";
      details.textContent = `${model.quant ?? "Unknown quant"} · ${formatSize(model.sizeBytes)} weights · ${model.weightsFit ? "weights fit" : "weights exceed budget"} · ${model.context ?? "default"} context · ${fit} · ${model.memoryKind.toUpperCase()} budget ${formatSize(model.usableBytes)} · Throughput unknown`;
      head.append(title, details);
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = "Start";
      button.disabled = !modelBypass?.checked;
      button.title = "Requires explicit bypass consent; integrity checks remain enabled";
      button.addEventListener("click", () => startModel(model.id, button, "ollama", { installed: true }));
      const actions = document.createElement("div");
      actions.className = "model-card-actions";
      actions.append(button);
      row.append(modelFace(model.fit === "yes" ? "yes" : model.fit === "no" ? "no" : "wow"), head, actions);
      recommendedList.append(row);
    }
    if (visible.length === 0) recommendedList.textContent = "No installed models match.";
  }

  modelSource?.addEventListener("change", () => {
    const installed = modelSource.value === "installed";
    if (installedPortField) installedPortField.hidden = !installed;
    if (kvCacheField) kvCacheField.hidden = installed;
    if (installed && contextWindow && !["custom", "65536"].includes(contextWindow.value)) contextWindow.value = "65536";
    if (contextTokensField) contextTokensField.hidden = contextWindow?.value !== "custom";
    if (modelBypass) modelBypass.checked = false;
    loadModels();
  });
  for (const control of [contextTokens, installedPort, modelsFitOnly, modelsRecency, modelBypass, kvCache]) {
    control?.addEventListener("change", () => loadModels());
  }

  if (refreshRuntime) {
    refreshRuntime.addEventListener("click", () => {
      loadHardware();
      loadRuntimeStatus();
    });
  }

  function connectorStatusLabel(status) {
    if (status === "connected") {
      return "Connected";
    }
    if (status === "connecting") {
      return "Connecting…";
    }
    if (status === "error") {
      return "Error";
    }
    return "Disconnected";
  }

  function renderConnectors(connectors) {
    if (!connectorList) {
      return;
    }
    connectorList.innerHTML = "";
    if (!connectors.length) {
      const empty = document.createElement("div");
      empty.className = "recommended-empty";
      empty.textContent = "No connectors yet. Add one above.";
      connectorList.appendChild(empty);
      return;
    }
    for (const connector of connectors) {
      const card = document.createElement("article");
      card.className = "connector-card";

      const head = document.createElement("div");
      head.className = "connector-card-head";
      const title = document.createElement("div");
      title.className = "connector-card-title";
      title.textContent = connector.name;
      const badge = document.createElement("span");
      badge.className = `connector-status connector-status-${connector.status}`;
      badge.textContent = connectorStatusLabel(connector.status);
      head.appendChild(title);
      head.appendChild(badge);

      const meta = document.createElement("div");
      meta.className = "connector-card-meta";
      meta.textContent = `${connector.transport} · ${connector.target}`;

      card.appendChild(head);
      card.appendChild(meta);

      if (connector.error) {
        const err = document.createElement("div");
        err.className = "connector-card-error";
        err.textContent = connector.error;
        card.appendChild(err);
      }

      if (connector.tools && connector.tools.length) {
        const tools = document.createElement("div");
        tools.className = "connector-tools";
        for (const tool of connector.tools) {
          const chip = document.createElement("span");
          chip.className = "connector-tool-chip";
          chip.textContent = tool.name;
          if (tool.description) {
            chip.title = tool.description;
          }
          tools.appendChild(chip);
        }
        card.appendChild(tools);
      }

      const actions = document.createElement("div");
      actions.className = "connector-card-actions";

      const connected = connector.status === "connected";
      const connecting = connector.status === "connecting";
      const toggle = document.createElement("div");
      toggle.className = `toggle${connected ? " on" : ""}${connecting ? " pending" : ""}`;
      toggle.setAttribute("role", "switch");
      toggle.setAttribute("aria-checked", connected ? "true" : "false");
      if (connecting) {
        toggle.setAttribute("aria-disabled", "true");
      }
      toggle.tabIndex = 0;

      const track = document.createElement("span");
      track.className = "toggle-track";
      const thumb = document.createElement("span");
      thumb.className = "toggle-thumb";
      track.appendChild(thumb);

      const toggleLabel = document.createElement("span");
      toggleLabel.className = "toggle-label";
      toggleLabel.textContent = connecting ? "Connecting…" : connected ? "Connected" : "Disconnected";

      toggle.appendChild(track);
      toggle.appendChild(toggleLabel);

      const fireToggle = () => {
        if (toggle.getAttribute("aria-disabled") === "true") {
          return;
        }
        connectorAction(connector.id, connected ? "disconnect" : "connect", toggle);
      };
      toggle.addEventListener("click", fireToggle);
      toggle.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          fireToggle();
        }
      });
      actions.appendChild(toggle);

      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "connector-remove";
      remove.textContent = "Remove";
      remove.addEventListener("click", () => removeConnector(connector.id, remove));
      actions.appendChild(remove);

      card.appendChild(actions);
      connectorList.appendChild(card);
    }
  }

  async function loadConnectors() {
    if (!connectorList) {
      return;
    }
    try {
      const response = await globalThis.fetch("/api/connectors");
      if (!response.ok) {
        throw new Error(`request failed (${response.status})`);
      }
      const data = await response.json();
      renderConnectors(Array.isArray(data.connectors) ? data.connectors : []);
      loadConnectorConfig(false);
    } catch (error) {
      connectorList.innerHTML = "";
      const empty = document.createElement("div");
      empty.className = "recommended-empty";
      empty.textContent = `Could not load connectors: ${error.message}`;
      connectorList.appendChild(empty);
    }
  }

  async function connectorAction(id, action, toggle) {
    if (connectorError) {
      connectorError.hidden = true;
    }
    toggle.setAttribute("aria-disabled", "true");
    toggle.classList.add("pending");
    const label = toggle.querySelector(".toggle-label");
    if (label) {
      label.textContent = action === "connect" ? "Connecting…" : "Disconnecting…";
    }
    try {
      const response = await globalThis.fetch(`/api/connectors/${encodeURIComponent(id)}/${action}`, {
        method: "POST",
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `request failed (${response.status})`);
      }
      await loadConnectors();
    } catch (error) {
      if (connectorError) {
        connectorError.hidden = false;
        connectorError.textContent = `Could not ${action}: ${error.message}`;
      }
      await loadConnectors();
    }
  }

  async function removeConnector(id, button) {
    if (connectorError) {
      connectorError.hidden = true;
    }
    button.disabled = true;
    try {
      const response = await globalThis.fetch(`/api/connectors/${encodeURIComponent(id)}`, {
        method: "DELETE",
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `request failed (${response.status})`);
      }
      await loadConnectors();
    } catch (error) {
      if (connectorError) {
        connectorError.hidden = false;
        connectorError.textContent = `Could not remove: ${error.message}`;
      }
      button.disabled = false;
    }
  }

  let connectorJsonDirty = false;

  function setConnectorJsonStatus(message, kind) {
    if (!connectorJsonStatus) {
      return;
    }
    if (!message) {
      connectorJsonStatus.hidden = true;
      connectorJsonStatus.textContent = "";
      connectorJsonStatus.classList.remove("banner-error", "banner-success");
      return;
    }
    connectorJsonStatus.hidden = false;
    connectorJsonStatus.textContent = message;
    connectorJsonStatus.classList.toggle("banner-error", kind === "error");
    connectorJsonStatus.classList.toggle("banner-success", kind === "success");
  }

  async function loadConnectorConfig(force) {
    if (!connectorJson) {
      return;
    }
    // Never clobber edits the user is in the middle of typing.
    if (!force && connectorJsonDirty) {
      return;
    }
    try {
      const response = await globalThis.fetch("/api/connectors/config");
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `request failed (${response.status})`);
      }
      connectorJson.value = JSON.stringify(data.config ?? {}, null, 2);
      connectorJsonDirty = false;
      if (force) {
        setConnectorJsonStatus("");
      }
    } catch (error) {
      setConnectorJsonStatus(`Could not load config: ${error.message}`, "error");
    }
  }

  async function applyConnectorConfig() {
    if (!connectorJson) {
      return;
    }
    let parsed;
    try {
      parsed = JSON.parse(connectorJson.value);
    } catch (error) {
      setConnectorJsonStatus(`Invalid JSON: ${error.message}`, "error");
      return;
    }
    if (connectorJsonApply) {
      connectorJsonApply.disabled = true;
    }
    try {
      const response = await globalThis.fetch("/api/connectors/config", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(parsed),
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `request failed (${response.status})`);
      }
      const count = Array.isArray(data.connectors) ? data.connectors.length : 0;
      connectorJsonDirty = false;
      setConnectorJsonStatus(`Applied — ${count} connector${count === 1 ? "" : "s"} saved.`, "success");
      await loadConnectors();
      await loadConnectorConfig(true);
    } catch (error) {
      setConnectorJsonStatus(`Could not apply: ${error.message}`, "error");
    } finally {
      if (connectorJsonApply) {
        connectorJsonApply.disabled = false;
      }
    }
  }

  if (connectorJson) {
    connectorJson.addEventListener("input", () => {
      connectorJsonDirty = true;
      setConnectorJsonStatus("");
    });
  }

  if (connectorJsonReload) {
    connectorJsonReload.addEventListener("click", () => loadConnectorConfig(true));
  }

  if (connectorJsonApply) {
    connectorJsonApply.addEventListener("click", () => applyConnectorConfig());
  }

  function updateConnectorFields() {
    if (!connectorTransport) {
      return;
    }
    const isStdio = connectorTransport.value === "stdio";
    if (connectorStdioFields) {
      connectorStdioFields.hidden = !isStdio;
    }
    if (connectorHttpFields) {
      connectorHttpFields.hidden = isStdio;
    }
  }

  if (connectorTransport) {
    connectorTransport.addEventListener("change", updateConnectorFields);
    updateConnectorFields();
  }

  if (refreshConnectors) {
    refreshConnectors.addEventListener("click", () => loadConnectors());
  }

  if (connectorForm) {
    connectorForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (connectorError) {
        connectorError.hidden = true;
      }
      const name = (document.querySelector("#connector-name")?.value || "").trim();
      const transport = connectorTransport ? connectorTransport.value : "stdio";
      let payload;
      if (transport === "http") {
        const url = (document.querySelector("#connector-url")?.value || "").trim();
        payload = { name, transport: "http", url };
      } else {
        const command = (document.querySelector("#connector-command")?.value || "").trim();
        const argsRaw = (document.querySelector("#connector-args")?.value || "").trim();
        const args = argsRaw.length ? argsRaw.split(/\s+/) : [];
        payload = { name, transport: "stdio", command, args };
      }
      try {
        const response = await globalThis.fetch("/api/connectors", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(payload),
        });
        const data = await response.json().catch(() => ({}));
        if (!response.ok) {
          throw new Error(data.error || `request failed (${response.status})`);
        }
        connectorForm.reset();
        updateConnectorFields();
        globalThis.dispatchEvent(new CustomEvent("rigspark:connector-added"));
        await loadConnectors();
      } catch (error) {
        if (connectorError) {
          connectorError.hidden = false;
          connectorError.textContent = `Could not add connector: ${error.message}`;
        }
      }
    });
  }

  if (prompt) {
    const autoGrow = () => {
      prompt.style.height = "auto";
      prompt.style.height = `${Math.min(prompt.scrollHeight, 240)}px`;
    };
    prompt.addEventListener("input", autoGrow);
    prompt.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        form.requestSubmit();
      }
    });
  }

  const selectedSkills = new Set();
  let agentsCache = [];
  let skillsCache = [];

  function libraryPath(kind) {
    return kind === "agent" ? "/api/agents" : "/api/skills";
  }

  function libraryEls(kind) {
    return kind === "agent"
      ? { form: agentForm, list: agentList, error: agentError, id: "#agent-edit-id", name: "#agent-name", desc: "#agent-desc", body: "#agent-body", enabled: "#agent-enabled" }
      : { form: skillForm, list: skillList, error: skillError, id: "#skill-edit-id", name: "#skill-name", desc: "#skill-desc", body: "#skill-body", enabled: "#skill-enabled" };
  }

  function setLibraryError(kind, message) {
    const els = libraryEls(kind);
    if (!els.error) return;
    if (!message) {
      els.error.hidden = true;
      els.error.textContent = "";
      return;
    }
    els.error.hidden = false;
    els.error.textContent = message;
  }

  function renderLibraryList(kind, items) {
    const listEl = libraryEls(kind).list;
    if (!listEl) return;
    listEl.innerHTML = "";
    if (!items.length) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = `No ${kind}s yet. Create one to use it in chat.`;
      listEl.appendChild(empty);
      return;
    }
    for (const item of items) {
      const card = document.createElement("article");
      card.className = "library-card";

      const head = document.createElement("div");
      head.className = "library-card-head";
      const title = document.createElement("div");
      title.className = "library-card-title";
      title.textContent = item.name;
      const badge = document.createElement("span");
      badge.className = `library-badge library-badge-${item.enabled ? "on" : "off"}`;
      badge.textContent = item.enabled ? "Enabled" : "Disabled";
      head.appendChild(title);
      head.appendChild(badge);
      card.appendChild(head);

      if (item.description) {
        const desc = document.createElement("div");
        desc.className = "library-card-desc";
        desc.textContent = item.description;
        card.appendChild(desc);
      }

      const actions = document.createElement("div");
      actions.className = "library-card-actions";

      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "ghost-btn";
      toggle.textContent = item.enabled ? "Disable" : "Enable";
      toggle.addEventListener("click", () => toggleLibraryItem(kind, item));

      const edit = document.createElement("button");
      edit.type = "button";
      edit.className = "ghost-btn";
      edit.textContent = "Edit";
      edit.addEventListener("click", () => openLibraryEditor(kind, item));

      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "connector-remove";
      remove.textContent = "Delete";
      remove.addEventListener("click", () => deleteLibraryItem(kind, item));

      actions.appendChild(toggle);
      actions.appendChild(edit);
      actions.appendChild(remove);
      card.appendChild(actions);
      listEl.appendChild(card);
    }
  }

  function renderAgentSkillsPicker(selectedIds) {
    if (!agentSkillsPicker) return;
    agentSkillsPicker.innerHTML = "";
    if (!skillsCache.length) {
      const empty = document.createElement("span");
      empty.className = "chip-empty";
      empty.textContent = "No skills yet — create a skill first.";
      agentSkillsPicker.appendChild(empty);
      return;
    }
    const chosen = new Set(selectedIds || []);
    for (const skill of skillsCache) {
      const label = document.createElement("label");
      label.className = "agent-skill-option";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.value = skill.id;
      box.checked = chosen.has(skill.id);
      const text = document.createElement("span");
      text.textContent = skill.name;
      label.appendChild(box);
      label.appendChild(text);
      agentSkillsPicker.appendChild(label);
    }
  }

  function openLibraryEditor(kind, item) {
    const els = libraryEls(kind);
    if (!els.form) return;
    els.form.hidden = false;
    document.querySelector(els.id).value = item ? item.id : "";
    document.querySelector(els.name).value = item ? item.name : "";
    document.querySelector(els.desc).value = item ? item.description : "";
    document.querySelector(els.body).value = item ? item.body : "";
    document.querySelector(els.enabled).checked = item ? item.enabled : true;
    if (kind === "agent") renderAgentSkillsPicker(item ? item.skills : []);
    setLibraryError(kind, "");
    document.querySelector(els.name).focus();
  }

  function closeLibraryEditor(kind) {
    const els = libraryEls(kind);
    if (els.form) els.form.hidden = true;
    setLibraryError(kind, "");
  }

  async function fetchLibrary(kind) {
    const response = await globalThis.fetch(libraryPath(kind));
    if (!response.ok) throw new Error(`request failed (${response.status})`);
    const data = await response.json();
    return Array.isArray(data.items) ? data.items : [];
  }

  async function loadLibrary() {
    try {
      agentsCache = await fetchLibrary("agent");
    } catch {
      agentsCache = [];
    }
    try {
      skillsCache = await fetchLibrary("skill");
    } catch {
      skillsCache = [];
    }
    renderLibraryList("agent", agentsCache);
    renderLibraryList("skill", skillsCache);
    populateChatLibrary();
  }

  async function submitLibrary(kind, event) {
    event.preventDefault();
    const els = libraryEls(kind);
    const name = (document.querySelector(els.name)?.value || "").trim();
    if (!name) {
      setLibraryError(kind, "A name is required.");
      return;
    }
    const id = (document.querySelector(els.id)?.value || "").trim();
    const payload = {
      name,
      description: (document.querySelector(els.desc)?.value || "").trim(),
      body: document.querySelector(els.body)?.value || "",
      enabled: Boolean(document.querySelector(els.enabled)?.checked),
    };
    if (kind === "agent" && agentSkillsPicker) {
      payload.skills = Array.from(agentSkillsPicker.querySelectorAll("input[type=checkbox]"))
        .filter((box) => box.checked)
        .map((box) => box.value);
    }
    try {
      const response = await globalThis.fetch(
        id ? `${libraryPath(kind)}/${encodeURIComponent(id)}` : libraryPath(kind),
        {
          method: id ? "PUT" : "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(payload),
        },
      );
      const data = await response.json().catch(() => ({}));
      if (!response.ok) {
        throw new Error(data.error || `request failed (${response.status})`);
      }
      closeLibraryEditor(kind);
      await loadLibrary();
    } catch (error) {
      setLibraryError(kind, `Could not save: ${error.message}`);
    }
  }

  async function toggleLibraryItem(kind, item) {
    try {
      const response = await globalThis.fetch(`${libraryPath(kind)}/${encodeURIComponent(item.id)}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ enabled: !item.enabled }),
      });
      if (!response.ok) {
        const data = await response.json().catch(() => ({}));
        throw new Error(data.error || `request failed (${response.status})`);
      }
      await loadLibrary();
    } catch (error) {
      setLibraryError(kind, `Could not update: ${error.message}`);
    }
  }

  async function deleteLibraryItem(kind, item) {
    try {
      const response = await globalThis.fetch(`${libraryPath(kind)}/${encodeURIComponent(item.id)}`, {
        method: "DELETE",
      });
      if (!response.ok) {
        const data = await response.json().catch(() => ({}));
        throw new Error(data.error || `request failed (${response.status})`);
      }
      if (kind === "skill") selectedSkills.delete(item.id);
      if (kind === "agent" && agentSelect && agentSelect.value === item.id) agentSelect.value = "";
      await loadLibrary();
    } catch (error) {
      setLibraryError(kind, `Could not delete: ${error.message}`);
    }
  }

  function populateChatLibrary() {
    const enabledAgents = agentsCache.filter((a) => a.enabled);
    if (agentSelect) {
      const previous = agentSelect.value;
      agentSelect.innerHTML = "";
      const none = document.createElement("option");
      none.value = "";
      none.textContent = "No agent";
      agentSelect.appendChild(none);
      for (const agent of enabledAgents) {
        const option = document.createElement("option");
        option.value = agent.id;
        option.textContent = agent.name;
        agentSelect.appendChild(option);
      }
      agentSelect.value = enabledAgents.some((a) => a.id === previous) ? previous : "";
    }

    const enabledSkills = skillsCache.filter((s) => s.enabled);
    for (const id of Array.from(selectedSkills)) {
      if (!enabledSkills.some((s) => s.id === id)) selectedSkills.delete(id);
    }
    if (skillChips) {
      skillChips.innerHTML = "";
      if (!enabledSkills.length) {
        const empty = document.createElement("span");
        empty.className = "chip-empty";
        empty.textContent = "No skills";
        skillChips.appendChild(empty);
      } else {
        for (const skill of enabledSkills) {
          const chip = document.createElement("button");
          chip.type = "button";
          chip.className = `skill-chip${selectedSkills.has(skill.id) ? " active" : ""}`;
          chip.textContent = skill.name;
          if (skill.description) chip.title = skill.description;
          chip.addEventListener("click", () => {
            if (selectedSkills.has(skill.id)) selectedSkills.delete(skill.id);
            else selectedSkills.add(skill.id);
            chip.classList.toggle("active");
          });
          skillChips.appendChild(chip);
        }
      }
    }
  }

  let sessionsEnabled = false;
  let currentSessionId = null;

  function clearMessages() {
    messages.innerHTML = "";
  }

  function showEmptyMessages() {
    clearMessages();
    messages.appendChild(buildWelcome());
    updateContextUsage();
  }

  const WELCOME_SUGGESTIONS = [
    { title: "Explain a concept", prompt: "Explain how transformer attention works, step by step." },
    { title: "Summarize a file", prompt: "Summarize the file I attach and list the key functions." },
    { title: "Review my code", prompt: "Review the attached code for bugs and suggest improvements." },
    { title: "Draft something", prompt: "Draft a concise commit message for the changes I describe." },
  ];

  function buildWelcome() {
    const empty = document.createElement("div");
    empty.className = "messages-empty";

    const hero = document.createElement("div");
    hero.className = "welcome";

    const mark = document.createElement("img");
    mark.className = "welcome-mark";
    mark.src = "/static/brand/3d/sparky-studio.webp";
    mark.alt = "";
    mark.decoding = "async";

    const title = document.createElement("h2");
    title.className = "welcome-title";
    title.textContent = "Ready when you are.";

    const sub = document.createElement("p");
    sub.className = "welcome-sub";
    sub.textContent = "Chat with a model running entirely on this machine. Nothing leaves the device.";

    const grid = document.createElement("div");
    grid.className = "welcome-suggestions";
    for (const item of WELCOME_SUGGESTIONS) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "suggestion";
      btn.title = item.prompt;
      btn.textContent = item.title;
      btn.addEventListener("click", () => {
        prompt.value = item.prompt;
        prompt.dispatchEvent(new globalThis.Event("input", { bubbles: true }));
        prompt.focus();
      });
      grid.appendChild(btn);
    }

    hero.append(mark, title, sub, grid);
    empty.appendChild(hero);
    return empty;
  }

  function renderSessionMessages(list) {
    clearMessages();
    if (!list.length) {
      showEmptyMessages();
      return;
    }
    for (const message of list) {
      if (message.role === "user" || message.role === "assistant") {
        addMessage(message.role, message.content);
      }
    }
  }

  function markActiveSession() {
    if (!sessionList) {
      return;
    }
    for (const item of sessionList.querySelectorAll(".rail-session-item")) {
      item.classList.toggle("active", item.dataset.sessionId === currentSessionId);
    }
  }

  async function activateSession(id) {
    if (activeRun) {
      return;
    }
    try {
      const activated = await globalThis.fetch(`/api/sessions/${encodeURIComponent(id)}/activate`, {
        method: "POST",
      });
      if (!activated.ok) {
        return;
      }
      currentSessionId = id;
      const res = await globalThis.fetch(`/api/sessions/${encodeURIComponent(id)}/messages?limit=500`);
      const data = await res.json().catch(() => ({ messages: [] }));
      renderSessionMessages(Array.isArray(data.messages) ? data.messages : []);
      markActiveSession();
      await loadStatus();
    } catch {
      // Leave the current view untouched on failure.
    }
  }

  function sessionSubText(session) {
    const count = session.messageCount || 0;
    return `${count} ${count === 1 ? "message" : "messages"}`;
  }

  function sessionActionButton(label, handler) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "rail-session-action";
    button.textContent = label;
    button.setAttribute("aria-label", label);
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      handler();
    });
    return button;
  }

  function sessionBucket(iso) {
    const then = new Date(iso).getTime();
    if (Number.isNaN(then)) {
      return "Earlier";
    }
    const startOfToday = new Date();
    startOfToday.setHours(0, 0, 0, 0);
    const day = 86400000;
    if (then >= startOfToday.getTime()) {
      return "Today";
    }
    if (then >= startOfToday.getTime() - day) {
      return "Yesterday";
    }
    if (then >= Date.now() - 7 * day) {
      return "Previous 7 days";
    }
    return "Earlier";
  }

  // Pins live in the browser only (no server/store change).
  const PIN_KEY = "llmup.pinnedSessions";
  function loadPinned() {
    try {
      const raw = globalThis.localStorage.getItem(PIN_KEY);
      return new Set(raw ? JSON.parse(raw) : []);
    } catch {
      return new Set();
    }
  }
  let pinnedSessions = loadPinned();
  function savePinned() {
    try {
      globalThis.localStorage.setItem(PIN_KEY, JSON.stringify(Array.from(pinnedSessions)));
    } catch {
      // Storage unavailable; pins are best-effort.
    }
  }
  function togglePin(id) {
    if (pinnedSessions.has(id)) {
      pinnedSessions.delete(id);
    } else {
      pinnedSessions.add(id);
    }
    savePinned();
    loadSessions(sessionSearch ? sessionSearch.value : "");
  }

  function buildSessionItem(session) {
    const item = document.createElement("div");
    item.className = `rail-session-item${session.id === currentSessionId ? " active" : ""}`;
    item.dataset.sessionId = session.id;

    const open = document.createElement("button");
    open.type = "button";
    open.className = "rail-session-open";
    const title = document.createElement("span");
    title.className = "rail-session-title";
    title.textContent = session.title || "Untitled";
    const sub = document.createElement("span");
    sub.className = "rail-session-sub";
    sub.textContent = sessionSubText(session);
    open.append(title, sub);
    open.addEventListener("click", () => activateSession(session.id));

    const actions = document.createElement("div");
    actions.className = "rail-session-actions";
    actions.append(
      sessionActionButton(pinnedSessions.has(session.id) ? "Unpin" : "Pin", () => togglePin(session.id)),
      sessionActionButton("Rename", () => renameSession(session)),
      sessionActionButton("Delete", () => deleteSession(session)),
    );

    item.append(open, actions);
    return item;
  }

  function renderSessions(sessions) {
    if (!sessionList) {
      return;
    }
    sessionList.innerHTML = "";
    if (!sessions.length) {
      const empty = document.createElement("div");
      empty.className = "rail-session-empty";
      empty.textContent = "No saved chats yet.";
      sessionList.appendChild(empty);
      return;
    }
    const addGroup = (name) => {
      const header = document.createElement("div");
      header.className = "rail-session-group";
      header.textContent = name;
      sessionList.appendChild(header);
    };
    const pinned = sessions.filter((s) => pinnedSessions.has(s.id));
    const rest = sessions.filter((s) => !pinnedSessions.has(s.id));
    if (pinned.length) {
      addGroup("Pinned");
      for (const session of pinned) {
        sessionList.appendChild(buildSessionItem(session));
      }
    }
    let currentBucket = null;
    for (const session of rest) {
      const bucket = sessionBucket(session.updatedAt);
      if (bucket !== currentBucket) {
        currentBucket = bucket;
        addGroup(bucket);
      }
      sessionList.appendChild(buildSessionItem(session));
    }
  }

  async function loadSessions(query) {
    if (!sessionsEnabled) {
      return;
    }
    const trimmed = query ? query.trim() : "";
    const url = trimmed.length ? `/api/sessions?q=${encodeURIComponent(trimmed)}` : "/api/sessions";
    try {
      const res = await globalThis.fetch(url);
      if (res.status === 404) {
        sessionsEnabled = false;
        return;
      }
      const data = await res.json().catch(() => ({}));
      if (trimmed.length) {
        renderSessions((data.results || []).map((result) => result.summary));
      } else {
        if (typeof data.activeSessionId === "string") {
          currentSessionId = data.activeSessionId;
        }
        renderSessions(data.sessions || []);
      }
    } catch {
      // Keep the last-rendered list on transient failures.
    }
  }

  function refreshSessionsIfEnabled() {
    if (!sessionsEnabled) {
      return undefined;
    }
    return loadSessions(sessionSearch && sessionSearch.value ? sessionSearch.value : "");
  }

  async function newChat() {
    if (activeRun) {
      return;
    }
    try {
      const res = await globalThis.fetch("/api/sessions", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });
      if (!res.ok) {
        return;
      }
      const data = await res.json().catch(() => ({}));
      currentSessionId = data.session ? data.session.id : null;
      showEmptyMessages();
      if (sessionSearch) {
        sessionSearch.value = "";
      }
      await loadSessions();
      await loadStatus();
    } catch {
      // Ignore; the user can retry.
    }
  }

  async function renameSession(session) {
    const next = globalThis.prompt("Rename chat", session.title || "");
    if (next === null) {
      return;
    }
    const title = next.trim();
    if (!title) {
      return;
    }
    try {
      await globalThis.fetch(`/api/sessions/${encodeURIComponent(session.id)}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
      });
      await loadSessions(sessionSearch ? sessionSearch.value : "");
    } catch {
      // Ignore rename failures.
    }
  }

  async function deleteSession(session) {
    if (!globalThis.confirm(`Delete "${session.title || "this chat"}"?`)) {
      return;
    }
    try {
      await globalThis.fetch(`/api/sessions/${encodeURIComponent(session.id)}`, { method: "DELETE" });
      if (currentSessionId === session.id) {
        currentSessionId = null;
        showEmptyMessages();
      }
      await loadSessions(sessionSearch ? sessionSearch.value : "");
    } catch {
      // Ignore delete failures.
    }
  }

  async function initSessions() {
    try {
      const res = await globalThis.fetch("/api/sessions");
      if (res.status === 404) {
        sessionsEnabled = false;
        loadHistory();
        return;
      }
      sessionsEnabled = true;
      if (sessionCurrent) {
        sessionCurrent.hidden = true;
      }
      if (sessionNew) {
        sessionNew.hidden = false;
      }
      if (sessionSearch) {
        sessionSearch.hidden = false;
      }
      const data = await res.json().catch(() => ({}));
      if (typeof data.activeSessionId === "string") {
        currentSessionId = data.activeSessionId;
      }
      renderSessions(data.sessions || []);
      if (currentSessionId) {
        await activateSession(currentSessionId);
      }
    } catch {
      sessionsEnabled = false;
      loadHistory();
    }
  }

  if (sessionNew) {
    sessionNew.addEventListener("click", () => newChat());
  }
  if (sessionSearch) {
    let searchTimer = null;
    sessionSearch.addEventListener("input", () => {
      if (searchTimer) {
        globalThis.clearTimeout(searchTimer);
      }
      searchTimer = globalThis.setTimeout(() => loadSessions(sessionSearch.value), 200);
    });
  }

  if (refreshLibrary) refreshLibrary.addEventListener("click", () => loadLibrary());
  if (agentNew) agentNew.addEventListener("click", () => openLibraryEditor("agent"));
  if (agentCancel) agentCancel.addEventListener("click", () => closeLibraryEditor("agent"));
  if (agentForm) agentForm.addEventListener("submit", (event) => submitLibrary("agent", event));
  if (skillNew) skillNew.addEventListener("click", () => openLibraryEditor("skill"));
  if (skillCancel) skillCancel.addEventListener("click", () => closeLibraryEditor("skill"));
  if (skillForm) skillForm.addEventListener("submit", (event) => submitLibrary("skill", event));
  if (agentSelect) {
    agentSelect.addEventListener("change", () => {
      const agent = agentsCache.find((a) => a.id === agentSelect.value);
      if (agent) {
        for (const skillId of agent.skills) {
          if (skillsCache.some((s) => s.id === skillId && s.enabled)) selectedSkills.add(skillId);
        }
      }
      populateChatLibrary();
    });
  }

  // --- Workspace context (@file, ranges, search, terminal/diag/git) 32.6/32.7 ---
  let workspaceRootId = null;
  let calculatorHtml = "";
  const selectedAttachments = [];
  const selectedSources = [];
  let pickerResults = [];
  let pickerIndex = -1;
  let pickerReturnFocus = null;

  function metaToken() {
    const el = document.querySelector('meta[name="llmup-token"]');
    return el ? el.getAttribute("content") || "" : "";
  }
  const workspaceToken = metaToken();

  function workspaceHeaders(extra) {
    return Object.assign({ "X-LLMUP-Token": workspaceToken }, extra || {});
  }

  function attachmentRefs() {
    return selectedAttachments.map((a) => ({
      workspaceId: a.workspaceId,
      path: a.path,
      ...(a.startLine ? { startLine: a.startLine } : {}),
      ...(a.endLine ? { endLine: a.endLine } : {}),
    }));
  }

  function contextSourceRefs() {
    return selectedSources.map((s) =>
      s.kind === "git"
        ? { kind: "git", workspaceId: s.workspaceId, mode: s.mode }
        : { kind: s.kind, label: s.label, content: s.content },
    );
  }

  function baseName(path) {
    const parts = path.split("/");
    return parts[parts.length - 1] || path;
  }

  function addContextLedger(items) {
    if (!items.length) {
      return;
    }
    const empty = messages.querySelector(".messages-empty");
    if (empty) {
      empty.remove();
    }
    const included = items.filter((i) => i.included).length;
    const row = document.createElement("div");
    row.className = "context-ledger";
    const head = document.createElement("span");
    head.className = "context-ledger-head";
    head.textContent = `Context: ${included} of ${items.length} item${items.length === 1 ? "" : "s"} sent`;
    const list = document.createElement("ul");
    list.className = "context-ledger-list";
    for (const item of items) {
      const li = document.createElement("li");
      li.className = item.included ? "context-ledger-item" : "context-ledger-item excluded";
      const name = item.label || item.path || "context";
      const range = item.range ? ` (lines ${item.range.startLine}-${item.range.endLine})` : "";
      const status = item.included ? "" : " — excluded (context budget)";
      li.textContent = `${name}${range}${status}`;
      list.append(li);
    }
    row.append(head, list);
    messages.append(row);
  }

  // Render an inert diff review: changed files, +/- counts, warnings, and
  // navigable hunks. Collapsed by default; no file is modified by viewing.
  function renderEditReview(review, workspaceId, operations, onApplied) {
    if (!review || !Array.isArray(review.files) || !review.files.length) {
      return;
    }
    const follow = nearBottom();
    const empty = messages.querySelector(".messages-empty");
    if (empty) {
      empty.remove();
    }
    const totals = review.files.reduce(
      (acc, file) => ({ added: acc.added + (file.added || 0), removed: acc.removed + (file.removed || 0) }),
      { added: 0, removed: 0 },
    );
    const details = document.createElement("details");
    details.className = "edit-review";
    const summary = document.createElement("summary");
    summary.className = "edit-review-summary";
    summary.textContent = `Proposed edits — ${review.files.length} file${review.files.length === 1 ? "" : "s"} · +${totals.added} −${totals.removed} (review only, nothing changed)`;
    details.appendChild(summary);
    for (const warning of review.warnings || []) {
      const note = document.createElement("div");
      note.className = "edit-review-warning";
      note.textContent = warning;
      details.appendChild(note);
    }
    for (const file of review.files) {
      details.appendChild(renderEditFile(file));
    }
    // Apply is only offered when the server echoed an applicable proposal and a
    // non-delete change exists (delete apply is disabled).
    const applicable =
      workspaceId &&
      Array.isArray(operations) &&
      operations.length > 0 &&
      review.files.every((file) => file.op !== "delete");
    if (applicable) {
      details.appendChild(renderApplyBar(workspaceId, operations, onApplied));
    }
    messages.appendChild(details);
    if (follow) {
      autoScroll(true);
    }
  }

  function renderApplyBar(workspaceId, operations, onApplied) {
    const bar = document.createElement("div");
    bar.className = "edit-apply-bar";
    const status = document.createElement("span");
    status.className = "edit-apply-status";
    const apply = document.createElement("button");
    apply.type = "button";
    apply.className = "edit-apply-btn";
    apply.textContent = "Apply edits";
    apply.addEventListener("click", async () => {
      apply.disabled = true;
      status.textContent = "Applying…";
      try {
        const res = await globalThis.fetch("/api/workspace/edits/apply", {
          method: "POST",
          headers: workspaceHeaders({ "Content-Type": "application/json" }),
          body: JSON.stringify({ workspaceId, operations }),
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok || !data.result) {
          status.textContent = data.error || "Apply failed.";
          apply.disabled = false;
          return;
        }
        apply.remove();
        status.textContent = "Applied.";
        bar.appendChild(renderRevertButton(data.result.applicationId));
        onApplied?.(data.result);
      } catch {
        status.textContent = "Apply failed.";
        apply.disabled = false;
      }
    });
    bar.append(apply, status);
    return bar;
  }

  function renderRevertButton(applicationId) {
    const revert = document.createElement("button");
    revert.type = "button";
    revert.className = "edit-revert-btn";
    revert.textContent = "Revert";
    revert.addEventListener("click", async () => {
      revert.disabled = true;
      try {
        const res = await globalThis.fetch("/api/workspace/edits/revert", {
          method: "POST",
          headers: workspaceHeaders({ "Content-Type": "application/json" }),
          body: JSON.stringify({ applicationId }),
        });
        const data = await res.json().catch(() => ({}));
        revert.textContent = res.ok && data.result ? `Reverted (${data.result.reverted.length})` : "Revert failed";
      } catch {
        revert.textContent = "Revert failed";
      }
    });
    return revert;
  }

  function renderEditFile(file) {
    const wrap = document.createElement("div");
    wrap.className = "edit-file";
    const head = document.createElement("div");
    head.className = "edit-file-head";
    const badge = document.createElement("span");
    badge.className = `edit-op edit-op-${file.op}`;
    badge.textContent = file.op;
    const path = document.createElement("span");
    path.className = "edit-file-path";
    path.textContent = file.path;
    const counts = document.createElement("span");
    counts.className = "edit-file-counts";
    counts.textContent = `+${file.added || 0} −${file.removed || 0}`;
    head.append(badge, path, counts);
    wrap.appendChild(head);
    for (const warning of file.warnings || []) {
      const note = document.createElement("div");
      note.className = "edit-review-warning";
      note.textContent = warning;
      wrap.appendChild(note);
    }
    for (const hunk of file.hunks || []) {
      wrap.appendChild(renderDiffHunk(hunk));
    }
    return wrap;
  }

  function renderDiffHunk(hunk) {
    const block = document.createElement("div");
    block.className = "diff-hunk";
    const header = document.createElement("div");
    header.className = "diff-hunk-header";
    header.textContent = hunk.header || "@@";
    block.appendChild(header);
    for (const line of hunk.lines || []) {
      const row = document.createElement("div");
      row.className = `diff-line diff-${line.type}`;
      const sign = line.type === "add" ? "+" : line.type === "del" ? "−" : " ";
      row.textContent = `${sign} ${line.text}`;
      block.appendChild(row);
    }
    return block;
  }

  function renderChips() {
    if (!contextChips) {
      return;
    }
    contextChips.innerHTML = "";
    const total = selectedAttachments.length + selectedSources.length;
    if (contextEmpty) {
      contextEmpty.hidden = total > 0;
    }
    selectedAttachments.forEach((attachment, index) => {
      const chip = document.createElement("span");
      chip.className = "context-chip";
      const label = document.createElement("span");
      label.className = "context-chip-label";
      const range = attachment.startLine ? `:${attachment.startLine}-${attachment.endLine}` : "";
      label.textContent = `${baseName(attachment.path)}${range}`;
      label.title = `${attachment.path}${range}`;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "context-chip-remove";
      remove.setAttribute("aria-label", `Remove ${attachment.path}`);
      remove.textContent = "×";
      remove.addEventListener("click", () => {
        selectedAttachments.splice(index, 1);
        renderChips();
      });
      chip.append(label, remove);
      contextChips.append(chip);
    });
    selectedSources.forEach((source, index) => {
      const chip = document.createElement("span");
      chip.className = "context-chip context-chip-source";
      const label = document.createElement("span");
      label.className = "context-chip-label";
      label.textContent = source.label;
      label.title = source.label;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "context-chip-remove";
      remove.setAttribute("aria-label", `Remove ${source.label}`);
      remove.textContent = "×";
      remove.addEventListener("click", () => {
        selectedSources.splice(index, 1);
        renderChips();
      });
      chip.append(label, remove);
      contextChips.append(chip);
    });
  }

  function renderResults() {
    if (!contextResults) {
      return;
    }
    contextResults.innerHTML = "";
    if (!pickerResults.length) {
      const li = document.createElement("li");
      li.className = "context-result-empty";
      li.textContent = "No matching files.";
      contextResults.append(li);
      return;
    }
    pickerResults.forEach((result, index) => {
      const li = document.createElement("li");
      li.className = `context-result${index === pickerIndex ? " active" : ""}`;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", index === pickerIndex ? "true" : "false");
      li.textContent = result.path;
      li.addEventListener("click", () => {
        pickerIndex = index;
        attachHighlighted();
      });
      contextResults.append(li);
    });
  }

  function moveHighlight(delta) {
    if (!pickerResults.length) {
      return;
    }
    pickerIndex = (pickerIndex + delta + pickerResults.length) % pickerResults.length;
    renderResults();
  }

  function parseLine(input) {
    if (!input) {
      return undefined;
    }
    const value = parseInt(input.value, 10);
    return Number.isInteger(value) && value > 0 ? value : undefined;
  }

  function attachHighlighted() {
    const result = pickerResults[pickerIndex];
    if (!result || !workspaceRootId) {
      return;
    }
    const startLine = parseLine(contextStart);
    const endLine = parseLine(contextEnd);
    const attachment = { workspaceId: workspaceRootId, path: result.path };
    if (startLine && endLine && endLine >= startLine) {
      attachment.startLine = startLine;
      attachment.endLine = endLine;
    }
    const duplicate = selectedAttachments.some(
      (a) => a.path === attachment.path && a.startLine === attachment.startLine && a.endLine === attachment.endLine,
    );
    if (!duplicate) {
      selectedAttachments.push(attachment);
      renderChips();
    }
    if (contextStart) {
      contextStart.value = "";
    }
    if (contextEnd) {
      contextEnd.value = "";
    }
    closePicker();
  }

  async function runSearch(query) {
    if (!workspaceRootId || !contextResults) {
      return;
    }
    try {
      const url = `/api/workspace/search?id=${encodeURIComponent(workspaceRootId)}&q=${encodeURIComponent(query)}`;
      const res = await globalThis.fetch(url, { headers: workspaceHeaders() });
      if (!res.ok) {
        pickerResults = [];
        pickerIndex = -1;
        renderResults();
        return;
      }
      const data = await res.json().catch(() => ({}));
      pickerResults = Array.isArray(data.results) ? data.results : [];
      pickerIndex = pickerResults.length ? 0 : -1;
      renderResults();
    } catch {
      pickerResults = [];
      pickerIndex = -1;
      renderResults();
    }
  }

  function showRootError(message) {
    if (contextRootError) {
      contextRootError.textContent = message;
      contextRootError.hidden = false;
    }
  }

  function hideRootError() {
    if (contextRootError) {
      contextRootError.hidden = true;
    }
  }

  function setCalculatorStatus(message) {
    if (calculatorStatus) {
      calculatorStatus.textContent = message;
    }
  }

  function updateWorkspaceActions() {
    if (workspaceActions) {
      workspaceActions.hidden = !workspaceRootId;
    }
    if (calculatorPreview && workspaceRootId) {
      calculatorPreview.disabled = false;
    }
  }

  function previewCalculator() {
    const template = globalThis.GuiCalculatorTemplate;
    const html =
      calculatorHtml ||
      (template && typeof template.createCalculatorProposal === "function"
        ? template.createCalculatorProposal("preview").operations[0]?.text || ""
        : "");
    if (html) {
      openArtifact(html, true);
    }
  }

  async function proposeCalculator() {
    const template = globalThis.GuiCalculatorTemplate;
    if (!workspaceRootId || !template || typeof template.createCalculatorProposal !== "function") {
      return;
    }
    const proposal = template.createCalculatorProposal(workspaceRootId);
    const operation = proposal.operations[0];
    if (!operation || typeof operation.text !== "string") {
      return;
    }
    if (calculatorCreate) {
      calculatorCreate.disabled = true;
    }
    setCalculatorStatus("Preparing review…");
    try {
      const res = await globalThis.fetch("/api/workspace/edits/review", {
        method: "POST",
        headers: workspaceHeaders({ "Content-Type": "application/json" }),
        body: JSON.stringify(proposal),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok || !data.review) {
        setCalculatorStatus(data.error || "Could not prepare calculator.");
        return;
      }
      renderEditReview(data.review, workspaceRootId, proposal.operations, () => {
        calculatorHtml = operation.text;
        if (calculatorPreview) {
          calculatorPreview.disabled = false;
        }
        setCalculatorStatus("Calculator created.");
      });
      setCalculatorStatus("Review the proposed calculator in chat.");
      closePicker();
    } catch {
      setCalculatorStatus("Could not prepare calculator.");
    } finally {
      if (calculatorCreate) {
        calculatorCreate.disabled = false;
      }
    }
  }

  // Desktop-only: open the native directory chooser (a single narrow bridge),
  // fill the path field with the selection, and register it. Cancel does nothing.
  async function browseForRoot() {
    const bridge = globalThis.llmupDesktop;
    if (!bridge || typeof bridge.selectWorkspaceDirectory !== "function") {
      return;
    }
    hideRootError();
    let selected;
    try {
      selected = await bridge.selectWorkspaceDirectory();
    } catch {
      return;
    }
    if (typeof selected !== "string" || selected.length === 0) {
      return; // cancelled — grant nothing
    }
    if (contextRootPath) {
      contextRootPath.value = selected;
    }
    await addRoot();
  }

  async function addRoot(create) {
    if (!contextRootPath) {
      return;
    }
    const path = contextRootPath.value.trim();
    if (!path) {
      return;
    }
    hideRootError();
    try {
      const res = await globalThis.fetch(create ? "/api/workspace/root/create" : "/api/workspace/root", {
        method: "POST",
        headers: workspaceHeaders({ "Content-Type": "application/json" }),
        body: JSON.stringify({ path }),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) {
        showRootError(data.error || "Could not add folder.");
        return;
      }
      workspaceRootId = data.root ? data.root.id : null;
      if (contextRoot) {
        contextRoot.hidden = true;
      }
      if (contextBrowse) {
        contextBrowse.hidden = false;
      }
      updateWorkspaceActions();
      if (contextSearch) {
        contextSearch.value = "";
        contextSearch.focus();
      }
      runSearch("");
    } catch {
      showRootError("Could not add folder.");
    }
  }

  function openPicker() {
    if (!contextPicker) {
      return;
    }
    pickerReturnFocus = document.activeElement;
    contextPicker.hidden = false;
    if (contextAdd) {
      contextAdd.setAttribute("aria-expanded", "true");
    }
    const hasRoot = Boolean(workspaceRootId);
    if (contextRoot) {
      contextRoot.hidden = hasRoot;
    }
    if (contextBrowse) {
      contextBrowse.hidden = !hasRoot;
    }
    updateWorkspaceActions();
    if (contextGit) {
      contextGit.hidden = !hasRoot;
    }
    if (contextGitNote) {
      contextGitNote.hidden = true;
    }
    if (hasRoot && contextSearch) {
      contextSearch.value = "";
      contextSearch.focus();
      runSearch("");
    } else if (contextRootPath) {
      contextRootPath.focus();
    }
  }

  function gitReasonText(reason) {
    if (reason === "git-not-found") {
      return "Git is not installed.";
    }
    if (reason === "not-a-repository") {
      return "This folder is not a Git repository.";
    }
    if (reason === "no-changes") {
      return "No changes to attach.";
    }
    return "Git command failed.";
  }

  async function addGit(mode) {
    if (!workspaceRootId) {
      return;
    }
    if (contextGitNote) {
      contextGitNote.hidden = true;
    }
    try {
      const url = `/api/workspace/git?id=${encodeURIComponent(workspaceRootId)}&mode=${mode}`;
      const res = await globalThis.fetch(url, { headers: workspaceHeaders() });
      const data = await res.json().catch(() => ({}));
      const snapshot = data.snapshot;
      if (!res.ok || !snapshot || !snapshot.available) {
        if (contextGitNote) {
          contextGitNote.textContent = gitReasonText(snapshot ? snapshot.reason : "git-failed");
          contextGitNote.hidden = false;
        }
        return;
      }
      const label = `git ${mode}`;
      if (!selectedSources.some((s) => s.kind === "git" && s.mode === mode)) {
        selectedSources.push({ kind: "git", workspaceId: workspaceRootId, mode, label });
        renderChips();
      }
      closePicker();
    } catch {
      if (contextGitNote) {
        contextGitNote.textContent = "Git command failed.";
        contextGitNote.hidden = false;
      }
    }
  }

  function addPastedContext() {
    if (!contextPasteText) {
      return;
    }
    const content = contextPasteText.value;
    if (!content.trim()) {
      return;
    }
    const kind = contextPasteKind && contextPasteKind.value === "diagnostics" ? "diagnostics" : "terminal";
    const label = kind === "terminal" ? "Terminal output" : "Diagnostics";
    selectedSources.push({ kind, label, content });
    renderChips();
    contextPasteText.value = "";
    closePicker();
  }

  function closePicker() {
    if (!contextPicker) {
      return;
    }
    contextPicker.hidden = true;
    if (contextAdd) {
      contextAdd.setAttribute("aria-expanded", "false");
    }
    // Restore focus to the trigger (or wherever it was) when dismissing.
    const restore = pickerReturnFocus && typeof pickerReturnFocus.focus === "function" ? pickerReturnFocus : contextAdd;
    pickerReturnFocus = null;
    if (restore && typeof restore.focus === "function") {
      restore.focus();
    }
  }

  // Keep Tab focus inside the open picker and close it on Escape.
  function trapPickerFocus(event) {
    if (!contextPicker || contextPicker.hidden) {
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closePicker();
      return;
    }
    if (event.key !== "Tab") {
      return;
    }
    const focusable = contextPicker.querySelectorAll(
      'button:not([hidden]):not([disabled]), input:not([hidden]):not([disabled]), textarea:not([hidden]):not([disabled]), select:not([hidden]):not([disabled]), [tabindex]:not([tabindex="-1"])',
    );
    const visible = Array.from(focusable).filter((el) => el.offsetParent !== null || el === document.activeElement);
    if (visible.length === 0) {
      return;
    }
    const first = visible[0];
    const last = visible[visible.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function initWorkspaceContext() {
    if (!contextBar || !workspaceToken) {
      return;
    }
    try {
      const res = await globalThis.fetch("/api/workspace/status", { headers: workspaceHeaders() });
      if (!res.ok) {
        return; // 404 disabled or 403 no token: stay a plain chat box
      }
      const data = await res.json().catch(() => ({}));
      workspaceRootId = typeof data.rootId === "string" ? data.rootId : null;
      contextBar.hidden = false;
      updateWorkspaceActions();
      // In the desktop shell, offer the native directory chooser alongside the
      // manual path entry; browser mode keeps manual entry only.
      if (contextRootBrowse && globalThis.llmupDesktop && typeof globalThis.llmupDesktop.selectWorkspaceDirectory === "function") {
        contextRootBrowse.hidden = false;
      }
      renderChips();
    } catch {
      // Leave context disabled on any failure.
    }
  }

  if (contextAdd) {
    contextAdd.addEventListener("click", () => {
      if (contextPicker && !contextPicker.hidden) {
        closePicker();
      } else {
        openPicker();
      }
    });
  }
  if (contextClose) {
    contextClose.addEventListener("click", () => closePicker());
  }
  if (contextPicker) {
    contextPicker.addEventListener("keydown", (event) => trapPickerFocus(event));
  }
  if (contextGitStatus) {
    contextGitStatus.addEventListener("click", () => addGit("status"));
  }
  if (contextGitDiff) {
    contextGitDiff.addEventListener("click", () => addGit("diff"));
  }
  if (contextPasteAdd) {
    contextPasteAdd.addEventListener("click", () => addPastedContext());
  }
  if (contextRootAdd) {
    contextRootAdd.addEventListener("click", () => addRoot());
  }
  if (contextRootCreate) {
    contextRootCreate.addEventListener("click", () => addRoot(true));
  }
  if (contextRootBrowse) {
    contextRootBrowse.addEventListener("click", () => browseForRoot());
  }
  if (contextRootPath) {
    contextRootPath.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        addRoot();
      }
    });
  }
  if (calculatorCreate) {
    calculatorCreate.addEventListener("click", () => proposeCalculator());
  }
  if (calculatorPreview) {
    calculatorPreview.addEventListener("click", () => previewCalculator());
  }
  if (contextSearch) {
    let searchTimer = null;
    contextSearch.addEventListener("input", () => {
      if (searchTimer) {
        globalThis.clearTimeout(searchTimer);
      }
      searchTimer = globalThis.setTimeout(() => runSearch(contextSearch.value), 180);
    });
    contextSearch.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        moveHighlight(1);
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        moveHighlight(-1);
      } else if (event.key === "Enter") {
        event.preventDefault();
        attachHighlighted();
      } else if (event.key === "Escape") {
        event.preventDefault();
        closePicker();
      }
    });
  }

  // --- Command palette (⌘K / Ctrl+K) -------------------------------------
  const commandPalette = document.querySelector("#command-palette");
  const commandInput = document.querySelector("#command-input");
  const commandList = document.querySelector("#command-list");
  let commandReturnFocus = null;
  let commandFiltered = [];
  let commandIndex = 0;

  function paletteCommands() {
    return [
      { label: "Go to Chat", run: () => switchView("chat") },
      { label: "Go to Models", run: () => switchView("models") },
      { label: "Go to Connectors", run: () => switchView("connectors") },
      { label: "Go to Library (Agents & Skills)", run: () => switchView("library") },
      { label: "Go to Runtime", run: () => switchView("tools") },
      { label: "New chat", run: () => newChat() },
      { label: "Focus message composer", run: () => prompt && prompt.focus() },
      { label: "Add workspace context", run: () => openPicker() },
      { label: "Search chats", run: () => sessionSearch && sessionSearch.focus() },
    ];
  }

  function renderCommands() {
    if (!commandList) {
      return;
    }
    commandList.innerHTML = "";
    commandFiltered.forEach((cmd, i) => {
      const li = document.createElement("li");
      li.className = `command-item${i === commandIndex ? " active" : ""}`;
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", i === commandIndex ? "true" : "false");
      li.textContent = cmd.label;
      li.addEventListener("mousedown", (event) => {
        event.preventDefault();
        runCommand(i);
      });
      commandList.appendChild(li);
    });
  }

  function filterCommands(query) {
    const q = query.trim().toLowerCase();
    commandFiltered = q
      ? paletteCommands().filter((c) => c.label.toLowerCase().includes(q))
      : paletteCommands();
    commandIndex = 0;
    renderCommands();
  }

  function openPalette() {
    if (!commandPalette || !commandInput) {
      return;
    }
    commandReturnFocus = document.activeElement;
    commandPalette.hidden = false;
    commandInput.value = "";
    filterCommands("");
    commandInput.focus();
  }

  function closePalette() {
    if (!commandPalette) {
      return;
    }
    commandPalette.hidden = true;
    if (commandReturnFocus && commandReturnFocus.focus) {
      commandReturnFocus.focus();
    }
  }

  function runCommand(i) {
    const cmd = commandFiltered[i];
    closePalette();
    if (cmd) {
      cmd.run();
    }
  }

  if (commandInput) {
    commandInput.addEventListener("input", () => filterCommands(commandInput.value));
    commandInput.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        commandIndex = Math.min(commandIndex + 1, commandFiltered.length - 1);
        renderCommands();
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        commandIndex = Math.max(commandIndex - 1, 0);
        renderCommands();
      } else if (event.key === "Enter") {
        event.preventDefault();
        runCommand(commandIndex);
      } else if (event.key === "Escape") {
        event.preventDefault();
        closePalette();
      }
    });
  }
  if (commandPalette) {
    commandPalette.addEventListener("mousedown", (event) => {
      if (event.target === commandPalette) {
        closePalette();
      }
    });
  }
  document.addEventListener("keydown", (event) => {
    if ((event.metaKey || event.ctrlKey) && (event.key === "k" || event.key === "K")) {
      event.preventDefault();
      if (commandPalette && commandPalette.hidden) {
        openPalette();
      } else {
        closePalette();
      }
    }
  });

  // --- Drag-and-drop text files onto the composer ------------------------
  const MAX_DROP_BYTES = 64 * 1024;
  function setupComposerDrop() {
    if (!form) {
      return;
    }
    const over = (event) => {
      event.preventDefault();
      form.classList.add("drag-over");
    };
    const leave = () => form.classList.remove("drag-over");
    form.addEventListener("dragover", over);
    form.addEventListener("dragleave", leave);
    form.addEventListener("drop", async (event) => {
      event.preventDefault();
      form.classList.remove("drag-over");
      const files = event.dataTransfer ? Array.from(event.dataTransfer.files) : [];
      for (const file of files) {
        if (selectedSources.length >= 10) {
          break;
        }
        // Text-only: images/binaries need a vision model we don't run locally.
        if (file.type && !file.type.startsWith("text/") && !/\.(txt|md|log|json|ya?ml|csv|ts|js|py|go|rs|java|c|cpp|h|sh)$/i.test(file.name)) {
          continue;
        }
        try {
          const text = await file.text();
          if (!text.trim()) {
            continue;
          }
          const content = text.length > MAX_DROP_BYTES ? text.slice(0, MAX_DROP_BYTES) : text;
          selectedSources.push({ kind: "terminal", label: file.name, content });
          renderChips();
          announce(`Attached ${file.name}.`);
        } catch {
          // Unreadable file; skip silently.
        }
      }
    });
  }
  setupComposerDrop();

  // --- Artifact preview (sandboxed) --------------------------------------
  const artifactModal = document.querySelector("#artifact-modal");
  const artifactFrame = document.querySelector("#artifact-frame");
  const artifactClose = document.querySelector("#artifact-close");
  let artifactReturnFocus = null;

  function openArtifact(html, interactive = false) {
    if (!artifactModal || !artifactFrame) {
      return;
    }
    artifactReturnFocus = document.activeElement;
    // Interactive previews run only the built-in calculator runtime under a
    // same-origin sandbox; model-generated artifacts stay script-free.
    artifactFrame.setAttribute("sandbox", interactive ? "allow-scripts allow-same-origin" : "");
    artifactFrame.srcdoc = html;
    artifactModal.hidden = false;
    if (artifactClose) {
      artifactClose.focus();
    }
  }

  function closeArtifact() {
    if (!artifactModal || !artifactFrame) {
      return;
    }
    artifactModal.hidden = true;
    artifactFrame.srcdoc = "";
    if (artifactReturnFocus && artifactReturnFocus.focus) {
      artifactReturnFocus.focus();
    }
  }

  if (artifactClose) {
    artifactClose.addEventListener("click", closeArtifact);
  }
  if (artifactModal) {
    artifactModal.addEventListener("mousedown", (event) => {
      if (event.target === artifactModal) {
        closeArtifact();
      }
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && !artifactModal.hidden) {
        event.preventDefault();
        closeArtifact();
      }
    });
  }

  // --- Per-chat system prompt --------------------------------------------
  const systemPromptToggle = document.querySelector("#system-prompt-toggle");
  const systemPromptPanel = document.querySelector("#system-prompt-panel");
  const systemPromptInput = document.querySelector("#system-prompt-input");
  const systemPromptClear = document.querySelector("#system-prompt-clear");
  const systemPromptNote = document.querySelector("#system-prompt-note");
  const temperatureInput = document.querySelector("#temperature-input");
  const temperatureValueEl = document.querySelector("#temperature-value");
  const SYSTEM_PROMPT_KEY = "llmup.systemPrompt";
  const TEMPERATURE_KEY = "llmup.temperature";

  function systemPromptValue() {
    return systemPromptInput ? systemPromptInput.value.trim() : "";
  }

  // Undefined until the user moves the slider, so the backend default stands
  // unless temperature is explicitly opted into.
  function temperatureValue() {
    if (!temperatureInput) {
      return undefined;
    }
    let stored = null;
    try {
      stored = globalThis.localStorage.getItem(TEMPERATURE_KEY);
    } catch {
      stored = null;
    }
    if (stored === null) {
      return undefined;
    }
    const num = Number(temperatureInput.value);
    return Number.isFinite(num) ? num : undefined;
  }

  if (temperatureInput) {
    try {
      const stored = globalThis.localStorage.getItem(TEMPERATURE_KEY);
      if (stored !== null) {
        temperatureInput.value = stored;
      }
    } catch {
      // Storage unavailable.
    }
    if (temperatureValueEl) {
      temperatureValueEl.textContent = temperatureInput.value;
    }
    temperatureInput.addEventListener("input", () => {
      if (temperatureValueEl) {
        temperatureValueEl.textContent = temperatureInput.value;
      }
      try {
        globalThis.localStorage.setItem(TEMPERATURE_KEY, temperatureInput.value);
      } catch {
        // Best-effort persistence.
      }
    });
  }

  function refreshSystemPromptState() {
    const active = systemPromptValue().length > 0;
    if (systemPromptToggle) {
      systemPromptToggle.classList.toggle("active", active);
    }
    if (systemPromptNote) {
      systemPromptNote.textContent = active ? "Applied to every turn in this chat." : "";
    }
  }

  if (systemPromptInput) {
    try {
      systemPromptInput.value = globalThis.localStorage.getItem(SYSTEM_PROMPT_KEY) || "";
    } catch {
      // Storage unavailable; start empty.
    }
    systemPromptInput.addEventListener("input", () => {
      try {
        globalThis.localStorage.setItem(SYSTEM_PROMPT_KEY, systemPromptInput.value);
      } catch {
        // Best-effort persistence.
      }
      refreshSystemPromptState();
    });
    refreshSystemPromptState();
  }
  if (systemPromptToggle && systemPromptPanel) {
    systemPromptToggle.addEventListener("click", () => {
      const open = systemPromptPanel.hidden;
      systemPromptPanel.hidden = !open;
      systemPromptToggle.setAttribute("aria-expanded", open ? "true" : "false");
      if (open && systemPromptInput) {
        systemPromptInput.focus();
      }
    });
  }
  if (systemPromptClear && systemPromptInput) {
    systemPromptClear.addEventListener("click", () => {
      systemPromptInput.value = "";
      try {
        globalThis.localStorage.removeItem(SYSTEM_PROMPT_KEY);
        globalThis.localStorage.removeItem(TEMPERATURE_KEY);
      } catch {
        // Best-effort.
      }
      if (temperatureInput) {
        temperatureInput.value = "0.7";
        if (temperatureValueEl) {
          temperatureValueEl.textContent = "0.7";
        }
      }
      refreshSystemPromptState();
      systemPromptInput.focus();
    });
  }

  showEmptyMessages();
  loadStatus();
  loadUpdateStatus();
  loadHarnesses();
  loadRuntimes();
  loadActive();
  loadHardware();
  loadLibrary();
  initSessions();
  initWorkspaceContext();
});