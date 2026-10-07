document.addEventListener("DOMContentLoaded", () => {
  const list = document.querySelector("#generation-models");
  const generationPanel = document.querySelector("#generation-model-panel");
  const textPanel = document.querySelector("#model-catalog-panel");
  const heading = document.querySelector("#generation-model-heading");
  const summary = document.querySelector("#active-models-summary");
  const refresh = document.querySelector("#refresh-generation");
  const tabs = [...document.querySelectorAll("[data-model-kind]")];
  const chatForm = document.querySelector("#chat-form");
  const chatPrompt = document.querySelector("#prompt");
  const messages = document.querySelector("#messages");
  const send = chatForm?.querySelector(".send-btn");
  const modeGroup = document.querySelector("#chat-mode");
  const modes = modeGroup ? [...modeGroup.querySelectorAll(".chat-mode-option")] : [];
  const modeNote = document.querySelector("#chat-mode-note");
  const errorBanner = document.querySelector("#generation-error");
  const a11y = document.querySelector("#a11y-status");
  if (!list || !chatForm || !chatPrompt || !messages) return;

  const token = document.querySelector('meta[name="llmup-token"]')?.getAttribute("content") || "";
  const selectionKeys = {
    image: "rigspark.active.image",
    video: "rigspark.active.video",
  };
  const placeholders = {
    text: "Message the local model…",
    image: "Describe an image to generate…",
    video: "Describe a video to generate…",
  };
  let models = [];
  let modelKind = "text";
  let chatMode = "text";
  let activeText = "not running";
  let chatJob = null;
  let comfyuiDir = "";
  let comfyuiPort = 8188;
  const activeSelections = { image: null, video: null };

  function stored(key) {
    try {
      return globalThis.localStorage.getItem(key);
    } catch {
      return null;
    }
  }

  function store(key, value) {
    try {
      globalThis.localStorage.setItem(key, value);
    } catch {
      // The current selection still works when browser storage is unavailable.
    }
  }

  function selectedModel(kind) {
    const selected = activeSelections[kind] || stored(selectionKeys[kind]);
    const runnable = models.filter((model) => model.kind === kind && model.runnable);
    return runnable.find((model) => model.id === selected)
      || runnable.find((model) => model.default)
      || runnable[0];
  }

  function showError(message) {
    errorBanner.hidden = !message;
    errorBanner.textContent = message || "";
  }

  function announce(message) {
    if (!a11y) return;
    a11y.textContent = "";
    globalThis.setTimeout(() => {
      a11y.textContent = message;
    }, 30);
  }

  function gib(bytes) {
    return `${(bytes / 1073741824).toFixed(1)} GiB`;
  }

  function updateSummary() {
    if (!summary) return;
    const image = selectedModel("image")?.id || "unavailable";
    const video = selectedModel("video")?.id || "unavailable";
    summary.textContent = `Text: ${activeText} · Image: ${image} · Video: ${video}`;
  }

  async function loadActiveText() {
    try {
      const response = await globalThis.fetch("/api/models/active");
      if (!response.ok) throw new Error(`request failed (${response.status})`);
      const data = await response.json();
      activeText = data.active?.modelId || "not running";
    } catch {
      activeText = "status unavailable";
    }
    updateSummary();
  }

  function selectModel(model) {
    activeSelections[model.kind] = model.id;
    store(selectionKeys[model.kind], model.id);
    renderModels();
    updateSummary();
    if (chatMode === model.kind) setChatMode(chatMode);
    announce(`${model.id} selected for ${model.kind}`);
  }

  function renderModels() {
    list.textContent = "";
    const visibleModels = models.filter((model) => model.kind === modelKind);
    const selected = selectedModel(modelKind);
    if (!visibleModels.length) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = `No ${modelKind} models are available.`;
      list.appendChild(empty);
      return;
    }
    for (const model of visibleModels) {
      const card = document.createElement("article");
      card.className = "model-card-item generation-card";
      const head = document.createElement("div");
      head.className = "model-card-head";
      const title = document.createElement("div");
      title.className = "model-card-title";
      title.textContent = model.id;
      const badge = document.createElement("span");
      badge.className = `verdict-badge verdict-${model.fit.verdict}`;
      badge.textContent = `fit ${model.fit.verdict}`;
      head.append(title, badge);
      if (!model.runnable) {
        const pending = document.createElement("span");
        pending.className = "verdict-badge verdict-slow";
        pending.textContent = "workflow coming";
        head.appendChild(pending);
      }
      const meta = document.createElement("div");
      meta.className = "model-card-meta";
      const origin = model.provenance === "auto" ? " · auto-sourced" : "";
      meta.textContent = `${model.params} params · ${gib(model.weightsBytes)} weights · ${model.license} · ${model.recency}${origin} · speed unknown`;
      const reason = document.createElement("div");
      reason.className = "model-card-meta";
      reason.textContent = model.fit.reason;
      const action = document.createElement("button");
      action.type = "button";
      action.className = selected?.id === model.id ? "accent-btn" : "ghost-btn";
      action.textContent = !model.runnable
        ? "Not runnable yet"
        : selected?.id === model.id
          ? "Active"
          : "Use in Chat";
      action.disabled = !model.runnable || selected?.id === model.id;
      action.addEventListener("click", () => selectModel(model));
      card.append(head, meta, reason, action);
      list.appendChild(card);
    }
  }

  async function loadModels() {
    showError("");
    try {
      const response = await globalThis.fetch("/api/generation/models");
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
      models = data.models;
      comfyuiDir = data.defaults.comfyuiDir || "";
      comfyuiPort = data.defaults.port || 8188;
      for (const kind of ["image", "video"]) {
        const model = selectedModel(kind);
        if (model) {
          activeSelections[kind] = model.id;
          store(selectionKeys[kind], model.id);
        }
      }
      renderModels();
      updateSummary();
      setChatMode(chatMode);
    } catch (error) {
      list.textContent = "";
      showError(`Could not load generation models: ${error.message}`);
    }
  }

  function setModelKind(kind, focus = false) {
    modelKind = kind;
    const text = kind === "text";
    textPanel.hidden = !text;
    generationPanel.hidden = text;
    for (const tab of tabs) {
      const selected = tab.dataset.modelKind === kind;
      tab.setAttribute("aria-selected", String(selected));
      tab.tabIndex = selected ? 0 : -1;
      if (selected && focus) tab.focus();
    }
    if (!text) {
      heading.textContent = `${kind === "image" ? "Image" : "Video"} models`;
      renderModels();
    }
    loadActiveText();
  }

  for (const tab of tabs) {
    tab.addEventListener("click", () => setModelKind(tab.dataset.modelKind));
    tab.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight"].includes(event.key)) return;
      event.preventDefault();
      const index = tabs.indexOf(tab);
      const step = event.key === "ArrowRight" ? 1 : -1;
      setModelKind(tabs[(index + step + tabs.length) % tabs.length].dataset.modelKind, true);
    });
  }

  function setChatMode(mode) {
    chatMode = mode;
    for (const option of modes) {
      const selected = option.dataset.mode === mode;
      option.setAttribute("aria-checked", String(selected));
      option.tabIndex = selected ? 0 : -1;
    }
    chatPrompt.placeholder = placeholders[mode];
    chatPrompt.setAttribute("aria-label", mode === "text" ? "Message the local model" : placeholders[mode].replace("…", ""));
    const model = mode === "text" ? null : selectedModel(mode);
    modeNote.hidden = mode === "text";
    modeNote.textContent = model
      ? `${model.id} via local ComfyUI · fit ${model.fit.verdict} · speed unknown`
      : `No ${mode} model available`;
    if (!chatJob) {
      send.textContent = mode === "text" ? "Send" : "Generate";
      send.setAttribute("aria-label", send.textContent);
    }
  }

  for (const option of modes) {
    option.addEventListener("click", () => setChatMode(option.dataset.mode));
    option.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight"].includes(event.key)) return;
      event.preventDefault();
      const index = modes.indexOf(option);
      const step = event.key === "ArrowRight" ? 1 : -1;
      const next = modes[(index + step + modes.length) % modes.length];
      setChatMode(next.dataset.mode);
      next.focus();
    });
  }

  function chatRow(role, label) {
    messages.querySelector(".messages-empty")?.remove();
    const row = document.createElement("div");
    row.className = `message ${role}`;
    const avatar = document.createElement("img");
    avatar.className = "message-avatar";
    avatar.src = role === "assistant" ? "/static/mascot-avatar.jpg" : "/static/mascot-welcome.jpg";
    avatar.alt = "";
    const content = document.createElement("div");
    content.className = "message-content";
    const roleLabel = document.createElement("div");
    roleLabel.className = "message-role";
    roleLabel.textContent = label;
    const body = document.createElement("div");
    body.className = "message-body";
    content.append(roleLabel, body);
    row.append(avatar, content);
    messages.appendChild(row);
    row.scrollIntoView({ block: "end" });
    return body;
  }

  function setBusy(busy) {
    for (const option of modes) option.disabled = busy;
    send.classList.toggle("is-stop", busy);
    send.textContent = busy ? "Stop" : chatMode === "text" ? "Send" : "Generate";
    send.setAttribute("aria-label", busy ? "Stop generation" : send.textContent);
  }

  async function pollJob(context) {
    while (chatJob === context) {
      await new Promise((resolve) => globalThis.setTimeout(resolve, 1000));
      const response = await globalThis.fetch("/api/generation/jobs/current");
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
      const job = data.job;
      if (!job || job.id !== context.id) throw new Error("Generation was replaced by another job");
      if (job.status === "running") {
        context.status.textContent = job.events[job.events.length - 1] || `Generating ${context.kind}…`;
        continue;
      }
      if (job.status !== "succeeded") throw new Error(job.error || `Generation ${job.status}`);
      context.status.textContent = `Generated ${context.kind}`;
      const media = document.createElement("img");
      media.className = "generated-media";
      media.src = job.outputUrl;
      media.alt = `Generated ${context.kind} for: ${context.prompt}`;
      const download = document.createElement("a");
      download.className = "accent-btn generation-download";
      download.href = job.outputUrl;
      download.download = job.result?.result?.path?.split(/[\\/]/).pop() || `rigspark-${context.kind}`;
      download.textContent = "Download";
      context.body.append(media, download);
      announce(`${context.kind} ready`);
      break;
    }
  }

  async function generate() {
    const text = chatPrompt.value.trim();
    if (!text) return;
    const model = selectedModel(chatMode);
    if (!model) throw new Error(`No local ${chatMode} model is available`);
    if (!comfyuiDir) {
      throw new Error("ComfyUI was not detected. Set RIGSPARK_COMFYUI_DIR before starting rigspark");
    }
    if (!model.runnable) {
      throw new Error(`${model.id} has no built-in ComfyUI workflow yet (workflow coming)`);
    }
    if (model.fit.verdict === "no") {
      throw new Error(`${model.id} does not fit this machine (${model.fit.reason})`);
    }
    chatRow("user", "You").textContent = text;
    chatPrompt.value = "";
    const body = chatRow("assistant", `${chatMode === "video" ? "Video" : "Image"} · ${model.id}`);
    const status = document.createElement("p");
    status.className = "generation-chat-status";
    status.textContent = `Starting ${model.id}…`;
    body.appendChild(status);
    setBusy(true);
    const response = await globalThis.fetch("/api/generation/jobs", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-LLMUP-Token": token },
      body: JSON.stringify({
        model: model.id,
        prompt: text,
        comfyuiDir,
        port: comfyuiPort,
        bypass: false,
      }),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
    const context = { id: data.job.id, kind: chatMode, prompt: text, body, status };
    chatJob = context;
    await pollJob(context);
  }

  chatForm.addEventListener("submit", async (event) => {
    if (chatMode === "text") return;
    event.preventDefault();
    event.stopImmediatePropagation();
    showError("");
    if (chatJob) {
      await globalThis.fetch("/api/generation/jobs/current/cancel", {
        method: "POST",
        headers: { "X-LLMUP-Token": token },
      });
      chatJob = null;
      setBusy(false);
      announce("Generation cancelled");
      return;
    }
    try {
      await generate();
    } catch (error) {
      showError(`Could not generate ${chatMode}: ${error.message}`);
    } finally {
      chatJob = null;
      setBusy(false);
    }
  }, true);

  refresh?.addEventListener("click", loadModels);
  globalThis.addEventListener("rigspark:text-model-active", (event) => {
    activeText = event.detail?.modelId || "not running";
    updateSummary();
  });
  loadModels();
  loadActiveText();
});
