use fantoccini::Client;
use serde_json::{Value, json};
use std::error::Error;

const RUN_REDUCER: &str = include_str!("../../static/run-reducer.js");
const SSE: &str = include_str!("../../static/sse.js");
const MARKDOWN: &str = include_str!("../../static/markdown.js");
const CALCULATOR: &str = include_str!("../../static/calculator-template.js");
const TELEMETRY: &str = include_str!("../../static/telemetry.js");

// Unit assertions for the shipped client modules, run in the real browser instead of Node.
const SUITE: &str = r####"
const [reducerSource, sseSource, markdownSource, calculatorSource, telemetrySource, done] = arguments;
const failures = [];
const check = (name, condition, detail = "") => { if (!condition) failures.push(`${name}: ${detail}`); };
const same = (name, actual, expected) => {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) failures.push(`${name}: expected ${e}, got ${a}`);
};
const flush = async () => { for (let i = 0; i < 50; i += 1) await Promise.resolve(); };

function reducerSuite() {
  const r = globalThis.GuiRunReducer;
  const initial = r.initialRunState();
  same("reducer initial phase", initial.phase, "idle");
  const next = r.reduceRun(initial, { type: "submit", prompt: "hi" });
  same("reducer does not mutate input", initial.phase, "idle");
  check("reducer returns a new state", next !== initial);

  let state = r.reduceRun(r.initialRunState(), { type: "submit", prompt: "hi" });
  same("submit -> sending", [state.phase, state.prompt], ["sending", "hi"]);
  state = r.reduceRun(state, { type: "stream-event", event: { type: "delta", content: "A" } });
  same("delta -> running", [state.phase, state.reply], ["running", "A"]);
  state = r.reduceRun(state, { type: "stream-event", event: { type: "delta", content: "B" } });
  same("deltas accumulate", state.reply, "AB");
  state = r.reduceRun(state, { type: "stream-event", event: { type: "done" } });
  same("done -> completed", [state.phase, state.reply], ["completed", "AB"]);

  state = r.reduceRun(r.initialRunState(), { type: "submit", prompt: "hi" });
  state = r.reduceRun(state, { type: "stream-event", event: { type: "delta", content: "half" } });
  state = r.reduceRun(state, { type: "stream-event", event: { type: "error", message: "backend down" } });
  same("failure keeps partial reply", [state.phase, state.reply, state.error], ["failed", "half", "backend down"]);

  state = r.reduceRun(r.initialRunState(), { type: "submit", prompt: "hi" });
  state = r.reduceRun(state, { type: "stream-event", event: { type: "delta", content: "partial" } });
  state = r.reduceRun(state, { type: "request-stop" });
  same("request-stop -> stopping", state.phase, "stopping");
  state = r.reduceRun(state, { type: "cancelled" });
  same("cancelled", state.phase, "cancelled");
  const late = r.reduceRun(state, { type: "stream-event", event: { type: "delta", content: " late" } });
  same("late delta keeps cancelled phase", [late.phase, late.reply], ["cancelled", "partial late"]);

  state = r.reduceRun(r.initialRunState(), { type: "submit", prompt: "hi" });
  state = r.reduceRun(state, { type: "stream-event", event: { type: "tool", name: "search", phase: "start" } });
  same("tool events are phase-neutral", state.phase, "sending");

  state = r.reduceRun(r.initialRunState(), { type: "submit", prompt: "hi" });
  state = r.reduceRun(state, { type: "stream-error", message: "connection lost" });
  same("transport failure", [state.phase, state.error], ["failed", "connection lost"]);

  const idle = r.initialRunState();
  check("stop on idle is ignored", r.reduceRun(idle, { type: "request-stop" }) === idle);
  check("idle is inactive", r.isActive(idle) === false);
  const completed = r.reduceRun(r.reduceRun(idle, { type: "submit", prompt: "x" }), { type: "stream-event", event: { type: "done" } });
  same("stop on terminal is ignored", r.reduceRun(completed, { type: "request-stop" }).phase, "completed");
}

