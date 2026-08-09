# AgentMesh — Claude Code terminal (Electron)

Electron app: tabbed xterm.js terminals, each backed by its own node-pty running its own `claude`. Sidebar lists Claude Code sessions across all projects on the machine (live + recent), sourced from the CLI's own local state — not a terminal wrapper, a mesh view over every running/past `claude` session, with click-to-attach.

## Files
- `main.js` — BrowserWindow config, per-tab node-pty spawn (`spawnTab`), pty env scrubbing, pid→tab binding (`parentMap`/`tabForPid`/`listAnnotated`), IPC wiring, session push to renderer
- `preload.js` — contextBridge, exposes `window.ptyAPI` (per-tab terminal IO, keyed by `tabId`) and `window.meshAPI` (sessions)
- `renderer.js` — tab manager, xterm.js setup + theme, cell measurement, fit logic, sidebar render, sidebar resizer
- `sessions.js` — reads `~/.claude/sessions/*.json` + `history.jsonl` + tails `~/.claude/projects/*/*.jsonl` for title/branch/cwd; pure Node, no Electron deps, unit-testable standalone
- `index.html` — shell page, sidebar + terminal split layout
- `titlebar.js` — shared `HEADER_HEIGHT` constant (main + preload both require it)

## Session sidebar — data sources (see full research in conversation, not repeated here)
- `claude agents --json` / `~/.claude/sessions/<pid>.json` — **live** sessions only, most stable API. Has `sessionId`, `pid`, `status` (busy/idle), `cwd`, socket path. `sessions.js` reads the JSON files directly (skip shelling out) and validates the pid is still alive with `process.kill(pid, 0)` — the registry file can outlive the process on a crash.
- `~/.claude/history.jsonl` — flat log of every prompt ever, any project. Cheap source for first-prompt/last-prompt/prompt-count per sessionId. Grows forever — cache the parsed Map keyed on the file's `mtimeMs`, don't reparse every poll.
- `~/.claude/projects/<slug>/<sessionId>.jsonl` — transcripts. **Explicitly unstable format per Anthropic docs ("changes between versions")**, so every read is defensive (try/catch around JSON.parse per line, never throw). Only tail the last ~96KB (`tailRead`) — these files grow unbounded and we only want the trailing `ai-title`/`mode`/`cwd` records, not the whole conversation.
- Title priority: custom `--name`/`/rename` (`nameSource: 'user'` in the live registry) > AI-generated `ai-title` record > first prompt from history > raw session id.

## Sidebar behavior
- Each tab owns a real pty running its own `claude` — tabs are not one TUI switched with `/resume`. That's what makes them independently interruptible and closable.
- Click a row → if it's already bound to an open tab, focus that tab; otherwise open a **new** tab running `claude --resume <sessionId>` in the session's own cwd.
- Binding is by **pid ancestry**: one `ps -Ao pid=,ppid=` per refresh, then walk the live session's pid up its parent chain until it hits a tab's shell pid (`tabForPid` in `main.js`). The `claude` process sits 1–3 hops above the pty shell, so the 24-hop cap is generous.
- Bound tabs get relabelled from session metadata (`syncTabLabels`), so the tab strip shows real titles instead of `session 3`.
- `RECENT` group is collapsible and **collapsed by default** — live agents are the point, history is on demand. `LIVE` never collapses.
- Right-click → reveals the session's cwd in Finder (`sessions:reveal`).
- Live sessions always sort first, then by `updatedAt` descending.
- `sessions.watch()` combines `fs.watch` (instant on most changes) with a 4s `setInterval` fallback — `fs.watch` misses status flips inside `sessions/*.json` on some macOS setups, poll covers the gap. Watch callback is debounced 250ms since writes land in bursts.
- Sizing runs against `#tabs-content`'s `clientWidth`, never `window.innerWidth` — the sidebar eats part of the window, and it's user-resizable (180–520px, persisted in `localStorage`), so no constant can stand in for it.

## Env hygiene for spawned ptys
If AgentMesh is itself launched from inside a `claude` session, that session's env leaks into every pty and makes each inner `claude` believe it's a child/subagent — which silently disables transcript persistence (`Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker`). `main.js` deletes `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_MESSAGING_SOCKET`, and `CLAUDE_CODE_ENTRYPOINT` from the pty env.

## Tabs
- `main.js` keeps `ptys: Map<tabId, ptyProcess>`; `tabId` is a `crypto.randomUUID()` minted at spawn. Every pty IPC message (`pty-input`/`pty-resize`/`pty-close`/`pty-data`/`pty-exit`) carries it.
- Switching tabs never touches a pty. Each tab's xterm host is `position: absolute; inset: 0` and toggles `visibility: hidden` — every shell keeps running with its own scrollback.
- Closing the last tab immediately opens a fresh one; zero-tab state is never allowed.
- Tab spawn/exit/close all call `pushSessions()` so pid→tab binding re-resolves without waiting on the 4s poll.
- `fitAll()` resizes **every** tab, not just the active one — a hidden tab still has a live pty whose `cols`/`rows` must track the window, or its output wraps wrong the moment you switch to it.

## Keys
- `Shift+Enter` and `Ctrl+Enter` insert a newline instead of submitting: the renderer intercepts both in `attachCustomKeyEventHandler` and writes `\x1b\r` (ESC+CR) to the pty. xterm sends a bare CR for these by default, which is "send"; ESC+CR reads as alt/option-Enter, which Claude Code's input treats as a line break natively — no `/terminal-setup` or terminal-side keybinding needed.
- **`e.preventDefault()` in that handler is load-bearing.** xterm's `_keyDown` clears `_keyDownHandled` *before* consulting the custom handler and returns early without suppressing the browser default, so `_keyPress` still fires and emits charCode 13 — a bare CR landing right after our ESC+CR. Claude Code then sees newline-then-submit and the prompt sends anyway. Killing the default keydown stops Chromium dispatching that keypress at all. The handler also returns `false` for `e.type === 'keypress'` as a guard for engines that dispatch it regardless.

