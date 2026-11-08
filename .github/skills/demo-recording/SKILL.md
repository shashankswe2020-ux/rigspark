# Skill: Demo Recording

Record terminal demos and screenshots of rigspark using **vhs** (charm.sh
terminal recorder). Produces reproducible GIFs and PNGs from `.tape` files.

---

## When to Use

- Before a release — refresh demo GIF and screenshots to match current UI
- After TUI changes — re-record affected screenshots
- When adding new commands — create tape files for new screenshots

---

## Prerequisites

- `vhs` installed (`brew install vhs`)
- The native `rigspark`, `llmup` and `rigspark-gui` binaries for the release on
  `PATH` (build them with `cargo build --release --locked -p rigspark-cli -p rigspark-gui`
  from the tagged commit, then prefix `PATH` with `target/release`)
- Tape files live in `assets/*.tape`
- Output images go to `assets/` (GIF for demos, PNG for screenshots); copy the
  refreshed files that the website (`site/index.html` in the rigspark-web repository) uses into its `site/assets/`

---

## Tape File Conventions

```tape
# Every tape must have an Output directive (required for Screenshot to work)
Output assets/<name>-out.gif

Set Shell "zsh"
Set Theme "Catppuccin Mocha"
Set FontSize 14
Set Width 1200
Set Height <appropriate-height>   # 700 for short, 1100+ for full TUI
Set Padding 20

Require rigspark

# Record against an empty, throwaway state home.
Hide
Type "export RIGSPARK_HOME=$(mktemp -d) && clear"
Enter
Show
```

### Key Rules

1. **Output directive is mandatory** — without it, `Screenshot` silently fails
2. **Never pipe commands** — piping disables the TUI (vhs provides a real PTY)
3. **Use `Sleep`** generously after commands to let the TUI fully render
4. **Screenshot path** — `Screenshot assets/screenshot-<name>.png`
5. **Quit TUI** — send `Type "q"` after screenshot to exit cleanly
6. **Never end on `Screenshot`** — add a `Sleep` after it, or vhs exits before writing the file
7. **Quote absolute paths** — `Screenshot "/tmp/x.png"`; unquoted absolute paths fail to parse

---

## Standard Tapes

| Tape | Purpose | Height |
|------|---------|--------|
| `assets/demo.tape` | Full end-to-end GIF (version → recommend with details and compare → doctor → can-run → plan) | 700 |
| `assets/recommend.tape` | Screenshot of recommend TUI | 1100 |
| `assets/doctor.tape` | Screenshot of doctor TUI | 700 |
| `assets/can-run.tape` | Screenshot of can-run verdicts (`--no-tui`) | 300 |

---

## Recording Workflow

1. **Verify the binaries** — ensure the release build is first on `PATH`:
   ```bash
   export PATH="$PWD/target/release:$PATH"
   rigspark --version
   ```

2. **Record all tapes**:
   ```bash
   vhs assets/demo.tape
   vhs assets/recommend.tape
   vhs assets/doctor.tape
   vhs assets/can-run.tape
   ```

3. **Inspect output** — visually confirm screenshots are complete (not cut off)

4. **Commit assets**:
   ```bash
   git add assets/*.tape assets/*.gif assets/*.png
   git commit -m "docs: refresh demo and screenshots for v<version>"
   ```

---

## Troubleshooting

| Problem | Fix |
|---------|-----|
| Screenshot is blank/missing | Add `Output assets/<name>-out.gif`; don't end the tape on `Screenshot`; re-record if a frame caught the TUI mid-redraw |
| TUI not rendering (plain text) | Remove any `\|` pipes from commands |
| Content cut off at bottom | Increase `Set Height` (try 1100+) |
| Content cut off at top | TUI is taller than terminal; increase height |
| Command not found | Put the release `target/release` directory first on `PATH` |
| Old version shown | Rebuild from the tagged commit before recording |

---

## Cleanup

- The `-out.gif` files produced by tapes that only need a Screenshot can be
  gitignored or deleted — only the PNG screenshots matter for those tapes
- Add `assets/*-out.gif` to `.gitignore` if they're not used in README
