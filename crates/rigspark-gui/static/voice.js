// Voice: the Models › Voice catalog, spoken replies, dictation, Voice mode in the
// composer, and the voice parts of the Runtime view. Audio stays on 127.0.0.1.
document.addEventListener("DOMContentLoaded", () => {
  const $ = (selector) => document.querySelector(selector);
  const token = document.querySelector('meta[name="llmup-token"]')?.getAttribute("content") || "";
  const panel = $("#voice-model-panel");
  const list = $("#voice-models");
  const banner = $("#voice-running-banner");
  const errorBanner = $("#voice-error");
  const search = $("#voice-search");
  const filterEmpty = $("#voice-filter-empty");
  const filters = [...document.querySelectorAll("[data-voice-filter]")];
  const voiceTab = document.querySelector('[data-model-kind="voice"]');
  const chatForm = $("#chat-form");
  const prompt = $("#prompt");
  const messages = $("#messages");
  const modeNote = $("#chat-mode-note");
  const compose = $("#voice-compose");
  const sources = [...document.querySelectorAll("[data-voice-source]")];
  const voiceSelect = $("#voice-select");
  const speedSelect = $("#voice-speed");
  const speakToggle = $("#speak-replies");
  const speakLabel = $("#speak-replies-label");
  const dictate = $("#dictate");
  const engines = $("#voice-engines");
  const defaultSpeak = $("#voice-default-speak");
  const microphone = $("#voice-microphone");
  const output = $("#voice-output");
  const outputRow = $("#voice-output-row");
  const attachForm = $("#voice-attach-form");
  const attachStatus = $("#voice-attach-status");
  const runtimeView = $("#view-tools");
  const a11y = $("#a11y-status");
  if (!list || !chatForm || !prompt || !messages) return;

  let catalog = { models: [], engines: [], status: [] };
  let filter = "all";
  let chatMode = "text";
  let source = "script";
  let pendingScript = false;
  let pollTimer = null;
  let memoryTimer = null;
  let activeText = null;

  // --- small helpers ---------------------------------------------------------
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
      // Settings still apply for this session when storage is unavailable.
    }
  }
  function announce(message) {
    if (!a11y) return;
    a11y.textContent = "";
    globalThis.setTimeout(() => {
      a11y.textContent = message;
    }, 30);
  }
  function showError(message) {
    if (!errorBanner) return;
    errorBanner.hidden = !message;
    errorBanner.textContent = message || "";
  }
  function size(bytes, basis) {
    const value = bytes >= 1073741824 ? `${(bytes / 1073741824).toFixed(1)} GiB` : `${Math.round(bytes / 1048576)} MiB`;
    return basis === "pinned" ? value : basis === "ceiling" ? `≤ ${value}` : `≈ ${value}`;
  }
  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }
  async function api(path, options = {}) {
    const headers = { ...(options.headers || {}) };
    if (options.method && options.method !== "GET") headers["X-LLMUP-Token"] = token;
    if (options.json !== undefined) headers["Content-Type"] = "application/json";
    const response = await globalThis.fetch(path, {
      method: options.method || "GET",
      headers,
      body: options.json !== undefined ? JSON.stringify(options.json) : options.body,
    });
    if (!response.ok) {
      const data = await response.json().catch(() => ({}));
      throw new Error(data.error || `request failed (${response.status})`);
    }
    return response;
  }
  function slot(kind) {
    return catalog.status.find((entry) => entry.kind === kind) || null;
  }
  function running(kind) {
    const entry = slot(kind);
    return entry && entry.state === "running" ? entry : null;
  }
  function modelById(id) {
    return catalog.models.find((model) => model.id === id) || null;
  }
  function engine(name) {
    return catalog.engines.find((entry) => entry.engine === name) || null;
  }

  // --- catalog and status ----------------------------------------------------
  async function load() {
    showError("");
    try {
      const response = await api("/api/voice/models");
      catalog = await response.json();
      render();
    } catch (error) {
      list.textContent = "";
      showError(`Could not load voice models: ${error.message}`);
    }
  }

  async function refreshStatus() {
    try {
      const response = await api("/api/voice/status");
      const data = await response.json();
      catalog.status = data.status;
      catalog.engines = data.engines;
      render();
    } catch {
      // The next poll or reload retries.
    }
  }

  function schedulePoll() {
    const busy = catalog.status.some((entry) => entry.state === "starting");
    if (busy && !pollTimer) {
      pollTimer = globalThis.setInterval(refreshStatus, 1500);
    } else if (!busy && pollTimer) {
      globalThis.clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  function render() {
    renderModels();
    renderBanner();
    renderComposer();
    renderEngines();
    renderMemory();
    schedulePoll();
  }

  // --- Models › Voice --------------------------------------------------------
  function setPanelVisible() {
    if (!panel || !voiceTab) return;
    const visible = voiceTab.getAttribute("aria-selected") === "true";
    panel.hidden = !visible;
    if (visible) load();
  }

  async function start(model) {
    showError("");
    try {
      const response = await api("/api/voice/start", { method: "POST", json: { model: model.id } });
      const data = await response.json();
      catalog.status = data.status;
      render();
      announce(`Starting ${model.id}`);
    } catch (error) {
      showError(`Could not start ${model.id}: ${error.message}`);
    }
  }

  async function stop(kind) {
    try {
      const response = await api("/api/voice/stop", { method: "POST", json: { kind } });
      const data = await response.json();
      catalog.status = data.status;
      render();
      announce(`Stopped the ${kind} model`);
    } catch (error) {
      showError(`Could not stop: ${error.message}`);
    }
  }

  function actionFor(model) {
    const current = slot(model.kind);
    const mine = current && current.model === model.id;
    const wrap = element("div", "voice-action");
    if (mine && current.state === "running") {
      const badge = element("span", "voice-in-use", "In use");
      const halt = element("button", "chip-btn", "Stop");
      halt.type = "button";
      halt.addEventListener("click", () => stop(model.kind));
      wrap.append(badge, halt);
      return wrap;
    }
    if (mine && current.state === "starting") {
      const [done, total] = current.progress || [];
      const percent = total ? ` ${Math.floor((done / total) * 100)}%` : "";
      wrap.append(element("span", "voice-progress", `${current.detail}${percent}…`));
      const cancel = element("button", "chip-btn", "Cancel");
      cancel.type = "button";
      cancel.addEventListener("click", () => stop(model.kind));
      wrap.append(cancel);
      return wrap;
    }
    const probe = engine(model.engine);
    const button = element("button", "ghost-btn", mine && current.state === "failed" ? "Retry" : "Start");
    button.type = "button";
    if (model.fit.verdict === "no") {
      button.disabled = true;
      button.title = model.fit.reason;
    } else if (probe && !probe.installed) {
      button.disabled = true;
      button.title = `Install ${probe.label}: ${probe.install}`;
    }
    button.addEventListener("click", () => start(model));
    wrap.append(button);
    return wrap;
  }

  function row(model) {
    const item = element("article", "model-card-item voice-row");
    item.dataset.kind = model.kind;
    const face = globalThis.RigSparkUI
      ? globalThis.RigSparkUI.faceImage(model.fit.verdict === "no" ? "no" : "yes", "model-face")
      : element("span");
    const head = element("div", "model-card-head");
    head.append(element("div", "model-card-title", model.id));
    const languages = model.languages.includes("multilingual") ? "multilingual" : model.languages.join(", ");
    head.append(element("div", "model-card-meta", `${model.params} · ${size(model.weightsBytes, model.sizeBasis)} · ${model.license} · ${languages}`));
    const fitCell = element("div", "model-card-fit");
    const verdict = element("span", `verdict-badge verdict-${model.fit.verdict === "no" ? "no" : "yes"}`, model.fit.verdict === "no" ? "Won't run here" : "Fits");
    fitCell.append(verdict, element("span", "voice-fit-reason", model.fit.reason));
    const detail = element("div", "voice-detail");
    const probe = engine(model.engine);
    detail.append(element("span", "voice-engine", probe ? probe.label : model.engine));
    const failed = slot(model.kind);
    const note = failed && failed.model === model.id && failed.state === "failed"
      ? failed.detail
      : probe && !probe.installed
        ? `Not installed · ${probe.install}`
        : model.voices.length
          ? `${model.voices.length} voices · speed unknown`
          : "speed unknown";
    detail.append(element("span", failed && failed.model === model.id && failed.state === "failed" ? "voice-note voice-note-error" : "voice-note", note));
    item.append(face, head, fitCell, detail, actionFor(model));
    item.title = model.integrity;
    return item;
  }

  function renderModels() {
    list.textContent = "";
    const needle = (search?.value || "").trim().toLowerCase();
    const groups = [
      ["speak", "Speak · reads replies aloud and makes audio"],
      ["listen", "Listen · dictation into the composer"],
    ];
    let shown = 0;
    for (const [kind, title] of groups) {
      if (filter !== "all" && filter !== kind) continue;
      const models = catalog.models.filter((model) => model.kind === kind
        && (!needle || model.id.includes(needle) || model.license.includes(needle)));
      if (!models.length) continue;
      list.append(element("div", "voice-group-head", title));
      for (const model of models) list.append(row(model));
      shown += models.length;
    }
    if (filterEmpty) filterEmpty.hidden = shown > 0 || !catalog.models.length;
  }

  function renderBanner() {
    if (!banner) return;
    const active = catalog.status.filter((entry) => entry.state === "running");
    banner.hidden = !active.length;
    banner.textContent = "";
    if (!active.length) return;
    const text = element("div", "running-banner-text");
    for (const entry of active) {
      const line = element("div");
      const name = element("b", "", entry.model);
      line.append(name, document.createTextNode(` is ready to ${entry.kind} `));
      line.append(element("span", "running-banner-detail", `· ${entry.owned ? entry.engine : "attached"} · ${entry.endpoint.replace(/^https?:\/\//, "")}`));
      text.append(line);
    }
    banner.append(text);
  }

  for (const button of filters) {
    button.addEventListener("click", () => {
      filter = button.dataset.voiceFilter;
      for (const other of filters) other.setAttribute("aria-pressed", String(other === button));
      renderModels();
    });
  }
  search?.addEventListener("input", renderModels);
  $("#refresh-voice")?.addEventListener("click", load);
  if (voiceTab) {
    new MutationObserver(setPanelVisible).observe(voiceTab, { attributes: true, attributeFilter: ["aria-selected"] });
  }

  // --- audio playback ----------------------------------------------------------
  const sinkSupported = typeof globalThis.HTMLMediaElement?.prototype?.setSinkId === "function";

  function clock(seconds) {
    if (!Number.isFinite(seconds)) return "0:00";
    const whole = Math.max(0, Math.round(seconds));
    return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
  }

  async function peaks(blob, count) {
    try {
      const Context = globalThis.AudioContext || globalThis.webkitAudioContext;
      const context = new Context();
      const audio = await context.decodeAudioData(await blob.arrayBuffer());
      context.close();
      const data = audio.getChannelData(0);
      const step = Math.max(1, Math.floor(data.length / count));
      const values = [];
      for (let index = 0; index < count; index += 1) {
        let peak = 0;
        for (let sample = index * step; sample < Math.min(data.length, (index + 1) * step); sample += 16) {
          peak = Math.max(peak, Math.abs(data[sample]));
        }
        values.push(peak);
      }
      const top = Math.max(...values, 0.01);
      return values.map((value) => value / top);
    } catch {
      return Array.from({ length: count }, () => 0.3);
    }
  }

  function player(blob, options = {}) {
    const url = URL.createObjectURL(blob);
    const card = element("div", "voice-player");
    if (options.title) {
      const head = element("div", "voice-player-head");
      head.append(element("strong", "", options.title));
      if (options.meta) head.append(element("span", "voice-player-meta", options.meta));
      card.append(head);
    }
    const controls = element("div", "voice-player-row");
    const audio = new Audio(url);
    audio.preload = "metadata";
    const outputId = stored("rigspark.voice.output");
    if (sinkSupported && outputId) audio.setSinkId(outputId).catch(() => {});
    const play = element("button", "voice-play");
    play.type = "button";
    play.setAttribute("aria-label", "Play");
    play.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z"/></svg>';
    const wave = element("div", "voice-wave");
    wave.setAttribute("aria-hidden", "true");
    const bars = [];
    for (let index = 0; index < 64; index += 1) {
      const bar = element("i");
      bar.style.height = "30%";
      wave.append(bar);
      bars.push(bar);
    }
    peaks(blob, 64).then((values) => {
      values.forEach((value, index) => {
        bars[index].style.height = `${Math.max(12, Math.round(value * 100))}%`;
      });
    });
    const time = element("span", "voice-time", "0:00");
    const rate = element("button", "voice-rate", "1×");
    rate.type = "button";
    rate.setAttribute("aria-label", "Playback speed");
    const rates = [1, 1.25, 1.5, 0.75];
    rate.addEventListener("click", () => {
      const next = rates[(rates.indexOf(audio.playbackRate) + 1) % rates.length];
      audio.playbackRate = next;
      rate.textContent = `${next}×`;
    });
    const download = element("a", "voice-download");
    download.href = url;
    download.download = options.filename || "rigspark-speech.wav";
    download.setAttribute("aria-label", "Download audio");
    download.title = "Download .wav";
    download.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg>';
    const update = () => {
      const progress = audio.duration ? audio.currentTime / audio.duration : 0;
      bars.forEach((bar, index) => bar.classList.toggle("played", index / bars.length < progress));
      time.textContent = `${clock(audio.currentTime)} / ${clock(audio.duration)}`;
    };
    audio.addEventListener("loadedmetadata", update);
    audio.addEventListener("timeupdate", update);
    const setPlaying = (playing) => {
      play.setAttribute("aria-label", playing ? "Pause" : "Play");
      play.innerHTML = playing
        ? '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg>'
        : '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z"/></svg>';
    };
    audio.addEventListener("play", () => setPlaying(true));
    audio.addEventListener("pause", () => setPlaying(false));
    audio.addEventListener("ended", () => setPlaying(false));
    play.addEventListener("click", () => (audio.paused ? audio.play() : audio.pause()));
    wave.addEventListener("click", (event) => {
      if (!audio.duration) return;
      const box = wave.getBoundingClientRect();
      audio.currentTime = ((event.clientX - box.left) / box.width) * audio.duration;
    });
    controls.append(play, wave, time, rate, download);
    card.append(controls);
    if (options.autoplay) audio.play().catch(() => {});
    return card;
  }

  // --- speaking -------------------------------------------------------------
  function speakModel() {
    const entry = running("speak");
    return entry ? modelById(entry.model) : null;
  }

  async function speak(text) {
    const response = await api("/api/voice/speak", {
      method: "POST",
      json: {
        text: text.slice(0, 8000),
        ...(voiceSelect?.value ? { voice: voiceSelect.value } : {}),
        speed: Number(speedSelect?.value || 1),
      },
    });
    return response.blob();
  }

  function plain(body) {
    const clone = body.cloneNode(true);
    for (const node of clone.querySelectorAll("pre, .code-actions, .voice-player, .msg-actions")) node.remove();
    return (clone.textContent || "").replace(/\s+/g, " ").trim();
  }

  async function speakInto(body, text, options = {}) {
    const holder = element("div", "voice-pending", "Speaking…");
    body.after(holder);
    try {
      const started = globalThis.performance.now();
      const blob = await speak(text);
      const seconds = ((globalThis.performance.now() - started) / 1000).toFixed(1);
      const entry = running("speak");
      holder.replaceWith(player(blob, {
        autoplay: options.autoplay,
        title: options.title,
        meta: `${entry ? entry.model : "voice"}${voiceSelect?.value ? ` · ${voiceSelect.value}` : ""} · made in ${seconds}s`,
        filename: options.filename,
      }));
      announce("Audio ready");
    } catch (error) {
      holder.className = "voice-pending voice-note-error";
      holder.textContent = `Could not speak: ${error.message}`;
    }
  }

  // A failed or stopped run must not leave a script waiting for the next reply.
  globalThis.addEventListener("rigspark:run-ended", (event) => {
    if (event.detail?.terminal !== "done") pendingScript = false;
  });

  globalThis.addEventListener("rigspark:assistant-final", (event) => {
    const body = event.detail?.body;
    if (!body) return;
    const script = pendingScript;
    pendingScript = false;
    if (!running("speak") || (!script && !speakToggle?.checked)) return;
    const text = plain(body);
    if (!text) return;
    speakInto(body, text, {
      autoplay: true,
      title: script ? "Generated audio" : undefined,
      filename: script ? "rigspark-script.wav" : undefined,
    });
  });

  // "Read aloud" on finished assistant messages.
  function decorate(row) {
    if (!row.classList.contains("assistant") || row.classList.contains("streaming")) return;
    const actions = row.querySelector(".msg-actions");
    if (!actions || actions.querySelector(".msg-read-aloud")) return;
    const button = element("button", "msg-action msg-read-aloud", "Read aloud");
    button.type = "button";
    button.hidden = !running("speak");
    button.addEventListener("click", () => {
      const body = row.querySelector(".message-body");
      const text = body ? plain(body) : "";
      if (!text || !running("speak")) return;
      speakInto(body, text, { autoplay: true });
    });
    actions.append(button);
  }
  new MutationObserver((records) => {
    for (const record of records) {
      if (record.type === "attributes") decorate(record.target);
      for (const node of record.addedNodes) {
        if (node instanceof HTMLElement && node.classList.contains("message")) decorate(node);
      }
    }
  }).observe(messages, { childList: true, attributes: true, attributeFilter: ["class"], subtree: true });

  // --- composer --------------------------------------------------------------
  function renderComposer() {
    const speaker = running("speak");
    const listener = running("listen");
    if (speakLabel) speakLabel.hidden = !speaker;
    if (dictate) dictate.hidden = !listener;
    for (const button of messages.querySelectorAll(".msg-read-aloud")) button.hidden = !speaker;
    const model = speaker ? modelById(speaker.model) : null;
    if (voiceSelect) {
      const previous = voiceSelect.value || stored("rigspark.voice.voice") || "";
      voiceSelect.textContent = "";
      const voices = model?.voices || [];
      if (!voices.length) voiceSelect.append(new Option("Default voice", ""));
      for (const voice of voices) voiceSelect.append(new Option(voice.label, voice.id));
      if ([...voiceSelect.options].some((option) => option.value === previous)) voiceSelect.value = previous;
    }
    if (chatMode === "voice" && modeNote) {
      modeNote.hidden = false;
      modeNote.textContent = speaker
        ? source === "script"
          ? `Script by the text model · voice by ${speaker.model}`
          : `Read by ${speaker.model}`
        : "Start a speak model in Models › Voice";
    }
  }

  function setSource(next) {
    source = next;
    for (const button of sources) {
      const selected = button.dataset.voiceSource === next;
      button.setAttribute("aria-checked", String(selected));
      button.tabIndex = selected ? 0 : -1;
    }
    renderComposer();
  }
  for (const button of sources) button.addEventListener("click", () => setSource(button.dataset.voiceSource));
  voiceSelect?.addEventListener("change", () => store("rigspark.voice.voice", voiceSelect.value));

  globalThis.addEventListener("rigspark:chat-mode", (event) => {
    chatMode = event.detail?.mode || "text";
    if (compose) compose.hidden = chatMode !== "voice";
    renderComposer();
  });

  function chatRow(role, label) {
    messages.querySelector(".messages-empty")?.remove();
    const row = element("div", `message ${role}`);
    const content = element("div", "message-content");
    const body = element("div", "message-body");
    content.append(element("div", "message-role", label), body);
    if (role === "assistant" && globalThis.RigSparkUI) {
      row.append(globalThis.RigSparkUI.faceImage("yes", "message-avatar"), content);
    } else {
      row.append(content);
    }
    messages.append(row);
    row.scrollIntoView({ block: "end" });
    return body;
  }

  chatForm.addEventListener("submit", (event) => {
    if (chatMode !== "voice") return;
    const text = prompt.value.trim();
    if (!text) {
      event.preventDefault();
      event.stopImmediatePropagation();
      return;
    }
    const speaker = running("speak");
    if (!speaker) {
      event.preventDefault();
      event.stopImmediatePropagation();
      const banner = $("#generation-error");
      if (banner) {
        banner.hidden = false;
        banner.textContent = "Start a speak model in Models › Voice first.";
      }
      return;
    }
    if (source === "script") {
      // chat.js sends this to the text model; the finished reply is then spoken.
      pendingScript = true;
      prompt.value = `Write the words to read aloud for this, with no headings, notes or stage directions: ${text}`;
      return;
    }
    event.preventDefault();
    event.stopImmediatePropagation();
    prompt.value = "";
    chatRow("user", "You").textContent = text;
    const body = chatRow("assistant", `Voice · ${speaker.model}`);
    body.textContent = text.length > 280 ? `${text.slice(0, 280)}…` : text;
    speakInto(body, text, { autoplay: true, title: "Generated audio", filename: "rigspark-speech.wav" });
  }, true);

  // --- dictation -------------------------------------------------------------
  let recorder = null;
  let recordTimer = null;

  function wav(buffer) {
    const samples = buffer.getChannelData(0);
    const bytes = new DataView(new ArrayBuffer(44 + samples.length * 2));
    const text = (offset, value) => [...value].forEach((char, index) => bytes.setUint8(offset + index, char.charCodeAt(0)));
    text(0, "RIFF");
    bytes.setUint32(4, 36 + samples.length * 2, true);
    text(8, "WAVE");
    text(12, "fmt ");
    bytes.setUint32(16, 16, true);
    bytes.setUint16(20, 1, true);
    bytes.setUint16(22, 1, true);
    bytes.setUint32(24, buffer.sampleRate, true);
    bytes.setUint32(28, buffer.sampleRate * 2, true);
    bytes.setUint16(32, 2, true);
    bytes.setUint16(34, 16, true);
    text(36, "data");
    bytes.setUint32(40, samples.length * 2, true);
    samples.forEach((sample, index) => {
      const clamped = Math.max(-1, Math.min(1, sample));
      bytes.setInt16(44 + index * 2, clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff, true);
    });
    return new Blob([bytes.buffer], { type: "audio/wav" });
  }

  async function toWav(blob) {
    const Context = globalThis.AudioContext || globalThis.webkitAudioContext;
    const context = new Context();
    const decoded = await context.decodeAudioData(await blob.arrayBuffer());
    context.close();
    // whisper.cpp expects 16 kHz mono.
    const offline = new OfflineAudioContext(1, Math.ceil(decoded.duration * 16000), 16000);
    const node = offline.createBufferSource();
    node.buffer = decoded;
    node.connect(offline.destination);
    node.start();
    return wav(await offline.startRendering());
  }

  function setRecording(on) {
    if (!dictate) return;
    dictate.setAttribute("aria-pressed", String(on));
    dictate.classList.toggle("recording", on);
    dictate.setAttribute("aria-label", on ? "Stop dictation" : "Dictate a message");
  }

  async function startDictation() {
    const deviceId = stored("rigspark.voice.microphone");
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: deviceId ? { deviceId: { exact: deviceId } } : true,
    });
    const chunks = [];
    recorder = new MediaRecorder(stream);
    recorder.addEventListener("dataavailable", (event) => chunks.push(event.data));
    recorder.addEventListener("stop", async () => {
      for (const track of stream.getTracks()) track.stop();
      globalThis.clearTimeout(recordTimer);
      setRecording(false);
      dictate.disabled = true;
      dictate.title = "Transcribing…";
      try {
        const audio = await toWav(new Blob(chunks, { type: recorder.mimeType }));
        const response = await api("/api/voice/transcribe", {
          method: "POST",
          headers: { "Content-Type": "audio/wav" },
          body: audio,
        });
        const data = await response.json();
        if (data.text) {
          prompt.value = prompt.value ? `${prompt.value.trimEnd()} ${data.text}` : data.text;
          prompt.dispatchEvent(new Event("input", { bubbles: true }));
          prompt.focus();
          announce("Dictation added to the message");
        }
      } catch (error) {
        const banner = $("#generation-error");
        if (banner) {
          banner.hidden = false;
          banner.textContent = `Dictation failed: ${error.message}`;
        }
      } finally {
        dictate.disabled = false;
        dictate.title = "Dictate (needs a listen model)";
        recorder = null;
      }
    });
    recorder.start();
    setRecording(true);
    announce("Listening");
    // Two minutes at most per dictation.
    recordTimer = globalThis.setTimeout(() => recorder?.state === "recording" && recorder.stop(), 120000);
  }

  dictate?.addEventListener("click", async () => {
    if (recorder?.state === "recording") {
      recorder.stop();
      return;
    }
    try {
      await startDictation();
    } catch (error) {
      setRecording(false);
      const banner = $("#generation-error");
      if (banner) {
        banner.hidden = false;
        banner.textContent = `Microphone unavailable: ${error.message}`;
      }
    }
  });

  // --- Runtime view ----------------------------------------------------------
  function renderEngines() {
    if (!engines) return;
    engines.textContent = "";
    for (const probe of catalog.engines) {
      const active = catalog.status.filter((entry) => entry.engine === probe.engine);
      const item = element("article", "runtime-row voice-engine-row");
      item.dataset.state = active.some((entry) => entry.state === "running")
        ? "on"
        : active.some((entry) => entry.state === "starting")
          ? "busy"
          : "off";
      const info = element("div", "runtime-info");
      info.append(element("div", "runtime-name", probe.label));
      const what = probe.engine === "whisper-cpp" ? "Listen" : "Speak and listen";
      const state = !probe.supported
        ? "Not available on this machine (Apple Silicon only)"
        : !probe.installed
          ? "Not installed"
          : active.length
            ? active.map((entry) => `${entry.kind} · ${entry.model} · ${entry.state}`).join(" · ")
            : "Installed · idle";
      info.append(element("div", "runtime-meta", `${what} · ${state}`));
      if (probe.supported && !probe.installed) info.append(element("div", "runtime-detail", probe.install));
      for (const entry of active.filter((entry) => entry.endpoint)) {
        info.append(element("div", "runtime-detail", entry.endpoint.replace(/^https?:\/\//, "")));
      }
      item.append(info);
      for (const entry of active) {
        const halt = element("button", "chip-btn", `Stop ${entry.kind}`);
        halt.type = "button";
        halt.addEventListener("click", () => stop(entry.kind));
        item.append(halt);
      }
      engines.append(item);
    }
    for (const entry of catalog.status.filter((entry) => entry.engine === "attached")) {
      const item = element("article", "runtime-row voice-engine-row");
      item.dataset.state = "on";
      const info = element("div", "runtime-info");
      info.append(element("div", "runtime-name", "Attached server"));
      info.append(element("div", "runtime-meta", `${entry.kind} · ${entry.model} · ${entry.endpoint.replace(/^https?:\/\//, "")}`));
      const halt = element("button", "chip-btn", "Detach");
      halt.type = "button";
      halt.addEventListener("click", () => stop(entry.kind));
      item.append(info, halt);
      engines.append(item);
    }
    if (!engines.children.length) engines.append(element("div", "empty", "No voice engines detected."));
  }

  const segments = [
    ["speak", "Speak model", "var(--amber)"],
    ["listen", "Listen model", "var(--metric-memory)"],
  ];

  async function renderMemory() {
    const bar = $("#memory-bar");
    const legend = $("#memory-legend");
    const total = $("#memory-total");
    const pressure = $("#memory-pressure");
    if (!bar || !legend || !total || !runtimeView?.classList.contains("active")) return;
    let sample = null;
    try {
      sample = await (await api("/api/telemetry")).json();
    } catch {
      sample = null;
    }
    const used = sample?.memoryUsedBytes;
    const all = sample?.memoryTotalBytes;
    bar.textContent = "";
    legend.textContent = "";
    if (!used || !all) {
      total.textContent = "Memory use unavailable";
      return;
    }
    const gib = (bytes) => `${(bytes / 1073741824).toFixed(1)} GiB`;
    total.textContent = `${gib(used)} used of ${gib(all)}`;
    const ratio = used / all;
    pressure.textContent = ratio > 0.9 ? "Memory is tight" : ratio > 0.75 ? "Getting busy" : "Plenty free";
    pressure.dataset.level = ratio > 0.9 ? "no" : ratio > 0.75 ? "slow" : "yes";
    let voiceBytes = 0;
    const parts = [];
    for (const [kind, label, color] of segments) {
      const entry = running(kind);
      const model = entry ? modelById(entry.model) : null;
      if (!model || !entry.owned) continue;
      voiceBytes += model.weightsBytes;
      parts.push({ label: `${label} · ${model.id}`, bytes: model.weightsBytes, color, approx: model.sizeBasis !== "pinned" });
    }
    const other = Math.max(0, used - voiceBytes);
    parts.unshift({
      label: activeText ? `Apps and the text model (${activeText})` : "Apps and system",
      bytes: other,
      color: "var(--text3)",
    });
    parts.push({ label: "Free", bytes: Math.max(0, all - used), color: "var(--line2)" });
    for (const part of parts) {
      const piece = element("span", "memory-segment");
      piece.style.width = `${Math.max(0.5, (part.bytes / all) * 100)}%`;
      piece.style.background = part.color;
      bar.append(piece);
      const item = element("li", "memory-key");
      const swatch = element("i");
      swatch.style.background = part.color;
      item.append(swatch, document.createTextNode(`${part.label} · ${part.approx ? "≈ " : ""}${gib(part.bytes)}`));
      legend.append(item);
    }
    bar.setAttribute("aria-label", parts.map((part) => `${part.label} ${gib(part.bytes)}`).join(", "));
  }

  async function loadActiveText() {
    try {
      const data = await (await api("/api/models/active")).json();
      activeText = data.active?.modelId || null;
    } catch {
      activeText = null;
    }
  }

  function watchRuntimeView() {
    if (!runtimeView) return;
    const update = async () => {
      const visible = runtimeView.classList.contains("active");
      if (visible) {
        await loadActiveText();
        await refreshStatus();
        await populateDevices();
        if (!memoryTimer) memoryTimer = globalThis.setInterval(renderMemory, 5000);
      } else if (memoryTimer) {
        globalThis.clearInterval(memoryTimer);
        memoryTimer = null;
      }
    };
    new MutationObserver(update).observe(runtimeView, { attributes: true, attributeFilter: ["class"] });
    update();
  }

  async function populateDevices() {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    try {
      const devices = await navigator.mediaDevices.enumerateDevices();
      const fill = (select, kind, key) => {
        if (!select) return;
        const current = stored(key) || "";
        select.textContent = "";
        select.append(new Option("System default", ""));
        let index = 0;
        for (const device of devices.filter((device) => device.kind === kind && device.deviceId && device.deviceId !== "default")) {
          index += 1;
          select.append(new Option(device.label || `${kind === "audioinput" ? "Microphone" : "Speaker"} ${index}`, device.deviceId));
        }
        select.value = [...select.options].some((option) => option.value === current) ? current : "";
      };
      fill(microphone, "audioinput", "rigspark.voice.microphone");
      if (sinkSupported) {
        if (outputRow) outputRow.hidden = false;
        fill(output, "audiooutput", "rigspark.voice.output");
      }
    } catch {
      // Device lists are optional; the system default still works.
    }
  }

  microphone?.addEventListener("change", () => store("rigspark.voice.microphone", microphone.value));
  output?.addEventListener("change", () => store("rigspark.voice.output", output.value));
  if (defaultSpeak) {
    defaultSpeak.checked = stored("rigspark.voice.speakDefault") === "1";
    defaultSpeak.addEventListener("change", () => {
      store("rigspark.voice.speakDefault", defaultSpeak.checked ? "1" : "0");
      if (speakToggle) speakToggle.checked = defaultSpeak.checked;
    });
  }
  if (speakToggle) speakToggle.checked = stored("rigspark.voice.speakDefault") === "1";

  attachForm?.addEventListener("submit", async (event) => {
    event.preventDefault();
    attachStatus.textContent = "Attaching…";
    try {
      const response = await api("/api/voice/attach", {
        method: "POST",
        json: {
          kind: $("#voice-attach-kind").value,
          endpoint: $("#voice-attach-endpoint").value.trim(),
          model: $("#voice-attach-model").value.trim(),
        },
      });
      catalog.status = (await response.json()).status;
      attachStatus.textContent = "Attached.";
      render();
    } catch (error) {
      attachStatus.textContent = `Could not attach: ${error.message}`;
    }
  });

  load();
  watchRuntimeView();
});
