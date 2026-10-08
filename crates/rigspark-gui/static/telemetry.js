(() => {
  const panel = document.querySelector("#live-metrics");
  if (!panel) return;
  const state = panel.querySelector("#metrics-state");
  const interval = 2000;
  const windowMs = 120000;
  const series = ["memory", "cpu", "disk", "latency", "tokens", "cacheHits", "cacheMisses"].map(
    (name) => ({
      name,
      element: panel.querySelector(`[data-metric="${name}"]`),
      value: panel.querySelector(`#metric-${name}-value`),
      detail: panel.querySelector(`#metric-${name}-detail`),
      canvas: panel.querySelector(`#metric-${name}-chart`),
      points: [],
    }),
  );
  let timer;
  let pending;
  let disposed = false;
  for (const metric of series) metric.value.setAttribute("aria-live", "off");
  let latencyMaximum = 100;
  const valid = (value) => typeof value === "number" && Number.isFinite(value) && value >= 0;
  const counter = (value) => (Number.isSafeInteger(value) && value >= 0 ? value : null);
  const tokenMetric = (name) => ["tokens", "cacheHits", "cacheMisses"].includes(name);
  const number = (value) => value.toLocaleString("en-US");
  const bytes = (value) => {
    const unit = value >= 1024 ** 4 ? 1024 ** 4 : 1024 ** 3;
    return `${(value / unit).toFixed(1)} ${unit === 1024 ** 4 ? "TiB" : "GiB"}`;
  };
  function draw(metric, now) {
    const canvas = metric.canvas;
    const bounds = canvas.getBoundingClientRect();
    if (bounds.width === 0) return;
    const ratio = Math.min(globalThis.devicePixelRatio || 1, 2);
    canvas.width = Math.round(bounds.width * ratio);
    canvas.height = Math.round(bounds.height * ratio);
    const context = canvas.getContext("2d");
    if (!context) return;
    context.scale(ratio, ratio);
    const width = bounds.width;
    const height = bounds.height;
    const color = globalThis
      .getComputedStyle(metric.element)
      .getPropertyValue("--metric-color")
      .trim();
    const maximum =
      metric.name === "latency"
        ? latencyMaximum
        : tokenMetric(metric.name)
          ? Math.max(1, ...metric.points.map((point) => point.value ?? 0)) * 1.1
          : 100;
    context.strokeStyle = globalThis.getComputedStyle(panel).getPropertyValue("--stroke");
    context.lineWidth = 1;
    for (const level of [0.25, 0.75]) {
      context.beginPath();
      context.moveTo(0, height * level);
      context.lineTo(width, height * level);
      context.stroke();
    }
    context.strokeStyle = color;
    context.lineWidth = 1.75;
    context.lineJoin = "round";
    context.lineCap = "round";
    context.beginPath();
    let previous;
    for (const point of metric.points) {
      if (point.value === null) {
        previous = undefined;
        continue;
      }
      const horizontal = width * (1 - (now - point.at) / windowMs);
      const vertical = height - 3 - (height - 6) * Math.min(point.value / maximum, 1);
      if (previous === undefined || point.at - previous.at > interval * 2.5)
        context.moveTo(horizontal, vertical);
      else context.lineTo(horizontal, vertical);
      previous = point;
    }
    context.stroke();
    const latest = metric.points.at(-1);
    if (latest && latest.value !== null) {
      context.fillStyle = color;
      context.beginPath();
      context.arc(
        width - 2,
        height - 3 - (height - 6) * Math.min(latest.value / maximum, 1),
        2,
        0,
        Math.PI * 2,
      );
      context.fill();
    }
  }
  function update(sample, latency, now) {
    const usage = (used, total) =>
      valid(used) && valid(total) && total > 0 && used <= total ? (100 * used) / total : null;
    const inference = sample?.inferenceUsage;
    const input = counter(inference?.inputTokens);
    const output = counter(inference?.outputTokens);
    const hits = counter(inference?.cacheHitTokens);
    const misses = counter(inference?.cacheMissTokens);
    const readings = {
      memory: usage(sample?.memoryUsedBytes, sample?.memoryTotalBytes),
      cpu: valid(sample?.cpuPercent) && sample.cpuPercent <= 100 ? sample.cpuPercent : null,
      disk: usage(sample?.diskUsedBytes, sample?.diskTotalBytes),
      latency,
      tokens: input !== null && output !== null ? counter(input + output) : null,
      cacheHits: hits,
      cacheMisses: misses,
    };
    if (latency !== null) latencyMaximum = Math.max(latencyMaximum, Math.ceil(latency / 50) * 50);
    for (const metric of series) {
      const reading = readings[metric.name];
      metric.points.push({ at: now, value: reading });
      metric.points = metric.points.filter((point) => now - point.at <= windowMs).slice(-61);
      metric.value.textContent =
        reading === null
          ? "—"
          : tokenMetric(metric.name)
            ? number(reading)
            : `${reading.toFixed(metric.name === "latency" ? 0 : 1)}${metric.name === "latency" ? " ms" : "%"}`;
      if (metric.name === "tokens") {
        metric.detail.textContent =
          input === null && output === null
            ? "Not reported yet"
            : `${input === null ? "—" : number(input)} in / ${output === null ? "—" : number(output)} out`;
      } else if (metric.name === "cacheHits" || metric.name === "cacheMisses") {
        metric.detail.textContent =
          reading === null ? "Prompt tokens · not reported" : "Prompt tokens";
      }
      if (metric.name === "memory" || metric.name === "disk") {
        const prefix = metric.name === "memory" ? "memory" : "disk";
        metric.detail.textContent =
          reading === null
            ? "Unavailable"
            : `${bytes(sample[`${prefix}UsedBytes`])} / ${bytes(sample[`${prefix}TotalBytes`])}`;
      }
      metric.canvas.setAttribute(
        "aria-label",
        `${metric.name === "memory" ? "RAM" : metric.name} history, current ${metric.value.textContent === "—" ? "unavailable" : metric.value.textContent}`,
      );
      draw(metric, now);
    }
    // Lets the sidebar companion reflect real RAM pressure; null means unknown.
    if (typeof globalThis.dispatchEvent === "function" && typeof globalThis.CustomEvent === "function") {
      globalThis.dispatchEvent(new globalThis.CustomEvent("rigspark:telemetry", { detail: { memory: readings.memory } }));
    }
  }
  async function sample() {
    if (disposed || document.hidden || pending) return;
    const controller = new globalThis.AbortController();
    pending = controller;
    const timeout = globalThis.setTimeout(() => controller.abort(), 5000);
    const started = globalThis.performance.now();
    try {
      const response = await globalThis.fetch("/api/telemetry", {
        signal: controller.signal,
        cache: "no-store",
      });
      if (!response.ok) throw new Error("metrics unavailable");
      const result = await response.json();
      const latency = globalThis.performance.now() - started;
      if (!result || !valid(result.sampledAt) || Math.abs(Date.now() - result.sampledAt) > 15000)
        throw new Error("stale metrics");
      if (disposed || document.hidden) return;
      update(result, latency, Date.now());
      state.textContent = "Live";
      state.dataset.state = "live";
    } catch {
      if (disposed || document.hidden) return;
      update(null, null, Date.now());
      state.textContent = "Offline";
      state.dataset.state = "offline";
    } finally {
      globalThis.clearTimeout(timeout);
      pending = undefined;
      if (!disposed && !document.hidden) timer = globalThis.setTimeout(sample, interval);
    }
  }
  const observer = new globalThis.ResizeObserver(() =>
    series.forEach((metric) => draw(metric, Date.now())),
  );
  observer.observe(panel);
  document.addEventListener("visibilitychange", () => {
    globalThis.clearTimeout(timer);
    if (document.hidden) {
      pending?.abort();
      state.textContent = "Paused";
      state.dataset.state = "paused";
    } else void sample();
  });
  globalThis.addEventListener("pagehide", () => {
    disposed = true;
    globalThis.clearTimeout(timer);
    pending?.abort();
    observer.disconnect();
  });
  void sample();
})();