function sseSuite() {
  const { SseFrameBuffer } = globalThis.GuiSse;
  const encode = (events) => new TextEncoder().encode(events.map((event) => `data: ${JSON.stringify(event)}\n\n`).join(""));
  const events = (results) => results.filter((result) => result.error === undefined).map((result) => result.event);
  const split = (name, source) => {
    const bytes = encode(source);
    for (let cut = 1; cut < bytes.length; cut += 1) {
      const buffer = new SseFrameBuffer();
      const parsed = [...events(buffer.push(bytes.slice(0, cut))), ...events(buffer.push(bytes.slice(cut))), ...events(buffer.flush())];
      if (JSON.stringify(parsed) !== JSON.stringify(source)) { same(`${name} at byte ${cut}`, parsed, source); return; }
    }
  };

  const whole = [{ type: "delta", content: "a" }, { type: "done", turnsAppended: 1 }];
  const buffer = new SseFrameBuffer();
  same("sse whole frames", events(buffer.push(encode(whole))).concat(events(buffer.flush())), whole);
  split("sse split frames", [
    { type: "delta", content: "hello" },
    { type: "tool", name: "search", phase: "start" },
    { type: "delta", content: "world" },
    { type: "done", turnsAppended: 1 },
  ]);
  split("sse split UTF-8", [{ type: "delta", content: "café → 🚀 汉字" }]);

  const bytewise = [{ type: "delta", content: "🚀" }, { type: "delta", content: "汉字" }, { type: "done", turnsAppended: 2 }];
  const single = new SseFrameBuffer();
  const collected = [];
  for (const byte of encode(bytewise)) collected.push(...events(single.push(Uint8Array.of(byte))));
  collected.push(...events(single.flush()));
  same("sse one byte at a time", collected, bytewise);

  const malformed = new SseFrameBuffer().push(new TextEncoder().encode('data: {not json}\n\ndata: {"type":"done"}\n\n'));
  same("sse malformed payload", [malformed[0]?.error, malformed[1]?.event], ["malformed SSE payload", { type: "done" }]);
  const comments = new SseFrameBuffer().push(new TextEncoder().encode(': heartbeat\n\ndata: {"type":"delta","content":"x"}\n\n'));
  same("sse ignores comments", events(comments), [{ type: "delta", content: "x" }]);
}

function markdownSuite() {
  const m = globalThis.GuiMarkdown;
  same("https link", m.safeLinkHref("https://example.com/docs?q=local"), "https://example.com/docs?q=local");
  same("http loopback link", m.safeLinkHref("http://127.0.0.1:4000/docs"), "http://127.0.0.1:4000/docs");
  for (const value of ["javascript:alert(1)", "data:text/html,<script>alert(1)</script>", "/relative"]) same(`rejected link ${value}`, m.safeLinkHref(value), null);
  same("relative image", m.safeImageSrc("chart.png"), "/api/images/chart.png");
  same("api image", m.safeImageSrc("/api/images/chart.svg"), "/api/images/chart.svg");
  same("inline image", m.safeImageSrc("data:image/png;base64,AAAA"), "data:image/png;base64,AAAA");
  for (const value of ["https://example.com/tracker.png", "file:///tmp/image.png", "../secret.png"]) same(`rejected image ${value}`, m.safeImageSrc(value), null);

  const classes = new Map();
  const container = { textContent: "", innerHTML: "", classList: { toggle: (name, force) => classes.set(name, force === true) } };
  const source = "# Safe\n<script>alert(1)</script>";
  same("fallback without parser globals", [m.renderAssistantMarkdown(container, source), container.textContent, classes.get("markdown-fallback")], [false, source, true]);
  same("escapeHtml", m.escapeHtml(`<button title="x">'unsafe' & more</button>`), "&lt;button title=&quot;x&quot;&gt;&#39;unsafe&#39; &amp; more&lt;/button&gt;");

  const scheduler = () => {
    const frames = [];
    const renders = [];
    const instance = m.createRenderScheduler((_target, text, streaming) => renders.push({ source: text, streaming }), {
      scheduleFrame: (callback) => { frames.push(callback); return frames.length - 1; },
      cancelFrame: (handle) => { frames[handle] = undefined; },
    });
    return { frames, renders, instance };
  };
  const batched = scheduler();
  const target = {};
  for (let index = 1; index <= 1000; index += 1) batched.instance.update(target, "x".repeat(index));
  same("1,000 updates schedule one frame", [batched.frames.length, batched.renders.length], [1, 0]);
  batched.frames[0]?.();
  same("frame renders latest source", batched.renders, [{ source: "x".repeat(1000), streaming: true }]);
  const finalized = scheduler();
  finalized.instance.update(target, "```ts\npartial");
  finalized.instance.finalize(target, "```ts\ncomplete\n```");
  finalized.frames[0]?.();
  same("finalize cancels the pending frame", finalized.renders, [{ source: "```ts\ncomplete\n```", streaming: false }]);
}

