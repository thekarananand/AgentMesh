# Architecture — Files

## Main app entry points

- **`main.js`** — BrowserWindow config, per-tab node-pty spawn (`spawnTab`), pty env scrubbing, 
  pid→tab binding (`tabForPid`/`listAnnotated`, over the map `platform.parentMap()` returns), 
  IPC wiring, session push to renderer

- **`preload.js`** — contextBridge, exposes:
  - `window.ptyAPI` — per-tab terminal IO, keyed by `tabId`
  - `window.meshAPI` — sessions, plan usage, CLI info, warnings
  - `window.platformAPI` — vibrancy and title-bar inset (the two facts the page can't work out)

- **`renderer.js`** — tab manager, xterm.js setup + theme, cell measurement, fit logic, 
  sidebar render, sidebar resizer

## Session and state

- **`sessions.js`** — reads `~/.claude/sessions/*.json` + `history.jsonl` + tails 
  `~/.claude/projects/*/*.jsonl` for title/branch/cwd; pure Node, no Electron deps, 
  unit-testable standalone

- **`rename.js`** — renames a session in sync with the CLI: control socket for live ones, 
  transcript records for dead ones; pure Node

- **`autoname.js`** — promotes a session's AI title into its actual name so no session stays 
  on its cwd-derived autoname

- **`usage.js`** — the account's plan windows (5h / weekly): credential read, 
  `/api/oauth/usage` fetch, normalizing, poll + backoff; pure Node, the OAuth token never leaves it

## System integration

- **`platform.js`** — **every OS branch in the app**: window chrome, pid→ppid map, shell 
  selection + argument quoting, socket usability. Pure Node; nothing else may test 
  `process.platform`. See [21-platforms.md](21-platforms.md).

- **`config.js`** — user settings (`settings.json` in the per-user data dir): `claudeBin`, 
  `autoname`, `usage`. Pure Node, type-checked on read, atomic write. 
  See [17-settings.md](17-settings.md).

- **`claude.js`** — locates the CLI (config override → login-shell `command -v`), probes its 
  version, and is what `spawnTab` puts at the front of the pty command

## UI and styling

- **`index.html`** — shell page, `@font-face` for the bundled fonts, sidebar + terminal split 
  layout, welcome/folder-picker pane

- **`titlebar.js`** — shared `HEADER_HEIGHT` constant (main + preload both require it). 
  Used to reserve space for traffic lights on macOS.

## Assets

- **`assets/fonts/`** — bundled woff2 faces and their OFL license texts. Shipped, not installed.
  See [20-fonts.md](20-fonts.md).

- **`build/`** — packaging inputs: `icon.svg` (source art), `icon.png` (committed, what 
  electron-builder consumes), `entitlements.mac.plist`

- **`tools/`** — authoring scripts, never part of a build:
  - `build-fonts.js` (ttf→woff2)
  - `make-icon.js` (svg→png)
