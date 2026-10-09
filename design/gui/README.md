# RigSpark Workspace: minimal GUI redesign

- **Prototype:** `design/gui/app.html`. Open it in a browser; it works offline and supports light and dark themes.
- **Figma:** page **App — Workspace (GUI)** at https://www.figma.com/design/FrHXEqYrQlnAK0q1MfZjzG. It has Chat (dark, inspector open), Models (dark) and Runtime (light). The Color variable collection now has **Light and Dark modes**; each screen frame pins its mode.
- **Screens:** `design/gui/screens/*.png`, with every view in dark, key views in light, and the popover, sheet and chat-reply states.

## Principle: minimal, nothing removed, everything shown only when it's needed

The current GUI shows every control on every page: four dropdowns, Refresh, a harness strip and a full hardware/metrics rail, around 25 always-visible controls. The redesign keeps **every feature** but uses progressive disclosure: each control appears in the place and at the moment you need it.

| Current GUI (always visible) | Redesign: where it lives now | Shown when |
|---|---|---|
| Model / Harness / Runtime / Agent dropdowns + Refresh (every page) | One **model chip** in the toolbar (`● qwen3:30b-a3b  ollama ▾`), opening a popover with all four selectors and Refresh | You click the chip. The agent can also be picked from the composer |
| Status strip: harness, runtime, endpoint, turns, context (every page) | Inside the model-chip popover (endpoint, turns, context), plus a one-line footnote under the chat composer | In the chat footnote, or in the popover |
| Right rail: active model, hardware, live RAM/CPU/Disk/Latency sparklines, latest call (tokens, cache hits/misses) | **Inspector** panel, hidden by default. Toggle it from the toolbar, with ⌘I, or by clicking Sparky in the sidebar | On demand |
| (none) | **Sparky status companion** in the sidebar footer: a RAM ring, a mood face (All good / Getting busy / Memory is tight) and the loopback address | Always: one glanceable cue instead of a full rail |
| Sidebar nav, session search, new chat, saved chats | Nav with ⌘1–⌘5, a **Recents** list, search and + icons on the Recents header | Always (small) |
| Chat composer: System prompt, Skills, Add context buttons, Text/Image/Video tabs, Send | Composer with a **+ menu** (Add context · Use a skill · System prompt) that adds removable tokens, a Text/Image/Video segmented control, an agent chip and a circular Send | The + menu opens on click; tokens appear only when used |
| Chat empty state + 4 suggestion cards | 3D Sparky + "Ready when you are." + 4 quiet suggestion pills that pre-fill the prompt | Empty chat only |
| Models: Text/Image/Video tabs, defaults line | Segmented Text/Image/Video in the header; defaults moved into the catalog footer | Always (small) |
| Models: Source, Context window, KV cache, Known-context-fit-only, Bypass estimated fit, Refresh | **Fit settings** popover | You click Fit settings |
| Models: running banner (long runtime model id) | Amber "is running" banner with **Stop**. The internal runtime model id is left out; it can go in the inspector | While a model runs |
| Models: cards with verdict pill, mono metadata, backend select, Start | A **list** with a Sparky face per verdict, a fit bar, tok/s, context and fit, Start / Running. The backend picker (Auto / ollama / llama.cpp / mlx) appears **on row hover**. Search plus All / Runs well / Slow / Won't fit filters | Always; the backend picker on hover |
| Models: catalog version + Update catalog | Footer line with an "Update catalog" link | Always (small) |
| Connectors: form always open (name, transport, command, args) | Empty state + **templates** (Filesystem, Git, Custom) → an **Add connector sheet** pre-filled from the template | After you pick a template or click Add connector |
| Connectors: Config JSON editor + Reload / Apply | **"Edit config as JSON"** disclosure with the same text, Reload and Apply | Expanded on demand |
| Library: Agents and Skills side by side, dashed empty boxes | Agents/Skills segmented control + New button + Sparky empty state + **starter templates** (labelled "Starter") | Always |
| Runtime: Machine card + runtime toggles | A System Settings-style grouped list (**This Mac**, **Inference runtimes**) with status dots, ports, toggles, and an *Install guide* link for missing runtimes. An **AI Hardware Score** card sits on top | Always |

## Visual language (shared with the v3 site)

- **Colour:** Apple-style neutrals with Sparky amber as the only accent. Dark is the default (`#1C1C1E` window, `#262628` sidebar), and light is one click away.
- **Type and controls:** Inter at 13px for UI, JetBrains Mono for ids, ports and numbers. Controls are 30px tall with 8px radii; cards use 12px radii; popovers and sheets use 14–16px radii with soft shadows.
- **Mascot:** Sparky appears four ways:
  - the sidebar status companion (its mood follows RAM pressure)
  - a face on every model row (happy = runs well, sleepy = slow, X-eyes = won't fit)
  - the chat avatar
  - 3D renders in empty states (Chat, Connectors) and on the Hardware Score card
- **Dark-theme faces:** `site/brand/illustrations/sparky-*-dark.svg` give Sparky a lighter body so he reads on dark surfaces. The prototype swaps them with the theme.

## Prototype notes (what's fake)

- **Chat replies:** canned and streamed locally, labelled "demo". Wire this to the existing chat endpoint.
- **Live metrics:** random-walk demo data. Wire this to the existing metrics feed (same four series).
- **Model list:** the six models from your screenshot, plus `llama3.3:70b` to show the "won't fit" state.
- **AI Hardware Score (78):** illustrative. Source it from `rigspark doctor`.
- **Library starters:** placeholders.

## Implementation pointers

- **Tokens:** the CSS custom properties at the top of `app.html` (`--bg`, `--side`, `--surface`, `--raised`, `--line`, `--text*`, `--amber*`, verdict colours) map 1:1 onto `site/brand/tokens/tokens.css`, under a `[data-theme=dark]` override.
- **Shortcuts:** ⌘1–⌘5 switch views, ⌘I toggles the inspector, Esc closes popovers and sheets, ↩ sends and ⇧↩ adds a newline.
- **Popovers:** one positioning helper (`openPop`) for the model chip, the + menu, Fit settings and the agent chip.
- **Responsive:** below 980px the sidebar collapses to icons and the model list drops the fit and tok/s columns.
