// Engines RigSpark installs and runs itself (ComfyUI for pictures and video, mlx-audio
// for voice). One click to set up, then everything starts on its own.
(() => {
  "use strict";

  const token = () => document.querySelector('meta[name="llmup-token"]')?.content || "";
  const pending = new Map();

  async function status(name) {
    const response = await globalThis.fetch("/api/generation/engines");
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || `request failed (${response.status})`);
    return (data.engines || []).find((engine) => engine.engine === name) || null;
  }

  function wait(ms) {
    return new Promise((resolve) => globalThis.setTimeout(resolve, ms));
  }

  // Resolves true once the engine is ready, false if the person chose not to set it up.
  async function ensure(name, { onStep = () => {}, ask = true } = {}) {
    if (pending.has(name)) return pending.get(name);
    const run = (async () => {
      let view = await status(name);
      if (!view) throw new Error(`${name} is not an engine RigSpark manages`);
      if (view.state === "ready") return true;
      if (view.state === "unsupported") throw new Error(view.detail || `${view.label} doesn't run on this machine`);
      if (view.state !== "installing") {
        if (ask) {
          const confirmed = await globalThis.rigsparkConfirm({
            title: `Set up ${view.label}`,
            message: view.installs,
            confirmLabel: "Set up",
          });
          if (!confirmed) return false;
        }
        const response = await globalThis.fetch(`/api/generation/engines/${encodeURIComponent(name)}/install`, {
          method: "POST",
          headers: { "X-LLMUP-Token": token() },
        });
        const data = await response.json().catch(() => ({}));
        // 409 means an install is already running: follow it.
        if (!response.ok && response.status !== 409) {
          throw new Error(data.error || `request failed (${response.status})`);
        }
      }
      for (;;) {
        await wait(1000);
        view = await status(name);
        if (!view) throw new Error(`${name} disappeared`);
        if (view.state === "ready") {
          onStep(`${view.label} is ready`);
          return true;
        }
        if (view.state === "failed") throw new Error(view.detail || `${view.label} setup failed`);
        const [step, of] = view.step || [];
        onStep(`${view.detail || `Setting up ${view.label}`}${of ? ` (${step} of ${of})` : ""}…`);
      }
    })();
    pending.set(name, run);
    try {
      return await run;
    } finally {
      pending.delete(name);
    }
  }

  globalThis.rigsparkEngines = { status, ensure };
})();
