// Create view: local image/video generation through the loopback ComfyUI integration.
document.addEventListener("DOMContentLoaded", () => {
  const list = document.querySelector("#generation-models");
  const form = document.querySelector("#generation-form");
  const prompt = document.querySelector("#generation-prompt");
  const directory = document.querySelector("#generation-dir");
  const seed = document.querySelector("#generation-seed");
  const port = document.querySelector("#generation-port");
  const bypass = document.querySelector("#generation-bypass");
  const start = document.querySelector("#generation-start");
  const cancel = document.querySelector("#generation-cancel");
  const status = document.querySelector("#generation-status");
  const log = document.querySelector("#generation-log");
  const errorBanner = document.querySelector("#generation-error");
  const result = document.querySelector("#generation-result");
  const preview = document.querySelector("#generation-preview");
  const caption = document.querySelector("#generation-caption");
  const refresh = document.querySelector("#refresh-generation");
  if (!list || !form || !prompt || !directory) {
    return;
  }
  const token = document.querySelector('meta[name="llmup-token"]')?.getAttribute("content") || "";
  let models = [];
  let selected = null;
  let polling = null;
  let shownOutput = null;

  function showError(message) {
    errorBanner.hidden = !message;
    errorBanner.textContent = message || "";
  }

  function gib(bytes) {
    return `${(bytes / 1073741824).toFixed(1)} GiB`;
  }

  function select(id) {
    selected = id;
    for (const card of list.querySelectorAll(".generation-card")) {
      const checked = card.dataset.id === id;
      card.setAttribute("aria-checked", String(checked));
      card.tabIndex = checked ? 0 : -1;
    }
  }

  function renderModels() {
    list.textContent = "";
    for (const model of models) {
      const card = document.createElement("button");
      card.type = "button";
      card.className = "model-card-item generation-card";
      card.setAttribute("role", "radio");
      card.dataset.id = model.id;
      const head = document.createElement("div");
      head.className = "model-card-head";
      const title = document.createElement("div");
      title.className = "model-card-title";
      title.textContent = model.id;
      const badge = document.createElement("span");
      badge.className = `verdict-badge verdict-${model.fit.verdict}`;
      badge.textContent = `fit ${model.fit.verdict}`;
      head.append(title, badge);
      const meta = document.createElement("div");
      meta.className = "model-card-meta";
      meta.textContent = `${model.kind} · ${model.params} params · ${gib(model.weightsBytes)} weights · ${model.license} · speed unknown`;
      const reason = document.createElement("div");
      reason.className = "model-card-meta";
      reason.textContent = model.fit.reason;
      card.append(head, meta, reason);
      card.addEventListener("click", () => select(model.id));
      card.addEventListener("keydown", (event) => {
        if (!["ArrowDown", "ArrowRight", "ArrowUp", "ArrowLeft"].includes(event.key)) return;
        event.preventDefault();
        const index = models.findIndex((entry) => entry.id === selected);
        const step = event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : -1;
        const next = models[(index + step + models.length) % models.length];
        select(next.id);
        list.querySelector(`[data-id="${CSS.escape(next.id)}"]`)?.focus();
      });
      list.appendChild(card);
    }
    if (!selected || !models.some((model) => model.id === selected)) {
      selected = (models.find((model) => model.kind === "image" && model.default) || models[0])?.id;
    }
    select(selected);
  }

  async function loadModels() {
    showError("");
    try {
      const response = await globalThis.fetch("/api/generation/models");
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
      models = data.models;
      if (!directory.value && data.defaults.comfyuiDir) directory.value = data.defaults.comfyuiDir;
      if (data.defaults.port && !port.value) port.value = String(data.defaults.port);
      renderModels();
    } catch (error) {
      list.textContent = "";
      showError(`Could not load generation models: ${error.message}`);
    }
  }

  function renderJob(job) {
    const running = Boolean(job && job.status === "running");
    start.disabled = running;
    cancel.hidden = !running;
    if (!job) {
      status.textContent = "Idle";
      return;
    }
    const labels = { running: "Generating…", succeeded: "Done", failed: "Failed", cancelled: "Cancelled" };
    status.textContent = `${labels[job.status] || job.status} · ${job.model}`;
    log.textContent = "";
    for (const line of job.events.slice(-50)) {
      const item = document.createElement("li");
      item.textContent = line;
      log.appendChild(item);
    }
    log.scrollTop = log.scrollHeight;
    if (job.status === "failed") showError(job.error || "Generation failed");
    if (job.status === "succeeded" && job.outputUrl && shownOutput !== job.outputUrl) {
      shownOutput = job.outputUrl;
      preview.src = job.outputUrl;
      preview.alt = `Generated ${job.kind} for: ${prompt.value}`;
      const outcome = job.result && job.result.result;
      caption.textContent = outcome
        ? `${outcome.path} · seed ${outcome.seed} · ${outcome.bytes} bytes`
        : job.outputUrl;
      result.hidden = false;
    }
  }

  async function poll() {
    try {
      const response = await globalThis.fetch("/api/generation/jobs/current");
      const data = await response.json();
      renderJob(data.job);
      if (!data.job || data.job.status !== "running") {
        globalThis.clearInterval(polling);
        polling = null;
      }
    } catch (error) {
      showError(`Lost contact with rigspark: ${error.message}`);
    }
  }

  function startPolling() {
    if (!polling) polling = globalThis.setInterval(poll, 1000);
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    showError("");
    if (!form.reportValidity()) return;
    const model = models.find((entry) => entry.id === selected);
    if (!model) {
      showError("Select a generation model");
      return;
    }
    if (model.fit.verdict === "no" && !bypass.checked) {
      showError(`${model.id} does not fit this machine (${model.fit.reason}). Tick “Bypass memory fit” to try anyway.`);
      return;
    }
    const payload = {
      model: model.id,
      prompt: prompt.value,
      comfyuiDir: directory.value.trim(),
      port: Number(port.value),
      bypass: bypass.checked,
    };
    if (seed.value !== "") payload.seed = Number(seed.value);
    start.disabled = true;
    result.hidden = true;
    shownOutput = null;
    try {
      const response = await globalThis.fetch("/api/generation/jobs", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-LLMUP-Token": token },
        body: JSON.stringify(payload),
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
      renderJob(data.job);
      startPolling();
    } catch (error) {
      start.disabled = false;
      showError(`Could not start generation: ${error.message}`);
    }
  });

  cancel.addEventListener("click", async () => {
    cancel.disabled = true;
    try {
      await globalThis.fetch("/api/generation/jobs/current/cancel", {
        method: "POST",
        headers: { "X-LLMUP-Token": token },
      });
      await poll();
    } finally {
      cancel.disabled = false;
    }
  });


  // --- Chat integration: reply with a generated image or video instead of text. ---
  const chatForm = document.querySelector("#chat-form");
  const chatPrompt = document.querySelector("#prompt");
  const messages = document.querySelector("#messages");
  const sendBtn = chatForm?.querySelector(".send-btn");
  const modeGroup = document.querySelector("#chat-mode");
  const modeNote = document.querySelector("#chat-mode-note");
  const a11y = document.querySelector("#a11y-status");
  const modeOptions = modeGroup ? [...modeGroup.querySelectorAll(".chat-mode-option")] : [];
  const placeholders = {
    text: "Message the local model…",
    image: "Describe an image to generate…",
    video: "Describe a video to generate…",
  };
  let chatMode = "text";
  let chatJob = null;

  function announce(text) {
    if (!a11y) return;
    a11y.textContent = "";
    globalThis.setTimeout(() => {
      a11y.textContent = text;
    }, 30);
  }

  function defaultModel(kind) {
    return models.find((model) => model.kind === kind && model.default) || models.find((model) => model.kind === kind);
  }

  function setChatMode(mode) {
    chatMode = mode;
    for (const option of modeOptions) {
      const checked = option.dataset.mode === mode;
      option.setAttribute("aria-checked", String(checked));
      option.tabIndex = checked ? 0 : -1;
    }
    if (chatPrompt) {
      chatPrompt.placeholder = placeholders[mode];
      chatPrompt.setAttribute("aria-label", mode === "text" ? "Message the local model" : placeholders[mode].replace("…", ""));
    }
    if (sendBtn && !chatJob && !sendBtn.classList.contains("is-stop")) {
      sendBtn.textContent = mode === "text" ? "Send" : "Generate";
    }
    const model = mode === "text" ? null : defaultModel(mode);
    modeNote.hidden = mode === "text";
    modeNote.textContent = model
      ? `${model.id} via local ComfyUI · fit ${model.fit.verdict} · speed unknown`
      : "Local ComfyUI generation";
  }

  function setBusy(busy) {
    for (const option of modeOptions) option.disabled = busy;
    if (!sendBtn) return;
    sendBtn.classList.toggle("is-stop", busy);
    sendBtn.textContent = busy ? "Stop" : chatMode === "text" ? "Send" : "Generate";
    sendBtn.setAttribute("aria-label", busy ? "Stop generation" : "Send");
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

  function generationReply(kind, modelId) {
    const body = chatRow("assistant", `${kind === "video" ? "Video" : "Image"} · ${modelId || "local"}`);
    const status = document.createElement("p");
    status.className = "generation-chat-status";
    const progress = document.createElement("p");
    progress.className = "generation-chat-progress";
    body.append(status, progress);
    return {
      update(text, detail) {
        status.textContent = text;
        progress.textContent = detail || "";
      },
      fail(text) {
        status.textContent = text;
        progress.textContent = "";
        body.closest(".message")?.classList.add("error");
        announce(text);
      },
      media(url, prompt, outcome) {
        status.textContent = kind === "video" ? "Generated video" : "Generated image";
        progress.remove();
        const media = document.createElement("img");
        media.className = "generated-media";
        media.src = url;
        media.alt = `Generated ${kind} for: ${prompt}`;
        const caption = document.createElement("div");
        caption.className = "generation-chat-caption";
        const meta = document.createElement("span");
        meta.textContent = outcome ? `seed ${outcome.seed} · ${outcome.path}` : "";
        const download = document.createElement("a");
        download.href = url;
        download.download = outcome ? outcome.path.split(/[\\/]/).pop() : `rigspark-${kind}`;
        download.textContent = "Download";
        caption.append(meta, download);
        body.append(media, caption);
        media.addEventListener("load", () => media.closest(".message")?.scrollIntoView({ block: "end" }));
        announce(`${kind === "video" ? "Video" : "Image"} ready`);
      },
    };
  }

  async function pollChatJob() {
    while (chatJob) {
      await new Promise((resolve) => globalThis.setTimeout(resolve, 1000));
      const current = chatJob;
      if (!current) return;
      let job;
      try {
        const response = await globalThis.fetch("/api/generation/jobs/current");
        job = (await response.json()).job;
      } catch (error) {
        current.reply.update("Waiting for rigspark…", error.message);
        continue;
      }
      if (!job || job.id !== current.id) {
        current.reply.fail("Generation was replaced by another job");
      } else if (job.status === "running") {
        const seconds = Math.round((Date.now() - current.started) / 1000);
        current.reply.update(`Generating ${current.kind}… ${seconds}s`, job.events[job.events.length - 1]);
        continue;
      } else if (job.status === "succeeded") {
        current.reply.media(job.outputUrl, current.prompt, job.result && job.result.result);
      } else if (job.status === "cancelled") {
        current.reply.fail("Generation cancelled");
      } else {
        current.reply.fail(`Generation failed: ${job.error || "unknown error"}`);
      }
      chatJob = null;
      setBusy(false);
      renderJob(job);
    }
  }

  async function generateFromChat(kind, text) {
    chatRow("user", "You").textContent = text;
    if (!models.length) await loadModels();
    const model = defaultModel(kind);
    const reply = generationReply(kind, model?.id);
    if (!model) {
      reply.fail(`No local ${kind} model is available.`);
      return;
    }
    const dir = (directory.value || "").trim();
    if (!dir) {
      reply.fail("Set your ComfyUI directory in the Create view first.");
      return;
    }
    if (model.fit.verdict === "no" && !bypass.checked) {
      reply.fail(`${model.id} does not fit this machine (${model.fit.reason}). Enable “Bypass memory fit” in Create to try anyway.`);
      return;
    }
    reply.update(`Starting ${model.id}…`);
    setBusy(true);
    chatJob = { id: null, kind, prompt: text, reply, started: Date.now() };
    try {
      const response = await globalThis.fetch("/api/generation/jobs", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-LLMUP-Token": token },
        body: JSON.stringify({
          model: model.id,
          prompt: text,
          comfyuiDir: dir,
          port: Number(port.value) || 8188,
          bypass: bypass.checked,
        }),
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
      chatJob.id = data.job.id;
      result.hidden = true;
      announce(`Generating ${kind}`);
      await pollChatJob();
    } catch (error) {
      reply.fail(`Could not start generation: ${error.message}`);
      chatJob = null;
      setBusy(false);
    }
  }

  if (chatForm && chatPrompt && messages && sendBtn && modeGroup) {
    // Capture at the document so this runs before the chat submit handler in every engine.
    document.addEventListener(
      "submit",
      (event) => {
        if (event.target !== chatForm) return;
        if (chatJob) {
          event.preventDefault();
          event.stopImmediatePropagation();
          if (chatJob.id) {
            globalThis.fetch("/api/generation/jobs/current/cancel", {
              method: "POST",
              headers: { "X-LLMUP-Token": token },
            });
            chatJob.reply.update("Cancelling…");
          }
          return;
        }
        if (chatMode === "text" || sendBtn.classList.contains("is-stop")) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        const text = chatPrompt.value.trim();
        if (!text) return;
        chatPrompt.value = "";
        chatPrompt.dispatchEvent(new Event("input"));
        generateFromChat(chatMode, text);
      },
      true,
    );
    for (const option of modeOptions) {
      option.addEventListener("click", () => {
        setChatMode(option.dataset.mode);
        chatPrompt.focus();
        if (option.dataset.mode !== "text" && !models.length) {
          loadModels().then(() => setChatMode(chatMode));
        }
      });
    }
    modeGroup.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
      event.preventDefault();
      const index = modeOptions.findIndex((option) => option.dataset.mode === chatMode);
      const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : -1;
      const next = modeOptions[(index + step + modeOptions.length) % modeOptions.length];
      if (next.disabled) return;
      next.click();
      next.focus();
    });
  }

  refresh?.addEventListener("click", loadModels);
  for (const item of document.querySelectorAll('.rail-item[data-view="create"]')) {
    item.addEventListener("click", async () => {
      await loadModels();
      await poll();
      if (!start.disabled) return;
      startPolling();
    });
  }
});