function calculatorSuite() {
  const proposal = globalThis.GuiCalculatorTemplate.createCalculatorProposal("workspace-1");
  const operation = proposal.operations[0];
  same("calculator proposal", [proposal.workspaceId, proposal.operations.length, operation?.op, operation?.path], ["workspace-1", 1, "create", "index.html"]);
  check("calculator is accessible", operation?.text.includes('aria-label="Calculator"'));
  check("calculator evaluates", operation?.text.includes('data-action="evaluate"'));
}

function telemetryFixture({ events: withEvents = true } = {}) {
  let now = 1_700_000_000_000;
  let handles = 0;
  const timers = new Map();
  const clock = {
    setTimeout: (callback, delay) => { handles += 1; timers.set(handles, { at: now + (delay || 0), callback }); return handles; },
    clearTimeout: (handle) => { timers.delete(handle); },
    async advance(ms) {
      const target = now + ms;
      await flush();
      for (;;) {
        let due;
        for (const entry of timers) if (entry[1].at <= target && (!due || entry[1].at < due[1].at)) due = entry;
        if (!due) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].callback();
        await flush();
      }
      now = target;
      await flush();
    },
  };
  const drawing = { arcs: 0, scale() {}, beginPath() {}, moveTo() {}, lineTo() {}, stroke() {}, fill() {}, arc() { this.arcs += 1; } };
  const elements = new Map();
  const panel = {
    querySelector(selector) {
      if (!elements.has(selector)) elements.set(selector, { textContent: "", dataset: {}, setAttribute() {}, getBoundingClientRect: () => ({ width: 200, height: 42 }), getContext: () => drawing });
      return elements.get(selector);
    },
  };
  const events = new Map();
  const document = { hidden: false, querySelector: () => panel, addEventListener: (name, handler) => events.set(name, handler) };
  const payload = { cpuPercent: 25, memoryUsedBytes: 8 * 1024 ** 3, memoryTotalBytes: 16 * 1024 ** 3, diskUsedBytes: 100 * 1024 ** 3, diskTotalBytes: 200 * 1024 ** 3 };
  const overrides = [];
  const fetch = { calls: 0 };
  let ticks = 0;
  let disconnects = 0;
  const dispatched = [];
  const scope = {
    AbortController,
    ...(withEvents
      ? {
          CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init?.detail; } },
          dispatchEvent: (event) => { dispatched.push(event); return true; },
        }
      : {}),
    fetch: async () => {
      fetch.calls += 1;
      const override = overrides.shift();
      if (override) return override();
      return { ok: true, json: async () => ({ ...payload, sampledAt: now }) };
    },
    performance: { now: () => (ticks += 10) },
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    addEventListener: (name, handler) => events.set(name, handler),
    getComputedStyle: () => ({ getPropertyValue: () => "#77b8ed" }),
    ResizeObserver: class { observe() {} disconnect() { disconnects += 1; } },
  };
  new Function("document", "globalThis", "Date", telemetrySource)(document, scope, { now: () => now });
  const text = (selector) => elements.get(selector)?.textContent;
  return { clock, drawing, document, events, fetch, overrides, payload, text, elements, dispatched, now: () => now, disconnects: () => disconnects };
}

