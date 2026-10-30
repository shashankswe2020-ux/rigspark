"use strict";

// Add from Hugging Face: search or paste a repository, pick a quantization, add it to the
// local catalog. Network requests happen only on Search and Add.
(() => {
  const sheet = document.querySelector("#hf-sheet");
  const open = document.querySelector("#hf-add-open");
  if (!sheet || !open) return;
  const form = document.querySelector("#hf-search-form");
  const query = document.querySelector("#hf-query");
  const results = document.querySelector("#hf-results");
  const choices = document.querySelector("#hf-choices");
  const repoLabel = document.querySelector("#hf-repo");
  const quantList = document.querySelector("#hf-quant-list");
  const status = document.querySelector("#hf-status");
  const add = document.querySelector("#hf-add");
  let repository = null;
  let returnFocus = null;

  const gigabytes = (bytes) => `${(bytes / 1e9).toFixed(1)} GB`;
  const repoPattern = /^(?:https:\/\/huggingface\.co\/|hf\.co\/)?[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*(?::[A-Za-z0-9_]+)?$/;

  function say(text) {
    status.textContent = text;
  }

  async function post(path, payload) {
    const response = await globalThis.fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(typeof data.error === "string" ? data.error : `request failed (${response.status})`);
    return data;
  }

  function reset() {
    repository = null;
    results.hidden = true;
    results.replaceChildren();
    choices.hidden = true;
    quantList.replaceChildren();
    add.disabled = true;
  }

  function show() {
    returnFocus = document.activeElement;
    reset();
    sheet.hidden = false;
    query.focus();
  }

  function hide() {
    if (sheet.hidden) return;
    sheet.hidden = true;
    if (returnFocus && typeof returnFocus.focus === "function") returnFocus.focus();
  }

  async function inspect(reference) {
    reset();
    say(`Reading ${reference} from Hugging Face…`);
    try {
      const data = await post("/api/catalog/local/inspect", { reference });
      repository = data.repository;
      repoLabel.textContent = `${data.repository} · ${data.license} · commit ${String(data.revision).slice(0, 12)}`;
      for (const choice of Array.isArray(data.choices) ? data.choices : []) {
        const item = document.createElement("li");
        const label = document.createElement("label");
        const radio = document.createElement("input");
        radio.type = "radio";
        radio.name = "hf-quant";
        radio.value = choice.quant;
        radio.checked = choice.id === data.selected;
        const name = document.createElement("span");
        name.prepend(radio, document.createTextNode(choice.quant));
        const detail = document.createElement("small");
        detail.textContent = `${gigabytes(choice.bytes)} · ${choice.fits ? "fits" : "too large"}`;
        label.append(name, detail);
        item.append(label);
        quantList.append(item);
      }
      choices.hidden = false;
      add.disabled = !quantList.querySelector("input:checked");
      say(data.selected ? "The largest quantization that fits is selected." : "No quantization fits this machine.");
    } catch (error) {
      say(error.message);
    }
  }

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const text = query.value.trim();
    if (!text) return;
    if (repoPattern.test(text) && text.includes("/")) {
      await inspect(text);
      return;
    }
    reset();
    say("Searching Hugging Face…");
    try {
      const data = await post("/api/catalog/local/search", { query: text });
      const hits = Array.isArray(data.results) ? data.results : [];
      for (const hit of hits) {
        const item = document.createElement("li");
        const button = document.createElement("button");
        button.type = "button";
        const name = document.createElement("span");
        name.textContent = hit.id;
        const downloads = document.createElement("small");
        downloads.textContent = `${Number(hit.downloads || 0).toLocaleString()} downloads`;
        button.append(name, downloads);
        button.addEventListener("click", () => inspect(hit.id));
        item.append(button);
        results.append(item);
      }
      results.hidden = hits.length === 0;
      say(hits.length ? "Choose a repository." : "No GGUF repositories match.");
    } catch (error) {
      say(error.message);
    }
  });

  quantList.addEventListener("change", () => {
    add.disabled = !quantList.querySelector("input:checked");
  });

  add.addEventListener("click", async () => {
    const chosen = quantList.querySelector("input:checked");
    if (!repository || !chosen) return;
    add.disabled = true;
    say("Adding…");
    try {
      const data = await post("/api/catalog/local/add", { reference: repository, quant: chosen.value });
      say(`Added ${data.model}. Start it from Models; weights download and are verified then.`);
      document.querySelector("#refresh-models")?.click();
    } catch (error) {
      say(error.message);
      add.disabled = false;
    }
  });

  open.addEventListener("click", show);
  for (const button of sheet.querySelectorAll("[data-hf-close]")) button.addEventListener("click", hide);
  sheet.addEventListener("pointerdown", (event) => {
    if (event.target === sheet) hide();
  });
  sheet.addEventListener("keydown", (event) => {
    if (event.key === "Escape") hide();
  });
})();
