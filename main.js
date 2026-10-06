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

// Ask Sparky: a preview of RigSpark's memory-fit rule using published Q4_K_M sizes.
(() => {
  const ramGroup = document.getElementById("ram");
  const modelGroup = document.getElementById("model");
  if (!ramGroup || !modelGroup) return;
  const OS_RESERVE_GIB = 2;
  const HEADROOM = 0.15;
  const $ = (id) => document.getElementById(id);
  const pressed = (group) => group.querySelector('[aria-pressed="true"]');
  const copy = {
    yes: ["Runs well.", () => "Fits in memory with headroom. Go for it.", "0"],
    slow: ["Fits. Probably slowly.", (m) => `A dense ${m.params}B model is usually bandwidth-bound on laptop memory. Run can-run for your real tok/s.`, "0"],
    no: ["Won\u2019t fit.", (m, budget) => `Needs ${m.mem} GiB, but only ${budget.toFixed(1)} GiB fits the budget. Skip this download.`, "1"],
  };
  const render = () => {
    const ram = Number(pressed(ramGroup).dataset.v);
    const chip = pressed(modelGroup);
    const model = { id: chip.dataset.v, mem: Number(chip.dataset.mem), params: Number(chip.dataset.params), dense: chip.dataset.dense === "1" };
    const budget = Math.max(0, ram - OS_RESERVE_GIB) * (1 - HEADROOM);
    const verdict = model.mem > budget ? "no" : model.dense && model.params >= 27 ? "slow" : "yes";
    const [say, why, exit] = copy[verdict];
    const gauge = $("g");
    gauge.style.width = `${Math.min((model.mem / budget) * 100, 100)}%`;
    gauge.className = `gauge-${verdict}`;
    $("need").textContent = `needs ${model.mem} GiB`;
    $("usable").textContent = `${budget.toFixed(1)} GiB budget`;
    $("t-model").textContent = model.id;
    $("t-pill").textContent = verdict;
    $("t-exit").textContent = exit;
    $("t-verdict").textContent = say;
    $("t-why").textContent = why(model, budget);
    $("tag-txt").textContent = verdict;
    $("tag-dot").className = `tag-${verdict}`;
    document.querySelectorAll(".viewer img").forEach((img) => img.classList.toggle("on", img.dataset.v === verdict));
  };
  const select = (group, btn) => {
    group.querySelectorAll("button").forEach((other) => other.setAttribute("aria-pressed", String(other === btn)));
    render();
  };
  [ramGroup, modelGroup].forEach((group) => {
    group.addEventListener("click", (event) => {
      const btn = event.target.closest("button");
      if (btn) select(group, btn);
    });
  });
  render();
})();
