const root = document.documentElement;
const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
const phone = matchMedia("(max-width: 560px)").matches;
root.classList.add("js");
if (reduceMotion || phone) root.classList.add("still");

// Copy buttons: the nearest [data-copy] holds the exact command.
document.querySelectorAll(".copy-btn").forEach((btn) => {
  const label = btn.getAttribute("aria-label");
  btn.addEventListener("click", async () => {
    const text = btn.closest("[data-copy]")?.getAttribute("data-copy") ?? "";
    try {
      await navigator.clipboard.writeText(text);
      btn.classList.add("done");
      btn.setAttribute("aria-label", "Copied");
      setTimeout(() => {
        btn.classList.remove("done");
        btn.setAttribute("aria-label", label);
      }, 1600);
    } catch {
      const code = btn.closest("[data-copy]")?.querySelector("code");
      if (code) getSelection()?.selectAllChildren(code);
    }
  });
});

// Mobile navigation
(() => {
  const toggle = document.querySelector(".nav-toggle");
  const links = document.getElementById("site-nav");
  if (!toggle || !links) return;
  const setOpen = (open) => {
    links.classList.toggle("is-open", open);
    toggle.setAttribute("aria-expanded", String(open));
    toggle.setAttribute("aria-label", open ? "Close navigation" : "Open navigation");
  };
  toggle.addEventListener("click", () => setOpen(toggle.getAttribute("aria-expanded") !== "true"));
  links.addEventListener("click", (event) => {
    if (event.target.closest("a")) setOpen(false);
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && links.classList.contains("is-open")) {
      setOpen(false);
      toggle.focus();
    }
  });
})();

// Highlight the download that matches this visitor and tailor the hero action.
(() => {
  const ua = navigator.userAgent.toLowerCase();
  const platform = (navigator.userAgentData?.platform || navigator.platform || "").toLowerCase();
  let os = null;
  let label = null;
  if (/iphone|ipad|ipod|android/.test(ua) || (navigator.maxTouchPoints > 1 && ua.includes("mac os"))) return;
  if (platform.includes("win") || ua.includes("windows")) [os, label] = ["win", "Windows"];
  else if (platform.includes("mac") || ua.includes("mac os")) [os, label] = ["mac-arm", "macOS"];
  else if (ua.includes("linux") && !ua.includes("android")) {
    [os, label] = [ua.includes("aarch64") || ua.includes("arm64") ? "linux-arm" : "linux-x64", "Linux"];
  }
  const card = os && document.querySelector(`.dl[data-os="${os}"]`);
  if (!card) return;
  card.classList.add("is-current");
  const hero = document.querySelector('.hero .pill[href="#install"]');
  if (hero) hero.textContent = `Download for ${label}`;
})();

// Respect reduced motion for autoplaying video.
if (reduceMotion) {
  document.querySelectorAll("video[autoplay]").forEach((video) => {
    video.removeAttribute("autoplay");
    video.pause();
  });
}

// Fade sections up as they scroll in.
(() => {
  const items = document.querySelectorAll(".reveal");
  if (reduceMotion || !("IntersectionObserver" in window)) {
    items.forEach((el) => el.classList.add("in"));
    return;
  }
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          entry.target.classList.add("in");
          observer.unobserve(entry.target);
        }
      }
    },
    { threshold: 0.15 },
  );
  items.forEach((el) => observer.observe(el));
})();

// Scroll-scrubbed 36-frame turntable with caption swaps.
(() => {
  const spin = document.getElementById("spin");
  const img = document.getElementById("spinimg");
  if (!spin || !img || root.classList.contains("still")) return;
  const count = 36;
  const frames = Array.from({ length: count }, (_, i) => `brand/3d/turntable/sparky-${String(i).padStart(2, "0")}.webp`);
  frames.forEach((src) => {
    new Image().src = src;
  });
  const captions = [...spin.querySelectorAll(".cap")];
  let last = 0;
  let queued = false;
  const update = () => {
    queued = false;
    const rect = spin.getBoundingClientRect();
    const total = Math.max(1, spin.offsetHeight - innerHeight);
    const progress = Math.min(1, Math.max(0, -rect.top / total));
    const frame = Math.min(count - 1, Math.floor(progress * count));
    if (frame !== last) {
      img.src = frames[frame];
      last = frame;
    }
    captions.forEach((cap) => cap.classList.toggle("on", progress >= Number(cap.dataset.from) && progress < Number(cap.dataset.to)));
  };
  addEventListener(
    "scroll",
    () => {
      if (!queued) {
        queued = true;
        requestAnimationFrame(update);
      }
    },
    { passive: true },
  );
  update();
})();

// Feature gallery arrows.
document.querySelectorAll(".arrows button").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.getElementById("scroller")?.scrollBy({ left: Number(btn.dataset.dir) * 392, behavior: reduceMotion ? "auto" : "smooth" });
  });
});

