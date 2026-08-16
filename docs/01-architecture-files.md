# Architecture — Files

Source lives under `src/`, split by process type: `src/main/`, `src/preload/`,
`src/renderer/`, and `src/lib/` for the pure-Node modules shared across
main/preload with no Electron dependency of their own.

## Main app entry points

- **`src/main/main.js`** — BrowserWindow config, per-tab node-pty spawn (`spawnTab`), pty env scrubbing, 
  pid→tab binding (`tabForPid`/`listAnnotated`, over the map `platform.parentMap()` returns), 
  IPC wiring, session push to renderer

- **`src/preload/preload.js`** — contextBridge, exposes:
  - `window.ptyAPI` — per-tab terminal IO, keyed by `tabId`
  - `window.meshAPI` — sessions, plan usage, CLI info, warnings
  - `window.platformAPI` — vibrancy and title-bar inset (the two facts the page can't work out)

- **`src/renderer/renderer.js`** — tab manager, xterm.js setup + theme, cell measurement, fit logic, 
  sidebar render, sidebar resizer

## Session and state

- **`src/lib/sessions.js`** — reads `~/.claude/sessions/*.json` + `history.jsonl` + tails 
  `~/.claude/projects/*/*.jsonl` for title/branch/cwd; pure Node, no Electron deps, 
  unit-testable standalone

- **`src/lib/rename.js`** — renames a session in sync with the CLI: control socket for live ones, 
  transcript records for dead ones; pure Node

- **`src/lib/autoname.js`** — promotes a session's AI title into its actual name so no session stays 
  on its cwd-derived autoname

- **`src/lib/usage.js`** — the account's plan windows (5h / weekly): credential read, 
  `/api/oauth/usage` fetch, normalizing, poll + backoff; pure Node, the OAuth token never leaves it

## System integration

- **`src/lib/platform.js`** — **every OS branch in the app**: window chrome, pid→ppid map, shell 
  selection + argument quoting, socket usability. Pure Node; nothing else may test 
  `process.platform`. See [21-platforms.md](21-platforms.md).

- **`src/lib/config.js`** — user settings (`settings.json` in the per-user data dir): `claudeBin`, 
  `autoname`, `usage`. Pure Node, type-checked on read, atomic write. 
  See [17-settings.md](17-settings.md).

- **`src/lib/claude.js`** — locates the CLI (config override → login-shell `command -v`), probes its 
  version, and is what `spawnTab` puts at the front of the pty command

- **`src/lib/debug.js`** — gates main-process debug logging behind `ELECTRON_ENABLE_LOGGING`, the 
  same flag that already forwards the renderer's own `console.log` to the terminal. One function, 
  `debugLog`, off by default.

## UI and styling

- **`src/renderer/index.html`** — shell page, `@font-face` for the bundled fonts, sidebar + terminal split 
  layout, welcome/folder-picker pane

- **`src/lib/titlebar.js`** — shared `HEADER_HEIGHT` constant (`preload.js` requires it directly; 
  `main.js` gets it transitively through `platform.js`). Used to reserve space for traffic lights on macOS.

## Assets

- **`assets/fonts/`** — bundled woff2 faces and their OFL license texts. Shipped, not installed.
  See [20-fonts.md](20-fonts.md).

- **`build/`** — packaging inputs: `icon.svg` (source art), `icon.png` (committed, what 
  electron-builder consumes), `entitlements.mac.plist`

- **`tools/`** — authoring scripts, never part of a build:
  - `build-fonts.js` (ttf→woff2)
  - `make-icon.js` (svg→png)