## External dependencies (Homebrew)

Not in `package.json` — these are machine-level installs the app assumes exist. Fresh machine setup:

```bash
brew install --cask font-jetbrains-mono-nerd-font   # terminal font
brew install --cask font-inter                      # all non-terminal UI chrome
```

`claude` (the Claude Code CLI) must also be on `PATH` — the app spawns it per tab and reads its local state under `~/.claude`. Everything else (`electron`, `node-pty`, `xterm`) comes from `npm install`.

Both fonts degrade rather than break: xterm falls back to generic `monospace`, the UI chain falls back to `-apple-system`. Layout still works, it just looks wrong.

## Fonts
- **Terminal**: `JetBrainsMono Nerd Font Mono`. Pick the `Mono` variant, not `Propo` — fixed-width glyphs align in monospace cells. Settings: fontSize 13, lineHeight 1 (flush, needed for clean ASCII art), letterSpacing 0.
- **Everything else** (sidebar, tab bar): `Inter`, with `-apple-system` fallback. Set on `body`; xterm overrides itself via its own `fontFamily` option, so the split needs no extra selectors.
- One-cell inline margin on each side of the terminal comes from `--cell-w`, measured in `renderer.js` with **canvas `measureText`, not DOM layout** — it has to be known before the first `term.open()`, since xterm sizes against the container box at open time. `fitAll()` subtracts `2 * CELL_W` from the available width when computing cols.

## Chrome vs glass
- Sidebar and tab bar are **deliberately opaque** (`--chrome-bg: #1b1e24`), which kills vibrancy behind them. Only the terminal pane keeps the blur. Both read the same CSS var — change one token, not two rules.
- The sidebar resizer is a 6px absolutely-positioned strip straddling the sidebar's right edge (`right: -3px`). It **must** carry `-webkit-app-region: no-drag`, otherwise dragging it drags the whole window instead.
- While dragging, `body.resizing` sets `pointer-events: none` on the terminal pane so the pointer can't get swallowed by the xterm canvas mid-drag.
- Terminals refit **at drop**, not per `mousemove` — a `term.resize()` per mouse event thrashes xterm's renderer and the pty.
- Width clamps to 180–520px, persists in `localStorage` under `sidebarWidth`, double-click resets to 248.

## Theme — One Dark Pro Glass
Colors pulled from `/Users/thekarananand/Library/Application Support/Zed/extensions/installed/one-dark-pro-max/themes/one-dark-pro-glass.json`. Full ANSI 16-color palette lives in `renderer.js` theme object.

## Real OS blur (vibrancy) — gotchas found the hard way

1. **`transparent: true` needs `visualEffectState: 'active'`.** Without it, macOS vibrancy (NSVisualEffectView) only renders the blur while the window is key/focused — screenshot tools and quick alt-tabs steal focus mid-check and make it look completely broken/opaque. This wasted the most time; always test focused, or just force `active`.
2. **Semi-opaque content kills the blur.** Chromium's compositor optimizes a mostly-opaque layer (e.g. `rgba(x,x,x,0.5+)`) into a fully opaque one for perf, which defeats vibrancy. Confirmed via isolated test windows — `about:blank` (no content) showed real blur; any HTML with alpha ≳0.5 flattened to solid.
3. **Don't double-tint.** Applying the same alpha color to both `html,body` background AND xterm's `theme.background` stacks two alpha layers only where the terminal canvas covers the div — the leftover rounding-gap strip (rows don't divide window height evenly) only gets one layer, so it looks visibly lighter than the terminal. Fix: tint at exactly one layer (`html,body`), set xterm `theme.background` to fully transparent (`#08090900`) so the canvas only paints glyphs, not a second background fill.
4. Match Zed's own approach: their theme JSON sets `"terminal.background": "#08090900"` — zero alpha, relying purely on window-level vibrancy for the tint. We do the same.

## titleBarStyle: 'hiddenInset' gotchas

- Traffic lights float over content with no reserved space by default — content gets drawn under them unless you push it down.
- Don't guess a pixel offset. Use `trafficLightPosition: { x, y }` in the `BrowserWindow` constructor to set the lights' position from a `HEADER_HEIGHT` constant *you* define — then pad the content by that same constant. Single source of truth in `titlebar.js`, shared by `main.js` (positions the lights) and `renderer.js` via preload (pads content + row-count math).
- Set that padding **before** calling `term.open()`. xterm measures/lays out against the container's box at open time; changing padding after open causes a double-offset.
- `-webkit-app-region: drag` goes on the padding/header strip only, with `no-drag` on the xterm element inside it — otherwise the whole terminal becomes undraggable-but-also-unclickable-correctly, or the whole window drags on any click.

## Preload + sandbox

Electron sandboxes preload scripts by default. A sandboxed preload **cannot** `require()` arbitrary local project files (like `./titlebar.js`) — only a small set of built-ins. Symptom: `Error: module not found: ./titlebar` plus a silent blank window (renderer throws before any UI paints). Fix: `sandbox: false` in `webPreferences` (contextIsolation stays on, still safe for this local-only app).

## macOS version note
Dev machine runs macOS 26 "Tahoe" (Liquid Glass redesign) — worth remembering if vibrancy/window-chrome behavior seems to differ from older Electron+macOS guides found online.
