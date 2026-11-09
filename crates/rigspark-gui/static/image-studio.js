// Image chat: presets (thumbnail, vision board, wallpaper, square), variations on new seeds,
// a crisp text layer drawn by RigSpark, and a vision board composer. Every picture is made
// by the local ComfyUI job API, one at a time; times shown are measured on this machine.
document.addEventListener("DOMContentLoaded", () => {
  const form = document.querySelector("#chat-form");
  const prompt = document.querySelector("#prompt");
  const send = form?.querySelector(".send-btn");
  const imageCompose = document.querySelector("#image-compose");
  const presetGroup = document.querySelector("#image-preset");
  const countGroup = document.querySelector("#image-count");
  const seedChip = document.querySelector("#image-seed");
  const estimate = document.querySelector("#image-estimate");
  if (!form || !prompt || !imageCompose || !presetGroup || !countGroup) return;
  const gen = () => globalThis.RigSparkGeneration;

  const PRESETS = {
    thumbnail: { label: "Thumbnail", aspect: "landscape", export: [1280, 720], file: "thumbnail" },
    wallpaper: { label: "Wallpaper", aspect: "portrait", export: [1179, 2556], file: "wallpaper" },
    square: { label: "Square", aspect: "square", export: null, file: "image" },
    board: { label: "Vision board", aspect: "square", export: null, file: "board" },
  };
  const MAX_SEED = 2 ** 53 - 1;
  let preset = read("rigspark.image.preset", "thumbnail");
  if (!PRESETS[preset]) preset = "thumbnail";
  let count = Number(read("rigspark.image.count", "4"));
  if (![1, 2, 4].includes(count)) count = 4;
  let keptSeed = null;
  let mode = "text";
  let run = null;

  function read(key, fallback) {
    try {
      return globalThis.localStorage.getItem(key) ?? fallback;
    } catch {
      return fallback;
    }
  }
  function write(key, value) {
    try {
      globalThis.localStorage.setItem(key, value);
    } catch {
      // Preferences are a convenience; the studio works without storage.
    }
  }

  function randomSeed() {
    const high = Math.floor(Math.random() * 2 ** 21);
    return (high * 2 ** 32 + Math.floor(Math.random() * 2 ** 32)) % MAX_SEED;
  }

  function duration(seconds) {
    if (seconds < 90) return `${Math.round(seconds)} s`;
    const minutes = Math.floor(seconds / 60);
    const rest = Math.round(seconds % 60);
    return rest ? `${minutes} min ${rest} s` : `${minutes} min`;
  }

  function timingKey(model, aspect) {
    return `rigspark.image.seconds.${model}.${aspect}`;
  }
  function measured(model, aspect) {
    const value = Number(read(timingKey(model, aspect), ""));
    return Number.isFinite(value) && value > 0 ? value : null;
  }
  function remember(model, aspect, seconds) {
    const previous = measured(model, aspect);
    // Weighted toward the latest run; the first run includes loading the weights.
    const value = previous ? previous * 0.4 + seconds * 0.6 : seconds;
    write(timingKey(model, aspect), String(Math.round(value * 10) / 10));
  }

  function radios(group, attribute, value) {
    for (const button of group.querySelectorAll(`[${attribute}]`)) {
      const on = button.getAttribute(attribute) === String(value);
      button.setAttribute("aria-checked", String(on));
      button.tabIndex = on ? 0 : -1;
    }
  }
  function arrowKeys(group, attribute, pick) {
    group.addEventListener("keydown", (event) => {
      if (!["ArrowLeft", "ArrowRight"].includes(event.key)) return;
      const buttons = [...group.querySelectorAll(`[${attribute}]`)];
      const index = buttons.indexOf(document.activeElement);
      if (index < 0) return;
      event.preventDefault();
      const next = buttons[(index + (event.key === "ArrowRight" ? 1 : -1) + buttons.length) % buttons.length];
      pick(next.getAttribute(attribute));
      next.focus();
    });
  }

  function variations() {
    return keptSeed === null ? count : 1;
  }

  function refresh() {
    radios(presetGroup, "data-preset", preset);
    radios(countGroup, "data-count", count);
    countGroup.hidden = preset === "board";
    seedChip.hidden = keptSeed === null || preset === "board";
    seedChip.textContent = keptSeed === null ? "" : `Keeping seed ${keptSeed} · change the words to steer it ×`;
    seedChip.setAttribute("aria-label", keptSeed === null ? "" : `Keeping seed ${keptSeed}. Click to stop keeping it`);
    const model = gen()?.selectedModel("image");
    if (!run && send && mode === "image") {
      send.textContent = preset === "board" ? "Open board" : `Generate ${variations() > 1 ? variations() : ""}`.trim();
      send.setAttribute("aria-label", send.textContent);
    }
    if (!model) {
      estimate.textContent = "";
      return;
    }
    if (preset === "board") {
      estimate.textContent = "Plan goals, one look and a layout";
      return;
    }
    const seconds = measured(model.id, PRESETS[preset].aspect);
    const n = variations();
    estimate.textContent = seconds
      ? `About ${duration(seconds * n)} (${n} × ${duration(seconds)}, measured here)`
      : "Speed shows after the first picture on this machine";
  }

  presetGroup.addEventListener("click", (event) => {
    const button = event.target.closest("[data-preset]");
    if (button) pickPreset(button.dataset.preset);
  });
  function pickPreset(value) {
    preset = value;
    write("rigspark.image.preset", value);
    refresh();
  }
  arrowKeys(presetGroup, "data-preset", pickPreset);
  countGroup.addEventListener("click", (event) => {
    const button = event.target.closest("[data-count]");
    if (button) pickCount(button.dataset.count);
  });
  function pickCount(value) {
    count = Number(value);
    write("rigspark.image.count", value);
    refresh();
  }
  arrowKeys(countGroup, "data-count", pickCount);
  seedChip.addEventListener("click", () => {
    keptSeed = null;
    refresh();
  });

  globalThis.addEventListener("rigspark:chat-mode", (event) => {
    mode = event.detail?.mode;
    imageCompose.hidden = mode !== "image";
    refresh();
  });

  // ---- running jobs ----

  async function postJob(model, text, seed, aspect) {
    const { dir, port } = gen().comfyui();
    const response = await globalThis.fetch("/api/generation/jobs", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-LLMUP-Token": gen().token },
      body: JSON.stringify({ model: model.id, prompt: text, seed, aspect, comfyuiDir: dir, port, bypass: false }),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
    return data.job;
  }

  async function waitJob(id, onEvent, cancelled) {
    for (;;) {
      await new Promise((resolve) => globalThis.setTimeout(resolve, 1000));
      if (cancelled()) throw new Error("Stopped");
      const response = await globalThis.fetch("/api/generation/jobs/current");
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
      const job = data.job;
      if (!job || job.id !== id) throw new Error("Generation was replaced by another job");
      if (job.status === "running") {
        onEvent(job.events[job.events.length - 1] || "");
        continue;
      }
      if (job.status !== "succeeded") throw new Error(job.error || `Generation ${job.status}`);
      return job;
    }
  }

  async function cancelCurrent() {
    await globalThis.fetch("/api/generation/jobs/current/cancel", {
      method: "POST",
      headers: { "X-LLMUP-Token": gen().token },
    }).catch(() => {});
  }

  /// Runs one picture: returns { url, seed, seconds }.
  async function makeOne(model, text, seed, aspect, onEvent, cancelled) {
    const started = globalThis.performance.now();
    const job = await postJob(model, text, seed, aspect);
    const done = await waitJob(job.id, onEvent, cancelled);
    const seconds = (globalThis.performance.now() - started) / 1000;
    remember(model.id, aspect, seconds);
    return { url: done.outputUrl, seed: done.seed ?? seed, seconds };
  }

  function preflight() {
    const model = gen()?.selectedModel("image");
    if (!model) throw new Error("No local image model is available");
    if (!gen().comfyui().dir) {
      throw new Error("ComfyUI was not detected. Set RIGSPARK_COMFYUI_DIR before starting rigspark");
    }
    if (!model.runnable) throw new Error(`${model.id} has no built-in ComfyUI workflow yet`);
    if (model.fit.verdict === "no") throw new Error(`${model.id} does not fit this machine (${model.fit.reason})`);
    return model;
  }

  function button(label, className = "ghost-btn") {
    const element = document.createElement("button");
    element.type = "button";
    element.className = className;
    element.textContent = label;
    return element;
  }

  async function generateInChat(text) {
    const model = preflight();
    const choice = PRESETS[preset];
    const total = variations();
    const seeds = keptSeed === null ? Array.from({ length: total }, randomSeed) : [keptSeed];
    gen().chatRow("user", "You").textContent = text;
    const body = gen().chatRow("assistant", `Image · ${model.id} · ${choice.label}`);
    const status = document.createElement("p");
    status.className = "generation-chat-status";
    const grid = document.createElement("div");
    grid.className = `variation-grid variation-${choice.aspect}`;
    const actions = document.createElement("div");
    actions.className = "variation-actions";
    actions.hidden = true;
    body.append(status, grid, actions);
    const card = { model, text, preset, choice, results: [], picked: 0, tiles: [], text_layer: null };
    for (let index = 0; index < seeds.length; index += 1) {
      const tile = document.createElement("button");
      tile.type = "button";
      tile.className = "variation-tile is-pending";
      tile.setAttribute("aria-pressed", "false");
      tile.setAttribute("aria-label", `Variation ${index + 1}, waiting`);
      const label = document.createElement("span");
      label.className = "variation-label mono";
      label.textContent = `#${index + 1} · waiting`;
      tile.appendChild(label);
      tile.addEventListener("click", () => pick(card, index));
      grid.appendChild(tile);
      card.tiles.push(tile);
    }
    const context = { stopped: false };
    run = context;
    gen().setBusy(true);
    for (let index = 0; index < seeds.length && !context.stopped; index += 1) {
      const known = measured(model.id, choice.aspect);
      const left = known ? ` · about ${duration(known * (seeds.length - index))} left` : "";
      status.textContent = `${index} of ${seeds.length} done${left}`;
      card.tiles[index].querySelector(".variation-label").textContent = `#${index + 1} · making…`;
      try {
        const result = await makeOne(model, text, seeds[index], choice.aspect, (event) => {
          if (event) status.textContent = `${index} of ${seeds.length} done · ${event}`;
        }, () => context.stopped);
        card.results[index] = result;
        const tile = card.tiles[index];
        tile.classList.remove("is-pending");
        const image = document.createElement("img");
        image.src = result.url;
        image.alt = `Variation ${index + 1} for: ${text}`;
        tile.prepend(image);
        tile.querySelector(".variation-label").textContent = `#${index + 1} · seed ${result.seed}`;
        tile.setAttribute("aria-label", `Variation ${index + 1}, seed ${result.seed}`);
        if (card.results.filter(Boolean).length === 1) pick(card, index);
      } catch (error) {
        if (context.stopped) break;
        card.tiles[index].querySelector(".variation-label").textContent = `#${index + 1} · failed`;
        gen().showError(`Could not generate image: ${error.message}`);
        break;
      }
    }
    const made = card.results.filter(Boolean);
    const last = made.length ? made[made.length - 1] : null;
    status.textContent = made.length
      ? `${made.length} of ${seeds.length} done · ${duration(last.seconds)} each, measured on this machine${context.stopped ? " · stopped" : ""}`
      : context.stopped ? "Stopped" : "Nothing was generated";
    for (let index = made.length; index < card.tiles.length; index += 1) {
      if (!card.results[index]) card.tiles[index].hidden = true;
    }
    if (made.length) renderActions(card, actions);
    gen().announce(made.length ? `${made.length} images ready` : "Generation stopped");
    run = null;
    gen().setBusy(false);
    refresh();
  }

  function pick(card, index) {
    if (!card.results[index]) return;
    card.picked = index;
    card.tiles.forEach((tile, position) => {
      tile.classList.toggle("is-picked", position === index);
      tile.setAttribute("aria-pressed", String(position === index));
    });
    card.onPick?.();
  }

  function renderActions(card, actions) {
    actions.hidden = false;
    actions.textContent = "";
    const text = button("Add text", "accent-btn");
    const more = button("");
    const again = button(card.results.length > 1 ? `${card.results.length} new seeds` : "New seed");
    const download = document.createElement("a");
    download.className = "ghost-btn";
    const update = () => {
      const result = card.results[card.picked];
      more.textContent = `Keep seed of #${card.picked + 1}`;
      download.href = result.url;
      download.download = `rigspark-${card.choice.file}-${result.seed}.png`;
      download.textContent = `Download #${card.picked + 1}`;
    };
    card.onPick = update;
    update();
    text.addEventListener("click", () => openEditor(card));
    more.addEventListener("click", () => {
      keptSeed = card.results[card.picked].seed;
      preset = card.preset;
      prompt.value = card.text;
      prompt.focus();
      refresh();
    });
    again.addEventListener("click", () => {
      if (run) return;
      keptSeed = null;
      preset = card.preset;
      count = card.results.length >= 4 ? 4 : card.results.length;
      refresh();
      generateInChat(card.text).catch((error) => gen().showError(error.message));
    });
    actions.append(text, more, again, download);
  }

  form.addEventListener("submit", async (event) => {
    if (mode !== "image") return;
    event.preventDefault();
    event.stopImmediatePropagation();
    gen()?.showError("");
    if (run) {
      run.stopped = true;
      await cancelCurrent();
      return;
    }
    if (preset === "board") {
      openBoard(prompt.value.trim());
      return;
    }
    const text = prompt.value.trim();
    if (!text) return;
    try {
      preflight();
      prompt.value = "";
      await generateInChat(text);
    } catch (error) {
      gen()?.showError(`Could not generate image: ${error.message}`);
      run = null;
      gen()?.setBusy(false);
      refresh();
    }
  }, true);

  // ---- shared drawing ----

  const FONTS = {
    inter: { family: "Inter", weight: 900 },
    anton: { family: "Anton", weight: 400 },
  };

  function loadImage(url) {
    return new Promise((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve(image);
      image.onerror = () => reject(new Error("could not load the picture"));
      image.src = url;
    });
  }

  function cover(context, image, x, y, width, height) {
    const scale = Math.max(width / image.width, height / image.height);
    const w = image.width * scale;
    const h = image.height * scale;
    context.save();
    context.beginPath();
    context.rect(x, y, width, height);
    context.clip();
    context.drawImage(image, x + (width - w) / 2, y + (height - h) / 2, w, h);
    context.restore();
  }

  function wrap(context, text, maxWidth) {
    const lines = [];
    let line = "";
    for (const word of text.split(/\s+/).filter(Boolean)) {
      const next = line ? `${line} ${word}` : word;
      if (line && context.measureText(next).width > maxWidth) {
        lines.push(line);
        line = word;
      } else {
        line = next;
      }
    }
    if (line) lines.push(line);
    return lines;
  }

  function luminance(hex) {
    const channel = (value) => {
      const c = value / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    };
    const n = Number.parseInt(hex.slice(1), 16);
    return 0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255);
  }

  // ---- thumbnail and wallpaper text editor ----

  const editor = document.querySelector("#thumb-editor");
  const canvas = document.querySelector("#thumb-canvas");
  const field = {
    text: document.querySelector("#thumb-text"),
    words: document.querySelector("#thumb-words"),
    font: document.querySelector("#thumb-font"),
    position: document.querySelector("#thumb-position"),
    size: document.querySelector("#thumb-size"),
    fill: document.querySelector("#thumb-fill"),
    outline: document.querySelector("#thumb-outline"),
    contrast: document.querySelector("#thumb-contrast"),
    safe: document.querySelector("#thumb-safe"),
    photo: document.querySelector("#thumb-photo"),
    photoSize: document.querySelector("#thumb-photo-size"),
    meta: document.querySelector("#thumb-editor-meta"),
    feed: document.querySelector("#thumb-feed"),
    export: document.querySelector("#thumb-export"),
    three: document.querySelector("#thumb-export-three"),
    close: document.querySelector("#thumb-close"),
  };
  const layer = { text: "", font: "inter", position: "left", size: 13, fill: "#ffffff", outline: true, photo: null, photoSize: 80 };
  let editing = null;

  function exportSize(card, image) {
    return card.choice.export || [image.width, image.height];
  }

  function textBox(context, width, height) {
    const font = FONTS[layer.font];
    const size = Math.round(height * (layer.size / 100));
    context.font = `${font.weight} ${size}px ${font.family}`;
    const side = layer.position === "left" || layer.position === "right";
    const maxWidth = width * (side ? 0.5 : 0.9);
    const lines = wrap(context, layer.text, maxWidth);
    const lineHeight = size * 0.98;
    const blockHeight = lines.length * lineHeight;
    const margin = Math.round(Math.min(width, height) * 0.06);
    const widest = Math.max(0, ...lines.map((line) => context.measureText(line).width));
    let x = margin;
    let y = (height - blockHeight) / 2;
    if (layer.position === "right") x = width - margin - widest;
    if (layer.position === "top") {
      x = (width - widest) / 2;
      y = margin;
    }
    if (layer.position === "bottom") {
      x = (width - widest) / 2;
      // Above YouTube's timestamp and a phone's home bar.
      y = height - margin * 2 - blockHeight;
    }
    return { lines, size, lineHeight, x, y, widest, blockHeight };
  }

  function paint(context, image, width, height, overlays) {
    context.clearRect(0, 0, width, height);
    cover(context, image, 0, 0, width, height);
    if (layer.photo) {
      const h = height * (layer.photoSize / 100);
      const w = layer.photo.width * (h / layer.photo.height);
      const left = layer.position === "right" ? width * 0.02 : width - w - width * 0.02;
      context.drawImage(layer.photo, left, height - h, w, h);
    }
    if (layer.text.trim()) {
      const box = textBox(context, width, height);
      context.textBaseline = "top";
      context.lineJoin = "round";
      box.lines.forEach((line, index) => {
        const lineWidth = context.measureText(line).width;
        const x = layer.position === "top" || layer.position === "bottom" ? (width - lineWidth) / 2 : box.x;
        const y = box.y + index * box.lineHeight;
        if (layer.outline) {
          context.lineWidth = Math.max(2, box.size * 0.14);
          context.strokeStyle = layer.fill === "#111111" ? "#ffffff" : "#111111";
          context.strokeText(line, x, y);
        }
        context.fillStyle = layer.fill;
        context.fillText(line, x, y);
      });
    }
    if (overlays && field.safe.checked) {
      context.save();
      context.setLineDash([Math.max(6, width / 160), Math.max(4, width / 240)]);
      context.lineWidth = Math.max(2, width / 640);
      context.strokeStyle = "#ffffffcc";
      const edge = Math.min(width, height) * 0.04;
      context.strokeRect(edge, edge, width - edge * 2, height - edge * 2);
      if (editing?.card.preset === "thumbnail") {
        // YouTube draws the video length over the bottom-right corner.
        const w = width * 0.11;
        const h = height * 0.07;
        context.fillStyle = "#000000aa";
        context.fillRect(width - w - width * 0.012, height - h - height * 0.02, w, h);
        context.fillStyle = "#ffffff";
        context.font = `600 ${Math.round(h * 0.55)}px Inter`;
        context.textBaseline = "middle";
        context.fillText("12:34", width - w - width * 0.012 + w * 0.18, height - h / 2 - height * 0.02);
      } else {
        // A phone's lock-screen clock sits in the top fifth.
        context.fillStyle = "#00000055";
        context.fillRect(0, height * 0.08, width, height * 0.14);
      }
      context.restore();
    }
  }

  function contrastNote(image, width, height) {
    if (!layer.text.trim()) return "";
    const probe = document.createElement("canvas");
    probe.width = width;
    probe.height = height;
    const context = probe.getContext("2d", { willReadFrequently: true });
    cover(context, image, 0, 0, width, height);
    const box = textBox(context, width, height);
    const x = Math.max(0, Math.floor(box.x));
    const y = Math.max(0, Math.floor(box.y));
    const w = Math.max(1, Math.min(width - x, Math.ceil(box.widest)));
    const h = Math.max(1, Math.min(height - y, Math.ceil(box.blockHeight)));
    const data = context.getImageData(x, y, w, h).data;
    let total = 0;
    let samples = 0;
    for (let index = 0; index < data.length; index += 16) {
      const hex = `#${[data[index], data[index + 1], data[index + 2]].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
      total += luminance(hex);
      samples += 1;
    }
    const behind = total / Math.max(1, samples);
    const text = luminance(layer.fill);
    const ratio = (Math.max(text, behind) + 0.05) / (Math.min(text, behind) + 0.05);
    const level = ratio >= 4.5 ? "good" : ratio >= 3 ? "fair" : "low";
    field.contrast.dataset.level = layer.outline ? "good" : level;
    return `Text against the picture behind it: ${ratio.toFixed(1)} : 1${layer.outline ? " · the outline keeps it readable" : level === "low" ? " · too low, add the outline or change colour" : ""}`;
  }

  function draw() {
    if (!editing) return;
    const { image, card } = editing;
    const [width, height] = exportSize(card, image);
    canvas.width = width;
    canvas.height = height;
    paint(canvas.getContext("2d"), image, width, height, true);
    for (const feed of field.feed.querySelectorAll("[data-feed]")) {
      const context = feed.getContext("2d");
      context.imageSmoothingQuality = "high";
      context.drawImage(canvas, 0, 0, feed.width, feed.height);
    }
    // Feed previews must not show the safe-zone guides.
    if (field.safe.checked) {
      const clean = document.createElement("canvas");
      clean.width = width;
      clean.height = height;
      paint(clean.getContext("2d"), image, width, height, false);
      for (const feed of field.feed.querySelectorAll("[data-feed]")) {
        feed.getContext("2d").drawImage(clean, 0, 0, feed.width, feed.height);
      }
    }
    const words = layer.text.trim() ? layer.text.trim().split(/\s+/).length : 0;
    field.words.textContent = words === 0
      ? "No words yet"
      : words <= 4 ? `${words} word${words === 1 ? "" : "s"} · reads at a glance` : `${words} words · try 2 to 4; long text vanishes on phones`;
    field.words.dataset.level = words <= 4 ? "good" : "fair";
    field.contrast.textContent = contrastNote(image, width, height);
  }

  async function exportCard(card, index) {
    const result = card.results[index];
    const image = await loadImage(result.url);
    const [width, height] = exportSize(card, image);
    const out = document.createElement("canvas");
    out.width = width;
    out.height = height;
    paint(out.getContext("2d"), image, width, height, false);
    const blob = await new Promise((resolve) => out.toBlob(resolve, "image/png"));
    if (!blob) throw new Error("could not encode the PNG");
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = `rigspark-${card.choice.file}-${result.seed}-${width}x${height}.png`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    globalThis.setTimeout(() => URL.revokeObjectURL(link.href), 60_000);
  }

  async function openEditor(card) {
    try {
      await Promise.all([
        document.fonts.load("900 40px Inter"),
        document.fonts.load("400 40px Anton"),
      ]).catch(() => {});
      const result = card.results[card.picked];
      const image = await loadImage(result.url);
      editing = { card, image };
      layer.text = card.text_layer?.text ?? "";
      field.text.value = layer.text;
      const [width, height] = exportSize(card, image);
      field.meta.textContent = `#${card.picked + 1} · seed ${result.seed} · ${width}×${height}`;
      field.export.textContent = `Export PNG · ${width}×${height}`;
      field.feed.hidden = card.preset !== "thumbnail";
      field.three.hidden = card.preset !== "thumbnail" || card.results.filter(Boolean).length < 3;
      draw();
      editor.showModal();
      field.text.focus();
    } catch (error) {
      gen().showError(`Could not open the text editor: ${error.message}`);
    }
  }

  field.text?.addEventListener("input", () => {
    layer.text = field.text.value;
    if (editing) editing.card.text_layer = { text: layer.text };
    draw();
  });
  field.font?.addEventListener("change", () => { layer.font = field.font.value; draw(); });
  field.position?.addEventListener("change", () => { layer.position = field.position.value; draw(); });
  field.size?.addEventListener("input", () => { layer.size = Number(field.size.value); draw(); });
  field.outline?.addEventListener("change", () => { layer.outline = field.outline.checked; draw(); });
  field.safe?.addEventListener("change", draw);
  field.photoSize?.addEventListener("input", () => { layer.photoSize = Number(field.photoSize.value); draw(); });
  field.fill?.addEventListener("click", (event) => {
    const swatch = event.target.closest("[data-fill]");
    if (!swatch) return;
    layer.fill = swatch.dataset.fill;
    radios(field.fill, "data-fill", layer.fill);
    draw();
  });
  field.photo?.addEventListener("change", async () => {
    const file = field.photo.files?.[0];
    layer.photo = null;
    if (file && file.type === "image/png" && file.size <= 25 * 1024 * 1024) {
      try {
        layer.photo = await createImageBitmap(file);
      } catch {
        gen().showError("That PNG could not be read");
      }
    }
    field.photoSize.disabled = !layer.photo;
    draw();
  });
  field.export?.addEventListener("click", () => {
    if (!editing) return;
    exportCard(editing.card, editing.card.picked).catch((error) => gen().showError(`Export failed: ${error.message}`));
  });
  field.three?.addEventListener("click", async () => {
    if (!editing) return;
    const { card } = editing;
    const order = [card.picked, ...card.results.map((_, index) => index).filter((index) => index !== card.picked && card.results[index])];
    try {
      for (const index of order.slice(0, 3)) await exportCard(card, index);
      gen().announce("Three thumbnails exported for YouTube Test & Compare");
    } catch (error) {
      gen().showError(`Export failed: ${error.message}`);
    }
  });
  field.close?.addEventListener("click", () => editor.close());
  editor?.addEventListener("close", () => {
    editing = null;
  });

  // ---- vision board ----

  const board = document.querySelector("#board-studio");
  const boardCanvas = document.querySelector("#board-canvas");
  const goalsList = document.querySelector("#board-goals");
  const boardTitle = document.querySelector("#board-title");
  const styleGroup = document.querySelector("#board-style");
  const layoutGroup = document.querySelector("#board-layout");
  const boardRun = document.querySelector("#board-run");
  const boardExport = document.querySelector("#board-export");
  const boardStatus = document.querySelector("#board-status");
  const boardSize = document.querySelector("#board-size");
  const LAYOUTS = {
    grid: { size: [3840, 2560], columns: 3, title: 0.13, tilt: false, label: "3840×2560" },
    collage: { size: [3840, 2560], columns: 3, title: 0.13, tilt: true, label: "3840×2560" },
    wallpaper: { size: [1179, 2556], columns: 2, title: 0.26, tilt: false, label: "1179×2556 · clock area left clear" },
    print: { size: [2480, 3508], columns: 2, title: 0.1, tilt: false, label: "A4 at 300 dpi · 2480×3508" },
  };
  const AREAS = ["Health", "Work", "Travel", "Home", "Money", "People"];
  const boardState = { layout: "grid", style: styleGroup?.querySelector("[aria-checked=true]")?.dataset.style || "", goals: [], running: null };

  function saveBoard() {
    write("rigspark.board", JSON.stringify({
      title: boardTitle.value,
      layout: boardState.layout,
      style: boardState.style,
      goals: boardState.goals.map(({ area, words, picture }) => ({ area, words, picture })),
    }));
  }
  function loadBoard() {
    let saved = null;
    try {
      saved = JSON.parse(read("rigspark.board", "null"));
    } catch {
      saved = null;
    }
    const goals = Array.isArray(saved?.goals) && saved.goals.length ? saved.goals : AREAS.map((area) => ({ area, words: "", picture: "" }));
    boardState.goals = goals.slice(0, 9).map((goal) => ({
      area: String(goal.area || "").slice(0, 30),
      words: String(goal.words || "").slice(0, 40),
      picture: String(goal.picture || "").slice(0, 400),
      image: null,
      seed: null,
      state: "",
    }));
    if (saved && LAYOUTS[saved.layout]) boardState.layout = saved.layout;
    if (saved?.style) boardState.style = String(saved.style).slice(0, 200);
    if (saved?.title !== undefined) boardTitle.value = String(saved.title).slice(0, 40);
  }

  function goalRow(goal, index) {
    const item = document.createElement("li");
    item.className = "board-goal";
    const thumb = document.createElement("canvas");
    thumb.width = 88;
    thumb.height = 88;
    thumb.className = "board-goal-thumb";
    thumb.setAttribute("aria-hidden", "true");
    if (goal.image) cover(thumb.getContext("2d"), goal.image, 0, 0, 88, 88);
    const fields = document.createElement("div");
    fields.className = "board-goal-fields";
    const make = (key, label, placeholder, max) => {
      const wrap = document.createElement("label");
      wrap.className = "studio-field";
      const caption = document.createElement("span");
      caption.className = "sr-only";
      caption.textContent = `${label} for goal ${index + 1}`;
      const input = document.createElement("input");
      input.type = "text";
      input.maxLength = max;
      input.placeholder = placeholder;
      input.value = goal[key];
      input.addEventListener("input", () => {
        goal[key] = input.value;
        saveBoard();
        if (key === "words" || key === "area") compose();
      });
      wrap.append(caption, input);
      return wrap;
    };
    const top = document.createElement("div");
    top.className = "board-goal-top";
    top.append(make("area", "Area", "Area", 30), make("words", "Words on the tile", "Words on the tile", 40));
    fields.append(top, make("picture", "Picture", "What the picture shows", 400));
    const side = document.createElement("div");
    side.className = "board-goal-side";
    const state = document.createElement("span");
    state.className = "studio-note";
    state.textContent = goal.state;
    const redo = button(goal.image ? "Redo" : "Make", "ghost-btn ghost-btn-sm");
    redo.disabled = Boolean(boardState.running);
    redo.addEventListener("click", () => runBoard([index]));
    const own = document.createElement("label");
    own.className = "ghost-btn ghost-btn-sm board-own";
    own.textContent = "My photo";
    const file = document.createElement("input");
    file.type = "file";
    file.accept = "image/png,image/jpeg,image/webp";
    file.className = "sr-only";
    file.addEventListener("change", async () => {
      const chosen = file.files?.[0];
      if (!chosen || chosen.size > 25 * 1024 * 1024) return;
      try {
        goal.image = await createImageBitmap(chosen);
        goal.state = "Your photo";
        goal.seed = null;
        renderGoals();
        compose();
      } catch {
        gen()?.showError("That photo could not be read");
      }
    });
    own.appendChild(file);
    const remove = button("Remove", "ghost-btn ghost-btn-sm");
    remove.disabled = boardState.goals.length <= 1 || Boolean(boardState.running);
    remove.addEventListener("click", () => {
      boardState.goals.splice(index, 1);
      saveBoard();
      renderGoals();
      compose();
    });
    side.append(state, redo, own, remove);
    item.append(thumb, fields, side);
    return item;
  }

  function renderGoals() {
    goalsList.textContent = "";
    boardState.goals.forEach((goal, index) => goalsList.appendChild(goalRow(goal, index)));
    document.querySelector("#board-add").disabled = boardState.goals.length >= 9;
  }

  function compose() {
    const layout = LAYOUTS[boardState.layout];
    const [width, height] = layout.size;
    boardCanvas.width = width;
    boardCanvas.height = height;
    boardSize.textContent = layout.label;
    const context = boardCanvas.getContext("2d");
    context.fillStyle = "#faf9f5";
    context.fillRect(0, 0, width, height);
    const pad = Math.round(Math.min(width, height) * 0.04);
    const titleBand = Math.round(height * layout.title);
    const title = boardTitle.value.trim();
    if (title) {
      context.fillStyle = "#1d1d1f";
      context.font = `900 ${Math.round(Math.min(titleBand * 0.42, width * 0.08))}px Inter`;
      context.textBaseline = "bottom";
      const measure = context.measureText(title).width;
      context.fillText(title, (width - measure) / 2, titleBand - pad * 0.4);
    }
    const goals = boardState.goals;
    const columns = Math.min(layout.columns, Math.max(1, goals.length));
    const rows = Math.ceil(goals.length / columns);
    const gap = Math.round(pad * 0.6);
    const cellWidth = (width - pad * 2 - gap * (columns - 1)) / columns;
    const cellHeight = (height - titleBand - pad - gap * (rows - 1)) / rows;
    goals.forEach((goal, index) => {
      const column = index % columns;
      const row = Math.floor(index / columns);
      const x = pad + column * (cellWidth + gap);
      const y = titleBand + row * (cellHeight + gap);
      context.save();
      if (layout.tilt) {
        const angle = [-2, 1.5, -1, 2, -1.5, 1, -2.5, 1.2, -0.8][index % 9] * (Math.PI / 180);
        context.translate(x + cellWidth / 2, y + cellHeight / 2);
        context.rotate(angle);
        context.translate(-(x + cellWidth / 2), -(y + cellHeight / 2));
        context.fillStyle = "#ffffff";
        context.fillRect(x - gap * 0.35, y - gap * 0.35, cellWidth + gap * 0.7, cellHeight + gap * 0.7);
      }
      if (goal.image) {
        cover(context, goal.image, x, y, cellWidth, cellHeight);
      } else {
        context.fillStyle = "#e6e6ea";
        context.fillRect(x, y, cellWidth, cellHeight);
        context.fillStyle = "#6e6e73";
        context.font = `500 ${Math.round(cellHeight * 0.07)}px Inter`;
        context.textBaseline = "middle";
        const label = goal.area || "Goal";
        context.fillText(label, x + (cellWidth - context.measureText(label).width) / 2, y + cellHeight / 2);
      }
      const words = goal.words.trim();
      if (words) {
        const size = Math.round(Math.min(cellHeight * 0.075, cellWidth * 0.07));
        context.font = `700 ${size}px Inter`;
        context.textBaseline = "top";
        const lines = wrap(context, words, cellWidth * 0.86);
        const inset = size * 0.45;
        const boxHeight = lines.length * size * 1.15 + inset * 1.4;
        const boxWidth = Math.max(...lines.map((line) => context.measureText(line).width)) + inset * 2;
        const boxY = y + cellHeight - boxHeight - inset;
        context.fillStyle = "#1d1d1f";
        context.fillRect(x + inset, boxY, boxWidth, boxHeight);
        context.fillStyle = "#ffffff";
        lines.forEach((line, lineIndex) => context.fillText(line, x + inset * 2, boxY + inset * 0.7 + lineIndex * size * 1.15));
      }
      context.restore();
    });
    boardExport.disabled = !goals.some((goal) => goal.image);
  }

  async function runBoard(indexes) {
    if (boardState.running) return;
    let model;
    try {
      model = preflight();
    } catch (error) {
      boardStatus.textContent = error.message;
      return;
    }
    const todo = indexes.filter((index) => boardState.goals[index]?.picture.trim());
    if (!todo.length) {
      boardStatus.textContent = "Describe at least one picture first";
      return;
    }
    const context = { stopped: false };
    boardState.running = context;
    boardRun.textContent = "Pause";
    for (const index of todo) boardState.goals[index].state = "Queued";
    renderGoals();
    let done = 0;
    for (const index of todo) {
      if (context.stopped) break;
      const goal = boardState.goals[index];
      if (!goal) continue;
      const known = measured(model.id, "square");
      const left = known ? ` · about ${duration(known * (todo.length - done))} left` : "";
      boardStatus.textContent = `${done} of ${todo.length}${left}`;
      goal.state = "Making…";
      renderGoals();
      try {
        const result = await makeOne(model, `${goal.picture.trim()}, ${boardState.style}`, randomSeed(), "square", (event) => {
          if (event) boardStatus.textContent = `${done} of ${todo.length} · ${event}`;
        }, () => context.stopped);
        goal.image = await loadImage(result.url);
        goal.seed = result.seed;
        goal.state = "Done";
        done += 1;
        compose();
      } catch (error) {
        goal.state = context.stopped ? "Paused" : "Failed";
        if (!context.stopped) boardStatus.textContent = `Tile ${index + 1} failed: ${error.message}`;
        renderGoals();
        if (!context.stopped) break;
      }
    }
    for (const index of todo) {
      const goal = boardState.goals[index];
      if (goal && goal.state === "Queued") goal.state = context.stopped ? "Paused" : "";
    }
    const seconds = measured(model.id, "square");
    if (!boardStatus.textContent.startsWith("Tile")) {
      boardStatus.textContent = `${done} of ${todo.length} made${seconds ? ` · ${duration(seconds)} each, measured here` : ""}${context.stopped ? " · paused" : ""}`;
    }
    boardState.running = null;
    boardRun.textContent = "Make tiles";
    renderGoals();
    compose();
  }

  function openBoard(seedText) {
    loadBoard();
    if (seedText && boardState.goals.every((goal) => !goal.picture)) {
      boardState.goals[0].picture = seedText;
    }
    radios(layoutGroup, "data-layout", boardState.layout);
    for (const option of styleGroup.querySelectorAll("[data-style]")) {
      const on = option.dataset.style === boardState.style;
      option.setAttribute("aria-checked", String(on));
      option.tabIndex = on ? 0 : -1;
    }
    renderGoals();
    document.fonts.load("900 40px Inter").catch(() => {}).finally(compose);
    board.showModal();
  }

  boardRun?.addEventListener("click", async () => {
    if (boardState.running) {
      boardState.running.stopped = true;
      await cancelCurrent();
      return;
    }
    const missing = boardState.goals.map((goal, index) => (goal.image ? -1 : index)).filter((index) => index >= 0);
    runBoard(missing.length ? missing : boardState.goals.map((_, index) => index));
  });
  layoutGroup?.addEventListener("click", (event) => {
    const option = event.target.closest("[data-layout]");
    if (!option) return;
    boardState.layout = option.dataset.layout;
    radios(layoutGroup, "data-layout", boardState.layout);
    saveBoard();
    compose();
  });
  arrowKeys(layoutGroup, "data-layout", (value) => {
    boardState.layout = value;
    radios(layoutGroup, "data-layout", value);
    saveBoard();
    compose();
  });
  styleGroup?.addEventListener("click", (event) => {
    const option = event.target.closest("[data-style]");
    if (!option) return;
    boardState.style = option.dataset.style;
    radios(styleGroup, "data-style", boardState.style);
    saveBoard();
  });
  boardTitle?.addEventListener("input", () => {
    saveBoard();
    compose();
  });
  document.querySelector("#board-add")?.addEventListener("click", () => {
    if (boardState.goals.length >= 9) return;
    boardState.goals.push({ area: "", words: "", picture: "", image: null, seed: null, state: "" });
    saveBoard();
    renderGoals();
    compose();
  });
  boardExport?.addEventListener("click", () => {
    boardCanvas.toBlob((blob) => {
      if (!blob) return;
      const link = document.createElement("a");
      link.href = URL.createObjectURL(blob);
      const [width, height] = LAYOUTS[boardState.layout].size;
      link.download = `rigspark-vision-board-${boardState.layout}-${width}x${height}.png`;
      document.body.appendChild(link);
      link.click();
      link.remove();
      globalThis.setTimeout(() => URL.revokeObjectURL(link.href), 60_000);
    }, "image/png");
  });
  document.querySelector("#board-close")?.addEventListener("click", () => board.close());
  board?.addEventListener("cancel", (event) => {
    if (boardState.running) event.preventDefault();
  });

  refresh();
});