async function telemetrySuite() {
  let t = telemetryFixture();
  await t.clock.advance(0);
  same("tokens start unreported", t.text("#metric-tokens-value"), "—");
  t.overrides.push(async () => ({ ok: true, json: async () => ({ ...t.payload, sampledAt: t.now(), inferenceUsage: { inputTokens: 1000, outputTokens: 200, cacheHitTokens: 0, cacheMissTokens: 1000 } }) }));
  await t.clock.advance(2000);
  same("token readings", ["#metric-tokens-value", "#metric-tokens-detail", "#metric-cacheHits-value", "#metric-cacheMisses-value"].map(t.text), ["1,200", "1,000 in / 200 out", "0", "1,000"]);
  await t.clock.advance(2000);
  same("stale token counts clear", [t.text("#metric-cacheHits-value"), t.text("#metric-cacheHits-detail")], ["—", "Prompt tokens · not reported"]);

  t = telemetryFixture();
  await t.clock.advance(0);
  same("system readings", ["#metric-memory-value", "#metric-memory-detail", "#metric-cpu-value", "#metric-latency-value"].map(t.text), ["50.0%", "8.0 GiB / 16.0 GiB", "25.0%", "10 ms"]);
  same("gauge markers drawn", t.drawing.arcs, 4);
  await t.clock.advance(2000);
  same("bounded polling interval", t.fetch.calls, 2);

  t = telemetryFixture();
  await t.clock.advance(0);
  t.overrides.push(async () => { throw new Error("offline"); });
  await t.clock.advance(2000);
  same("failure clears readings", [t.elements.get("#metrics-state")?.dataset.state, t.text("#metric-cpu-value")], ["offline", "—"]);
  same("memory pressure is announced, unknown when offline", t.dispatched.map((event) => [event.type, event.detail?.memory]), [["rigspark:telemetry", 50], ["rigspark:telemetry", null]]);
  await t.clock.advance(2000);
  same("next sample recovers", t.elements.get("#metrics-state")?.dataset.state, "live");

  t = telemetryFixture({ events: false });
  await t.clock.advance(0);
  same("hosts without events still render readings", t.text("#metric-memory-value"), "50.0%");

  t = telemetryFixture();
  await t.clock.advance(0);
  t.document.hidden = true;
  t.events.get("visibilitychange")?.();
  await t.clock.advance(10000);
  same("hidden page pauses polling", t.fetch.calls, 1);
  t.document.hidden = false;
  t.events.get("visibilitychange")?.();
  await t.clock.advance(0);
  same("visible page resumes", t.fetch.calls, 2);
  t.events.get("pagehide")?.();
  await t.clock.advance(10000);
  same("pagehide stops polling and releases the observer", [t.fetch.calls, t.disconnects()], [2, 1]);
}

(async () => {
  try {
    for (const source of [reducerSource, sseSource, markdownSource, calculatorSource]) (0, eval)(source);
    reducerSuite();
    sseSuite();
    markdownSuite();
    calculatorSuite();
    await telemetrySuite();
  } catch (error) {
    failures.push(`suite threw: ${error?.stack ?? error}`);
  }
  done(failures);
})();
"####;

pub async fn run(client: &Client) -> Result<(), Box<dyn Error>> {
    client.goto("about:blank").await?;
    let result = client
        .execute_async(
            SUITE,
            vec![
                json!(RUN_REDUCER),
                json!(SSE),
                json!(MARKDOWN),
                json!(CALCULATOR),
                json!(TELEMETRY),
            ],
        )
        .await?;
    match result {
        Value::Array(failures) if failures.is_empty() => Ok(()),
        Value::Array(failures) => Err(format!(
            "client module assertions failed:\n{}",
            failures
                .iter()
                .map(|failure| failure.as_str().unwrap_or("non-string failure"))
                .collect::<Vec<_>>()
                .join("\n")
        )
        .into()),
        other => Err(format!("client module suite returned {other}").into()),
    }
}