// Ask Sparky: a preview of RigSpark's fit and speed rules. Model sizes and hardware classes come
// from site/data/latest.js, generated by `cargo catalog-site` from the shipped catalog and perf data.
(() => {
  const gpuGroup = document.getElementById("gpu");
  const ramGroup = document.getElementById("ram");
  const vramGroup = document.getElementById("vram");
  const diskGroup = document.getElementById("disk");
  const popularGroup = document.getElementById("model");
  const newGroup = document.getElementById("model-new");
  const sourceGroup = document.getElementById("model-source");
  if (!gpuGroup || !ramGroup || !vramGroup || !diskGroup || !popularGroup || !newGroup || !sourceGroup) return;
  // Mirrors rigspark-core sizing: 2 GiB OS reserve outside VRAM, 15% headroom, slow below 10 tok/s.
  const OS_RESERVE_GIB = 2;
  const HEADROOM = 0.15;
  const SLOW_BELOW_TOK_PER_SEC = 10;
  const GIB = 1073741824;
  const data = globalThis.RIGSPARK_LATEST ?? {};
  const latest = data.models ?? [];
  const popular = new Map((data.popular ?? []).map((item) => [item.id, item]));
  const classes = data.hardware ?? [];
  const $ = (id) => document.getElementById(id);
  const pressed = (group) => group.querySelector('[aria-pressed="true"]');
  const kindName = { text: "text", image: "image", video: "video" };
  const graphics = {
    apple: { vendor: "apple", kind: "unified", memory: "ram", noun: "unified memory", note: "Unified memory on Apple Silicon." },
    nvidia: { vendor: "nvidia", kind: "discrete", memory: "vram", noun: "VRAM", note: "Video memory (VRAM) on the GPU. Models load into VRAM." },
    amd: { vendor: "amd", kind: "discrete", memory: "vram", noun: "VRAM", note: "Video memory (VRAM) on the GPU. Models load into VRAM." },
    cpu: { vendor: "none", kind: "cpu", memory: "ram", noun: "system memory", note: "System RAM. RigSpark sizes against free RAM; this preview assumes it is all free." },
  };
  let source = "popular";

  const fmt = (value) => (Math.round(value * 10) / 10).toString();

  const fillNew = (kind) => {
    newGroup.textContent = "";
    const items = latest.filter((item) => item.kind === kind);
    const empty = $("model-empty");
    empty.hidden = items.length > 0;
    empty.textContent = items.length ? "" : `No new ${kindName[kind]} models in the last month. Check back after the weekly catalog update.`;
    items.slice(0, 12).forEach((item, index) => {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.v = item.id;
      button.setAttribute("aria-pressed", String(index === 0));
      button.textContent = item.id;
      button.title = `${item.basis === "added" ? "added" : "released"} ${item.date}${item.runnable ? "" : " · workflow coming"}`;
      newGroup.appendChild(button);
    });
    return items.length;
  };

  const facts = (chip) => (source === "popular" ? popular.get(chip.dataset.v) : latest.find((item) => item.id === chip.dataset.v));

  const hardware = () => {
    const gpu = graphics[pressed(gpuGroup).dataset.v];
    const memory = Number(pressed(gpu.memory === "vram" ? vramGroup : ramGroup).dataset.v);
    const usable = gpu.memory === "vram" ? memory : Math.max(0, memory - OS_RESERVE_GIB);
    const bytes = memory * GIB;
    const perf = classes.find((item) => item.vendor === gpu.vendor && item.kind === gpu.kind && bytes >= item.minBytes && bytes < item.maxBytes);
    return { gpu, budget: usable * (1 - HEADROOM), disk: Number(pressed(diskGroup).dataset.v), perf };
  };

  // Same estimate as `rigspark can-run`: bandwidth × efficiency ÷ bytes read per token, ±30%.
  const speed = (model, perf) => {
    if (!perf || !model.decodeBytes) return null;
    const point = (perf.bandwidthGBps * 1e9 * perf.efficiency) / model.decodeBytes;
    return { low: point * 0.7, high: point * 1.3, mid: point, label: perf.label };
  };

  const judge = (model, hw) => {
    const generation = model.kind === "image" || model.kind === "video";
    const where = hw.gpu.noun;
    if (generation) {
      if (model.memGiB > hw.budget) {
        const offload = hw.gpu.memory === "vram" ? " ComfyUI may offload to system RAM; check with rigspark catalog --generation." : "";
        return ["no", "Won\u2019t fit.", `Its largest weight file needs ${model.memGiB} GiB, but only ${fmt(hw.budget)} GiB of ${where} fits the budget.${offload}`];
      }
      if (model.diskGiB > hw.disk) return ["no", "Won\u2019t download.", `It fits in ${where}, but the files need ${model.diskGiB} GiB and you have ${hw.disk} GB free.`];
      if (model.totalGiB <= hw.budget) return ["yes", "Runs well.", "All weights fit in memory together. Generation speed is not estimated."];
      return ["slow", "Fits, stage by stage.", "The largest file fits, so ComfyUI swaps models between stages. Generation speed is not estimated."];
    }
    if (model.memGiB > hw.budget) return ["no", "Won\u2019t fit.", `Needs ${model.memGiB} GiB, but only ${fmt(hw.budget)} GiB of ${where} fits the budget. Skip this download.`];
    if (model.diskGiB > hw.disk) return ["no", "Won\u2019t download.", `It fits in ${where}, but the download is ${model.diskGiB} GiB and you have ${hw.disk} GB free.`];
    const estimate = speed(model, hw.perf);
    if (!estimate) {
      const why = !model.decodeBytes ? "this quantization has no sourced speed data" : "there is no sourced speed data for this hardware";
      return ["slow", "Fits. Speed unknown.", `It fits in ${where} with headroom, but ${why}, so RigSpark reports speed as unknown rather than guessing.`];
    }
    if (estimate.mid < SLOW_BELOW_TOK_PER_SEC) {
      return ["slow", "Fits. Probably slowly.", `It fits in ${where}, but the speed estimate is under the ${SLOW_BELOW_TOK_PER_SEC} tok/s RigSpark calls comfortable.`];
    }
    return ["yes", "Runs well.", `Fits in ${where} with headroom. Go for it.`];
  };

  const render = () => {
    const group = source === "popular" ? popularGroup : newGroup;
    const chip = pressed(group);
    const model = chip && facts(chip);
    const result = document.querySelector("#ask .result");
    const tag = document.querySelector("#ask .verdict-tag");
    if (!model) {
      result.hidden = true;
      tag.hidden = true;
      return;
    }
    result.hidden = false;
    tag.hidden = false;
    const hw = hardware();
    const generation = model.kind === "image" || model.kind === "video";
    const [verdict, say, reason] = judge(model, hw);
    const runnable = !generation || model.runnable;
    const gauge = $("g");
    gauge.style.width = `${hw.budget > 0 ? Math.min((model.memGiB / hw.budget) * 100, 100) : 100}%`;
    gauge.className = `gauge-${verdict}`;
    $("need").textContent = `needs ${model.memGiB} GiB ${hw.gpu.memory === "vram" ? "VRAM" : "memory"}`;
    $("usable").textContent = `${fmt(hw.budget)} GiB budget`;
    $("download").textContent = `download ${model.diskGiB} GiB`;
    $("free").textContent = `${hw.disk} GB free`;
    const estimate = !generation && verdict !== "no" ? speed(model, hw.perf) : null;
    const speedLine = $("t-speed");
    speedLine.hidden = verdict === "no";
    speedLine.textContent = generation
      ? "Generation speed: unknown (not estimated)"
      : estimate
        ? `About ${fmt(estimate.low)}\u2013${fmt(estimate.high)} tok/s on ${estimate.label} (estimate for Ollama)`
        : "Speed: unknown";
    $("t-model").textContent = model.id;
    $("t-pill").textContent = verdict;
    $("t-exit").textContent = verdict === "no" ? "1" : "0";
    $("t-verdict").textContent = say;
    $("t-why").textContent = runnable ? reason : `${reason} Fit only for now: a built-in workflow is coming.`;
    $("tag-txt").textContent = verdict;
    $("tag-dot").className = `tag-${verdict}`;
    $("t-cmd").textContent = generation ? "rigspark catalog --generation" : "rigspark can-run";
    document.querySelectorAll(".viewer img").forEach((img) => img.classList.toggle("on", img.dataset.v === verdict));
  };

  const select = (group, btn) => {
    group.querySelectorAll("button").forEach((other) => other.setAttribute("aria-pressed", String(other === btn)));
    if (group === gpuGroup) {
      const gpu = graphics[btn.dataset.v];
      vramGroup.hidden = gpu.memory !== "vram";
      ramGroup.hidden = gpu.memory === "vram";
      $("ram-note").textContent = gpu.note;
    }
    render();
  };

  sourceGroup.addEventListener("click", (event) => {
    const btn = event.target.closest("button");
    if (!btn) return;
    sourceGroup.querySelectorAll("button").forEach((other) => other.setAttribute("aria-pressed", String(other === btn)));
    source = btn.dataset.v;
    const isPopular = source === "popular";
    popularGroup.hidden = !isPopular;
    newGroup.hidden = isPopular;
    $("model-note").textContent = isPopular
      ? "At the quantization rigspark up installs, default context."
      : `Released, or auto-added to the catalog, in the last ${data.windowDays ?? 31} days.`;
    if (isPopular) $("model-empty").hidden = true;
    else fillNew(source);
    render();
  });
  [gpuGroup, ramGroup, vramGroup, diskGroup, popularGroup, newGroup].forEach((group) => {
    group.addEventListener("click", (event) => {
      const btn = event.target.closest("button");
      if (btn) select(group, btn);
    });
  });
  render();
})();
