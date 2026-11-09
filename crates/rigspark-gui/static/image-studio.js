// Image chat: the shape you need (thumbnail 16:9, wallpaper 9:16, square), several
// versions on new seeds, and hand-off. RigSpark serves the model; editing, text and layouts
// belong to the apps people already use, reached by copy, download or a connected app.
// Every picture runs on the local ComfyUI job API, one at a time; times are measured here.
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
    thumbnail: { label: "Thumbnail", aspect: "landscape", file: "thumbnail" },
    wallpaper: { label: "Wallpaper", aspect: "portrait", file: "wallpaper" },
    square: { label: "Square", aspect: "square", file: "image" },
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
      // Preferences are a convenience; image chat works without storage.
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
    seedChip.hidden = keptSeed === null;
    seedChip.textContent = keptSeed === null ? "" : `Keeping seed ${keptSeed} · change the words to steer it ×`;
    seedChip.setAttribute("aria-label", keptSeed === null ? "" : `Keeping seed ${keptSeed}. Click to stop keeping it`);
    if (!run && send && mode === "image") {
      send.textContent = `Generate ${variations() > 1 ? variations() : ""}`.trim();
      send.setAttribute("aria-label", send.textContent);
    }
    const model = gen()?.selectedModel("image");
    if (!model) {
      estimate.textContent = "";
      return;
    }
    const seconds = measured(model.id, PRESETS[preset].aspect);
    const n = variations();
    estimate.textContent = seconds
      ? `About ${duration(seconds * n)} (${n} × ${duration(seconds)}, measured here)`
      : "Speed shows after the first picture on this machine";
  }

  function pickPreset(value) {
    if (!PRESETS[value]) return;
    preset = value;
    write("rigspark.image.preset", value);
    refresh();
  }
  function pickCount(value) {
    count = Number(value);
    write("rigspark.image.count", value);
    refresh();
  }
  presetGroup.addEventListener("click", (event) => {
    const button = event.target.closest("[data-preset]");
    if (button) pickPreset(button.dataset.preset);
  });
  arrowKeys(presetGroup, "data-preset", pickPreset);
  countGroup.addEventListener("click", (event) => {
    const button = event.target.closest("[data-count]");
    if (button) pickCount(button.dataset.count);
  });
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
    const seeds = keptSeed === null ? Array.from({ length: variations() }, randomSeed) : [keptSeed];
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
    const card = { text, preset, choice, results: [], picked: 0, tiles: [] };
    seeds.forEach((_, index) => {
      const tile = document.createElement("button");
      tile.type = "button";
      tile.className = "variation-tile is-pending";
      tile.setAttribute("aria-pressed", "false");
      tile.setAttribute("aria-label", `Version ${index + 1}, waiting`);
      const label = document.createElement("span");
      label.className = "variation-label mono";
      label.textContent = `#${index + 1} · waiting`;
      tile.appendChild(label);
      tile.addEventListener("click", () => pick(card, index));
      grid.appendChild(tile);
      card.tiles.push(tile);
    });
    const context = { stopped: false };
    run = context;
    gen().setBusy(true);
    for (let index = 0; index < seeds.length && !context.stopped; index += 1) {
      const known = measured(model.id, choice.aspect);
      const left = known ? ` · about ${duration(known * (seeds.length - index))} left` : "";
      status.textContent = `${index} of ${seeds.length} done${left}`;
      card.tiles[index].querySelector(".variation-label").textContent = `#${index + 1} · making…`;
      try {
        const started = globalThis.performance.now();
        const job = await postJob(model, text, seeds[index], choice.aspect);
        const done = await waitJob(job.id, (event) => {
          if (event) status.textContent = `${index} of ${seeds.length} done · ${event}`;
        }, () => context.stopped);
        const seconds = (globalThis.performance.now() - started) / 1000;
        remember(model.id, choice.aspect, seconds);
        const result = {
          url: done.outputUrl,
          seed: done.seed ?? seeds[index],
          path: done.result?.result?.path || "",
          seconds,
        };
        card.results[index] = result;
        const tile = card.tiles[index];
        tile.classList.remove("is-pending");
        const image = document.createElement("img");
        image.src = result.url;
        image.alt = `Version ${index + 1} for: ${text}`;
        tile.prepend(image);
        tile.querySelector(".variation-label").textContent = `#${index + 1} · seed ${result.seed}`;
        tile.setAttribute("aria-label", `Version ${index + 1}, seed ${result.seed}`);
        if (card.results.filter(Boolean).length === 1) pick(card, index);
      } catch (error) {
        if (context.stopped) break;
        card.tiles[index].querySelector(".variation-label").textContent = `#${index + 1} · failed`;
        gen().showError(`Could not generate image: ${error.message}`);
        break;
      }
    }
    const made = card.results.filter(Boolean);
    const last = made[made.length - 1];
    status.textContent = made.length
      ? `${made.length} of ${seeds.length} done · ${duration(last.seconds)} each, measured on this machine${context.stopped ? " · stopped" : ""}`
      : context.stopped ? "Stopped" : "Nothing was generated";
    card.tiles.forEach((tile, index) => {
      if (!card.results[index]) tile.hidden = true;
    });
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

  async function copyImage(result, note) {
    try {
      const blob = await (await globalThis.fetch(result.url)).blob();
      await navigator.clipboard.write([new ClipboardItem({ [blob.type || "image/png"]: blob })]);
      note.textContent = "Copied. Paste it into any app.";
    } catch {
      note.textContent = "This browser would not copy images; use Download instead.";
    }
  }

  async function connectedApps() {
    try {
      const response = await globalThis.fetch("/api/connectors");
      if (!response.ok) return [];
      const data = await response.json();
      return (data.connectors || []).filter((connector) => connector.status === "connected");
    } catch {
      return [];
    }
  }

  // Hands the picture to a connected app through the normal chat: the text model calls that
  // connector's tools, and every call goes through the usual approval.
  function handOff(connector, card) {
    const result = card.results[card.picked];
    const modeText = document.querySelector('.chat-mode-option[data-mode="text"]');
    modeText?.click();
    prompt.value = `Using ${connector.name}, take the image file at ${result.path} (a ${card.choice.label.toLowerCase()}, prompt: "${card.text}") and `;
    prompt.focus();
    prompt.setSelectionRange(prompt.value.length, prompt.value.length);
    prompt.dispatchEvent(new Event("input", { bubbles: true }));
  }

  function renderActions(card, actions) {
    actions.hidden = false;
    actions.textContent = "";
    const keep = button("");
    const again = button(card.results.length > 1 ? `${card.results.length} new seeds` : "New seed");
    const download = document.createElement("a");
    download.className = "ghost-btn";
    const copy = button("Copy image");
    const handoff = button("Send to app…");
    handoff.setAttribute("aria-haspopup", "menu");
    handoff.setAttribute("aria-expanded", "false");
    const note = document.createElement("span");
    note.className = "variation-note";
    note.setAttribute("aria-live", "polite");
    const update = () => {
      const result = card.results[card.picked];
      keep.textContent = `Keep seed of #${card.picked + 1}`;
      download.href = result.url;
      download.download = `rigspark-${card.choice.file}-${result.seed}.png`;
      download.textContent = `Download #${card.picked + 1}`;
    };
    card.onPick = update;
    update();
    keep.addEventListener("click", () => {
      keptSeed = card.results[card.picked].seed;
      pickPreset(card.preset);
      prompt.value = card.text;
      prompt.focus();
      refresh();
    });
    again.addEventListener("click", () => {
      if (run) return;
      keptSeed = null;
      pickPreset(card.preset);
      count = card.results.length >= 4 ? 4 : card.results.length;
      refresh();
      generateInChat(card.text).catch((error) => gen().showError(error.message));
    });
    copy.addEventListener("click", () => copyImage(card.results[card.picked], note));
    handoff.addEventListener("click", async () => {
      actions.querySelector(".handoff-menu")?.remove();
      if (handoff.getAttribute("aria-expanded") === "true") {
        handoff.setAttribute("aria-expanded", "false");
        return;
      }
      const apps = await connectedApps();
      if (!apps.length) {
        note.textContent = "No app is connected. Add one under Connectors (Canva, Figma, a file store), or use Copy or Download.";
        return;
      }
      const menu = document.createElement("div");
      menu.className = "handoff-menu";
      menu.setAttribute("role", "menu");
      for (const app of apps) {
        const item = button(app.name, "");
        item.setAttribute("role", "menuitem");
        item.addEventListener("click", () => {
          menu.remove();
          handoff.setAttribute("aria-expanded", "false");
          handOff(app, card);
        });
        menu.appendChild(item);
      }
      menu.addEventListener("keydown", (event) => {
        if (event.key === "Escape") {
          menu.remove();
          handoff.setAttribute("aria-expanded", "false");
          handoff.focus();
        }
      });
      actions.appendChild(menu);
      handoff.setAttribute("aria-expanded", "true");
      menu.querySelector("button")?.focus();
    });
    actions.append(keep, again, download, copy, handoff, note);
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

  refresh();
});
