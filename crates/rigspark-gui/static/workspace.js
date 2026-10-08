// Workspace shell: theme, popovers, inspector, shortcuts and progressive disclosure.
// Loaded in <head> so the saved theme applies before first paint.
(() => {
  const root = document.documentElement;
  const THEME_KEY = "rigspark.theme";
  const INSPECTOR_KEY = "rigspark.inspector";
  const FACES = new Set(["yes", "slow", "no", "wink", "wow"]);

  const read = (key) => {
    try {
      return globalThis.localStorage.getItem(key);
    } catch {
      return null;
    }
  };
  const write = (key, value) => {
    try {
      globalThis.localStorage.setItem(key, value);
    } catch {
      // Best-effort persistence.
    }
  };

  root.dataset.theme = read(THEME_KEY) === "light" ? "light" : "dark";

  const isDark = () => root.dataset.theme !== "light";
  const face = (name) => {
    const safe = FACES.has(name) ? name : "yes";
    return `/static/brand/sparky-${safe}${isDark() ? "-dark" : ""}.svg`;
  };
  // Theme-aware Sparky image; callers set the mood via data-face.
  const faceImage = (name, className) => {
    const image = document.createElement("img");
    image.className = className;
    image.dataset.face = FACES.has(name) ? name : "yes";
    image.src = face(image.dataset.face);
    image.alt = "";
    image.decoding = "async";
    return image;
  };
  const setFace = (image, name) => {
    if (!image) return;
    image.dataset.face = FACES.has(name) ? name : "yes";
    image.src = face(image.dataset.face);
  };
  const refreshFaces = () => {
    for (const image of document.querySelectorAll("img[data-face]")) setFace(image, image.dataset.face);
    for (const mark of document.querySelectorAll("img[data-brand-mark]")) {
      mark.src = `/static/brand/${isDark() ? "mark-dark" : "mark"}.svg`;
    }
  };

  globalThis.RigSparkUI = Object.freeze({ face, faceImage, setFace });

  document.addEventListener("DOMContentLoaded", () => {
    const $ = (selector) => document.querySelector(selector);
    const $$ = (selector) => [...document.querySelectorAll(selector)];
    const app = $("#app");
    if (!app) return;

    refreshFaces();

    // --- Theme ---------------------------------------------------------------
    const themeToggle = $("#theme-toggle");
    const syncThemeToggle = () => {
      if (!themeToggle) return;
      themeToggle.setAttribute("aria-label", isDark() ? "Switch to light theme" : "Switch to dark theme");
    };
    syncThemeToggle();
    themeToggle?.addEventListener("click", () => {
      root.dataset.theme = isDark() ? "light" : "dark";
      write(THEME_KEY, root.dataset.theme);
      syncThemeToggle();
      refreshFaces();
      // Charts read colours from computed styles; redraw on the next resize tick.
      globalThis.dispatchEvent(new Event("resize"));
    });

    // --- Popovers -------------------------------------------------------------
    let openPop = null;
    let openAnchor = null;

    function place(pop, anchor, placement) {
      const rect = anchor.getBoundingClientRect();
      const margin = 12;
      pop.style.maxHeight = "";
      const width = pop.offsetWidth;
      const height = pop.offsetHeight;
      const left =
        placement.endsWith("start") ? rect.left : rect.right - width;
      pop.style.left = `${Math.max(margin, Math.min(innerWidth - width - margin, left))}px`;
      let top = placement.startsWith("top") ? rect.top - height - 8 : rect.bottom + 8;
      if (top < margin) top = Math.min(rect.bottom + 8, innerHeight - height - margin);
      if (top + height > innerHeight - margin) top = Math.max(margin, rect.top - height - 8);
      pop.style.top = `${Math.max(margin, top)}px`;
      pop.style.maxHeight = `${innerHeight - 2 * margin}px`;
    }

    function closePops(restoreFocus = false) {
      if (!openPop) return;
      openPop.hidden = true;
      openAnchor?.setAttribute("aria-expanded", "false");
      const anchor = openAnchor;
      openPop = null;
      openAnchor = null;
      if (restoreFocus) anchor?.focus();
    }

    function showPop(pop, anchor, placement, focusTarget) {
      if (openPop === pop && openAnchor === anchor) {
        closePops(true);
        return;
      }
      closePops();
      pop.hidden = false;
      place(pop, anchor, placement);
      anchor.setAttribute("aria-expanded", "true");
      openPop = pop;
      openAnchor = anchor;
      const target = (focusTarget && pop.querySelector(focusTarget)) ||
        pop.querySelector("select, input, button, [tabindex]:not([tabindex='-1'])");
      target?.focus();
    }

    const bindPop = (anchorSelector, popSelector, placement, focusTarget) => {
      const anchor = $(anchorSelector);
      const pop = $(popSelector);
      if (!anchor || !pop) return;
      anchor.addEventListener("click", (event) => {
        event.stopPropagation();
        showPop(pop, anchor, placement, focusTarget);
      });
    };
    bindPop("#model-chip", "#model-pop", "bottom-end", "#model-select");
    bindPop("#agent-chip", "#model-pop", "top-end", "#agent-select");
    bindPop("#composer-plus", "#composer-menu", "top-start");
    bindPop("#fit-settings", "#fit-pop", "bottom-end");

    document.addEventListener("pointerdown", (event) => {
      if (!openPop) return;
      if (openPop.contains(event.target) || openAnchor?.contains(event.target)) return;
      closePops();
    });
    globalThis.addEventListener("resize", () => closePops());
    $(".stage-body")?.addEventListener("scroll", () => closePops(), true);

    // Arrow keys move through menu items.
    $("#composer-menu")?.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      const items = $$("#composer-menu [role='menuitem']:not([disabled])");
      const index = items.indexOf(document.activeElement);
      const next = event.key === "ArrowDown" ? index + 1 : index - 1;
      items[(next + items.length) % items.length]?.focus();
      event.preventDefault();
    });

    // --- Inspector ------------------------------------------------------------
    const inspector = $("#inspector");
    const inspectorControls = [$("#inspector-toggle"), $("#buddy")].filter(Boolean);
    function setInspector(open) {
      app.classList.toggle("inspector-open", open);
      if (inspector) inspector.inert = !open;
      for (const control of inspectorControls) {
        control.setAttribute("aria-expanded", open ? "true" : "false");
      }
      $("#inspector-toggle")?.setAttribute("aria-label", open ? "Hide inspector" : "Show inspector");
      write(INSPECTOR_KEY, open ? "open" : "closed");
      globalThis.setTimeout(() => globalThis.dispatchEvent(new Event("resize")), 320);
    }
    setInspector(read(INSPECTOR_KEY) === "open");
    for (const control of inspectorControls) {
      control.addEventListener("click", () => setInspector(!app.classList.contains("inspector-open")));
    }

    // --- Keyboard shortcuts ---------------------------------------------------
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        if (openPop) {
          event.preventDefault();
          closePops(true);
          return;
        }
        if (sheet && !sheet.hidden) {
          event.preventDefault();
          closeSheet();
        }
        return;
      }
      if (!(event.metaKey || event.ctrlKey) || event.altKey || event.shiftKey) return;
      const view = $(`.rail-item[data-shortcut="${event.key}"]`);
      if (view) {
        event.preventDefault();
        closePops();
        view.click();
        return;
      }
      if (event.key === "i" || event.key === "I") {
        event.preventDefault();
        setInspector(!app.classList.contains("inspector-open"));
      }
    });
    for (const item of $$(".rail-item[data-view]")) {
      item.addEventListener("click", () => closePops());
    }

    // --- Recents search -------------------------------------------------------
    const sessionSearch = $("#session-search");
    const searchToggle = $("#session-search-toggle");
    const railSession = $(".rail-session");
    if (sessionSearch && searchToggle && railSession) {
      const syncSearch = () => {
        searchToggle.hidden = sessionSearch.hidden;
      };
      new MutationObserver(syncSearch).observe(sessionSearch, { attributes: true, attributeFilter: ["hidden"] });
      syncSearch();
      searchToggle.addEventListener("click", () => {
        const searching = !railSession.classList.contains("searching");
        railSession.classList.toggle("searching", searching);
        searchToggle.setAttribute("aria-expanded", searching ? "true" : "false");
        if (searching) sessionSearch.focus();
        else if (sessionSearch.value) {
          sessionSearch.value = "";
          sessionSearch.dispatchEvent(new Event("input", { bubbles: true }));
        }
      });
    }

    // --- Model chip, agent chip and composer footnote ------------------------
    const modelSelect = $("#model-select");
    const runtimeSelect = $("#runtime-select");
    const agentSelect = $("#agent-select");
    const modelCard = $("#model-card");
    let activeModel = null;

    const text = (selector) => ($(selector)?.textContent || "").trim();
    function syncChips() {
      const name = activeModel || modelSelect?.value || "No model";
      const chipName = $("#model-chip-name");
      if (chipName) chipName.textContent = name;
      const runtime = runtimeSelect?.value || text("#runtime-status");
      const chipRuntime = $("#model-chip-runtime");
      if (chipRuntime) chipRuntime.textContent = runtime;
      $("#model-chip-dot")?.setAttribute("data-state", activeModel ? "on" : "off");
      const agent = agentSelect?.selectedOptions?.[0]?.textContent?.trim() || "No agent";
      const agentName = $("#agent-chip-name");
      if (agentName) agentName.textContent = agent;
      $("#agent-chip")?.classList.toggle("active", Boolean(agentSelect?.value));
      const turns = Number.parseInt(text("#turn-count"), 10) || 0;
      const foot = $("#composer-foot");
      if (foot) {
        foot.textContent = `${name} · harness ${text("#backend-status") || "local"} · ${turns} turn${turns === 1 ? "" : "s"} · ${text("#context-usage") || "~0 tok"} context · on-device`;
      }
      const promptField = $("#prompt");
      if (promptField && promptField.placeholder.startsWith("Message ")) {
        promptField.placeholder = activeModel ? `Message ${activeModel}…` : "Message the local model…";
      }
    }
    globalThis.addEventListener("rigspark:text-model-active", (event) => {
      activeModel = event.detail?.modelId || null;
      for (const image of $$(".active-model-face")) setFace(image, activeModel ? "yes" : "slow");
      syncChips();
    });
    for (const control of [modelSelect, runtimeSelect, agentSelect]) {
      control?.addEventListener("change", syncChips);
      if (control) new MutationObserver(syncChips).observe(control, { childList: true });
    }
    for (const selector of ["#runtime-status", "#backend-status", "#turn-count", "#context-usage"]) {
      const node = $(selector);
      if (node) new MutationObserver(syncChips).observe(node, { childList: true, characterData: true, subtree: true });
    }
    if (modelCard) {
      new MutationObserver(() => {
        const value = modelCard.textContent.trim();
        if (value && value !== "No model running") activeModel = value;
        syncChips();
      }).observe(modelCard, { childList: true, characterData: true, subtree: true });
    }
    syncChips();

    // --- Composer: + menu, context, skills and system prompt -----------------
    const composer = $("#chat-form");
    const contextBar = $("#context-bar");
    const contextAdd = $("#context-add");
    const contextMenuItem = $("[data-composer-action='context']");
    const syncContext = () => {
      const available = Boolean(contextBar && !contextBar.hidden);
      if (contextAdd) contextAdd.hidden = !available;
      if (contextMenuItem) {
        contextMenuItem.disabled = !available;
        const note = $("#composer-context-note");
        if (note) note.textContent = available ? "Attach files, git state or pasted output" : "Unavailable in this session";
      }
    };
    if (contextBar) new MutationObserver(syncContext).observe(contextBar, { attributes: true, attributeFilter: ["hidden"] });
    syncContext();

    const skillChips = $("#skill-chips");
    const systemToggle = $("#system-prompt-toggle");
    const contextChips = $("#context-chips");
    const contextPicker = $("#context-picker");
    // The extras row (tokens, skills, panels) only takes space once something is in use.
    const syncExtras = () => {
      if (!composer) return;
      const show =
        composer.classList.contains("show-skills") ||
        composer.classList.contains("has-skills") ||
        Boolean(systemToggle?.classList.contains("active")) ||
        systemToggle?.getAttribute("aria-expanded") === "true" ||
        Boolean(contextChips?.children.length) ||
        Boolean(contextPicker && !contextPicker.hidden);
      composer.classList.toggle("has-extras", show);
    };
    const syncSkills = () => {
      const selected = Boolean(skillChips?.querySelector(".skill-chip.active"));
      composer?.classList.toggle("has-skills", selected);
      syncExtras();
    };
    if (skillChips) {
      new MutationObserver(syncSkills).observe(skillChips, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] });
    }
    if (systemToggle) {
      new MutationObserver(syncExtras).observe(systemToggle, { attributes: true, attributeFilter: ["class", "aria-expanded"] });
    }
    if (contextChips) new MutationObserver(syncExtras).observe(contextChips, { childList: true });
    if (contextPicker) new MutationObserver(syncExtras).observe(contextPicker, { attributes: true, attributeFilter: ["hidden"] });
    // Reveal the row before chat.js opens a panel and focuses inside it; observers run too late.
    document.addEventListener(
      "click",
      (event) => {
        if (event.target.closest?.("#context-add, #system-prompt-toggle")) composer?.classList.add("has-extras");
      },
      true,
    );
    syncSkills();

    $("#composer-menu")?.addEventListener("click", (event) => {
      const item = event.target.closest("[data-composer-action]");
      if (!item || item.disabled) return;
      const action = item.dataset.composerAction;
      closePops();
      if (action === "context") {
        contextAdd?.click();
      } else if (action === "skills") {
        composer?.classList.add("show-skills");
        syncExtras();
        const first = skillChips?.querySelector("button, .skill-chip");
        if (first && typeof first.focus === "function") first.focus();
      } else if (action === "system") {
        const panel = $("#system-prompt-panel");
        if (panel?.hidden) $("#system-prompt-toggle")?.click();
        else $("#system-prompt-input")?.focus();
      }
    });

    // --- Models: search and verdict filter -----------------------------------
    const recommended = $("#recommended-list");
    const modelSearch = $("#model-search");
    const filterEmpty = $("#model-filter-empty");
    let verdictFilter = "all";
    function applyModelFilters() {
      if (!recommended) return;
      const query = (modelSearch?.value || "").trim().toLowerCase();
      const cards = [...recommended.querySelectorAll(".model-card-item")];
      let shown = 0;
      for (const card of cards) {
        const title = (card.querySelector(".model-card-title")?.textContent || "").toLowerCase();
        const verdict = card.dataset.verdict || "";
        const visible = (!query || title.includes(query)) && (verdictFilter === "all" || verdict === verdictFilter);
        card.hidden = !visible;
        if (visible) shown += 1;
      }
      if (filterEmpty) filterEmpty.hidden = cards.length === 0 || shown > 0;
      if (modelSearch) {
        modelSearch.placeholder = cards.length ? `Search ${cards.length} model${cards.length === 1 ? "" : "s"}` : "Search models";
      }
    }
    modelSearch?.addEventListener("input", applyModelFilters);
    for (const button of $$("[data-verdict-filter]")) {
      button.addEventListener("click", () => {
        verdictFilter = button.dataset.verdictFilter;
        for (const other of $$("[data-verdict-filter]")) {
          other.setAttribute("aria-pressed", other === button ? "true" : "false");
        }
        applyModelFilters();
      });
    }
    if (recommended) new MutationObserver(applyModelFilters).observe(recommended, { childList: true });

    // --- Connectors: sheet, templates and empty state ------------------------
    const sheet = $("#connector-sheet");
    let sheetReturn = null;
    const TEMPLATES = {
      filesystem: { name: "filesystem", transport: "stdio", command: "npx", args: "-y @modelcontextprotocol/server-filesystem " },
      git: { name: "git", transport: "stdio", command: "uvx", args: "mcp-server-git --repository ." },
      custom: { name: "", transport: "stdio", command: "", args: "" },
    };
    function openSheet(template, opener) {
      if (!sheet) return;
      closePops();
      sheetReturn = opener || document.activeElement;
      const fill = TEMPLATES[template];
      if (fill) {
        const transport = $("#connector-transport");
        if (transport) {
          transport.value = fill.transport;
          transport.dispatchEvent(new Event("change", { bubbles: true }));
        }
        for (const [selector, value] of [["#connector-name", fill.name], ["#connector-command", fill.command], ["#connector-args", fill.args]]) {
          const field = $(selector);
          if (field) field.value = value;
        }
      }
      sheet.hidden = false;
      const focus = fill && fill.name ? (template === "filesystem" ? $("#connector-args") : $("#connector-name")) : $("#connector-name");
      focus?.focus();
      if (focus && template === "filesystem") focus.setSelectionRange(focus.value.length, focus.value.length);
    }
    function closeSheet() {
      if (!sheet || sheet.hidden) return;
      sheet.hidden = true;
      const target = sheetReturn;
      sheetReturn = null;
      if (target && typeof target.focus === "function") target.focus();
    }
    $("#connector-add")?.addEventListener("click", (event) => openSheet(null, event.currentTarget));
    for (const button of $$("[data-connector-template]")) {
      button.addEventListener("click", () => openSheet(button.dataset.connectorTemplate, button));
    }
    for (const button of $$("[data-sheet-close]")) button.addEventListener("click", closeSheet);
    sheet?.addEventListener("pointerdown", (event) => {
      if (event.target === sheet) closeSheet();
    });
    sheet?.addEventListener("keydown", (event) => {
      if (event.key !== "Tab") return;
      const focusable = [...sheet.querySelectorAll("input, select, button, textarea")].filter(
        (node) => !node.disabled && node.getClientRects().length > 0,
      );
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        last.focus();
        event.preventDefault();
      } else if (!event.shiftKey && document.activeElement === last) {
        first.focus();
        event.preventDefault();
      }
    });
    globalThis.addEventListener("rigspark:connector-added", closeSheet);

    const connectorList = $("#connector-list");
    const connectorEmpty = $("#connector-empty");
    if (connectorList && connectorEmpty) {
      const syncConnectors = () => {
        connectorEmpty.hidden = !connectorList.querySelector(".recommended-empty");
      };
      new MutationObserver(syncConnectors).observe(connectorList, { childList: true });
      syncConnectors();
    }

    // --- Library: tabs and starters ------------------------------------------
    const libraryGrid = $(".library-grid");
    const libraryTabs = $$("[data-library-tab]");
    function selectLibrary(tab, focus = false) {
      if (!libraryGrid) return;
      libraryGrid.dataset.libraryActive = tab;
      for (const button of libraryTabs) {
        const selected = button.dataset.libraryTab === tab;
        button.setAttribute("aria-selected", selected ? "true" : "false");
        button.tabIndex = selected ? 0 : -1;
        if (selected && focus) button.focus();
      }
    }
    for (const button of libraryTabs) {
      button.addEventListener("click", () => selectLibrary(button.dataset.libraryTab));
      button.addEventListener("keydown", (event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        const index = libraryTabs.indexOf(button);
        const next = libraryTabs[(index + (event.key === "ArrowRight" ? 1 : -1) + libraryTabs.length) % libraryTabs.length];
        selectLibrary(next.dataset.libraryTab, true);
      });
    }

    const STARTERS = {
      "agent:reviewer": {
        name: "Code reviewer",
        description: "Finds bugs and edge cases, suggests fixes.",
        body: "You are a meticulous code reviewer. Look for correctness bugs, unhandled edge cases, security issues and unclear naming. Quote the exact lines you mean, explain why each issue matters, and propose a concrete fix. Skip style nits unless they hide a bug.",
      },
      "agent:research": {
        name: "Research buddy",
        description: "Summarizes sources and keeps citations.",
        body: "You are a careful research assistant. Summarize the material you are given in plain language, keep every claim tied to its source, and say clearly when something is uncertain or missing. Never invent citations.",
      },
      "agent:editor": {
        name: "Writing editor",
        description: "Tightens prose and keeps your voice.",
        body: "You are a writing editor. Tighten the text, fix grammar and remove filler while keeping the author's voice and meaning. Return the edited text first, then a short list of the most important changes.",
      },
      "skill:commits": {
        name: "Commit messages",
        description: "Conventional commits from a diff.",
        body: "When asked for a commit message, write a Conventional Commits subject line under 72 characters (type(scope): summary), a blank line, then a short body explaining what changed and why. Do not describe unchanged code.",
      },
      "skill:explain": {
        name: "Explain like I'm new",
        description: "Plain-language explanations with an example.",
        body: "Explain concepts in plain language for someone new to the topic. Define any jargon the first time it appears, use one small concrete example, and end with a one-sentence summary.",
      },
      "skill:json": {
        name: "Strict JSON",
        description: "Answers as valid JSON only.",
        body: "Respond with a single valid JSON value and nothing else: no prose, no Markdown fences, no comments. If the request cannot be answered, return {\"error\": \"<short reason>\"}.",
      },
    };
    for (const button of $$("[data-starter]")) {
      button.addEventListener("click", () => {
        const starter = STARTERS[button.dataset.starter];
        if (!starter) return;
        const kind = button.dataset.starter.startsWith("agent:") ? "agent" : "skill";
        selectLibrary(kind === "agent" ? "agents" : "skills");
        $(`#${kind}-new`)?.click();
        const set = (selector, value) => {
          const field = $(selector);
          if (field) field.value = value;
        };
        set(`#${kind}-name`, starter.name);
        set(`#${kind}-desc`, starter.description);
        set(`#${kind}-body`, starter.body);
        $(`#${kind}-name`)?.focus();
      });
    }

    // --- Sparky status companion (real RAM pressure from telemetry) ----------
    const ring = $("#buddy-ring");
    const mood = $("#buddy-mood");
    const buddyFace = $("#buddy-face");
    const circumference = 2 * Math.PI * 15;
    if (ring) {
      ring.style.strokeDasharray = `${circumference}`;
      ring.style.strokeDashoffset = `${circumference}`;
    }
    globalThis.addEventListener("rigspark:telemetry", (event) => {
      const memory = event.detail?.memory;
      const known = typeof memory === "number" && Number.isFinite(memory);
      if (ring) ring.style.strokeDashoffset = `${circumference * (1 - (known ? Math.min(memory, 100) : 0) / 100)}`;
      const [name, label, state] = !known
        ? ["slow", "Metrics offline", "unknown"]
        : memory > 85
          ? ["no", "Memory is tight", "high"]
          : memory > 65
            ? ["slow", "Getting busy", "medium"]
            : ["yes", "All good", "low"];
      if (buddyFace?.dataset.face !== name) setFace(buddyFace, name);
      if (mood && mood.textContent !== label) mood.textContent = label;
      const buddy = $("#buddy");
      if (buddy) {
        buddy.dataset.pressure = state;
        buddy.setAttribute(
          "aria-label",
          `${label}${known ? ` · RAM ${memory.toFixed(0)}% used` : ""} · Loopback 127.0.0.1 · show inspector`,
        );
      }
    });
  });
})();
