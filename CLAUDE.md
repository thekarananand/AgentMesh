# AgentMesh — Claude Code terminal (Electron)

Electron app: tabbed xterm.js terminals, each backed by its own node-pty running its own `claude`. Sidebar lists Claude Code sessions across all projects on the machine (live + recent), sourced from the CLI's own local state — not a terminal wrapper, a mesh view over every running/past `claude` session, with click-to-attach.

## Files
- `main.js` — BrowserWindow config, per-tab node-pty spawn (`spawnTab`), pty env scrubbing, pid→tab binding (`parentMap`/`tabForPid`/`listAnnotated`), IPC wiring, session push to renderer
- `preload.js` — contextBridge, exposes `window.ptyAPI` (per-tab terminal IO, keyed by `tabId`) and `window.meshAPI` (sessions)
- `renderer.js` — tab manager, xterm.js setup + theme, cell measurement, fit logic, sidebar render, sidebar resizer
- `sessions.js` — reads `~/.claude/sessions/*.json` + `history.jsonl` + tails `~/.claude/projects/*/*.jsonl` for title/branch/cwd; pure Node, no Electron deps, unit-testable standalone
- `rename.js` — renames a session in sync with the CLI: control socket for live ones, transcript records for dead ones; pure Node
- `autoname.js` — promotes a session's AI title into its actual name so no session stays on its cwd-derived autoname
- `index.html` — shell page, sidebar + terminal split layout, welcome/folder-picker pane
- `titlebar.js` — shared `HEADER_HEIGHT` constant (main + preload both require it)

## Session sidebar — data sources
- `claude agents --json` / `~/.claude/sessions/<pid>.json` — **live** sessions only, most stable API. Has `sessionId`, `pid`, `status` (busy/idle), `cwd`, socket path. `sessions.js` reads the JSON files directly (skip shelling out) and validates the pid is still alive with `process.kill(pid, 0)` — the registry file can outlive the process on a crash.
- `~/.claude/history.jsonl` — flat log of every prompt ever, any project. Cheap source for first-prompt/last-prompt/prompt-count per sessionId. Grows forever — cache the parsed Map keyed on the file's `mtimeMs`, don't reparse every poll.
- `~/.claude/projects/<slug>/<sessionId>.jsonl` — transcripts. **Explicitly unstable format per Anthropic docs ("changes between versions")**, so every read is defensive (try/catch around JSON.parse per line, never throw). Only tail the last ~96KB (`tailRead`) — these files grow unbounded and we only want the trailing `ai-title`/`mode`/`cwd` records, not the whole conversation.
- Title priority: deliberate name (live registry `name` when `nameSource !== 'derived'`, else the transcript's `custom-title`/`agent-name`) > AI-generated `ai-title` record > first prompt from history > raw session id. See the rename section for why `nameSource` is tested that way.
- Transcript record types seen in the wild: `user`, `attachment`, `last-prompt`, `mode`, `permission-mode`, `ai-title`, `custom-title`, `agent-name`. Records are append-only and last-wins, so `scanTranscript` reads the tail **backwards** and takes the first hit of each kind.

## Session rename — the UDS control socket

Claude Code exposes an undocumented control channel on the per-session socket in
`~/.claude/sessions/<pid>.json` (`messagingSocketPath`, i.e. `/tmp/cc-socks/<pid>.sock`).
One newline-terminated JSON line renames a live session:

```json
{"type":"control","action":"rename","name":"my-name","session_id":"<uuid>"}
```

- `session_id` is optional in the protocol but **always send it** — Claude Code validates it and
  drops the message on mismatch. A pid file can outlive its process and the socket path is keyed
  on a pid the OS is free to reuse.
- There is **no ack**. A flushed write is the strongest signal available; confirmation arrives out
  of band when the CLI rewrites its pid file and `sessions.watch()` fires.
- One write updates everything at once: prompt-box banner, terminal title (OSC 0), the pid
  registry, and two transcript records (`custom-title` + `agent-name`). Same pipeline `/rename`
  (alias `/name`) uses.

`rename.js` picks the path: socket for live sessions, else append those same two records to the
transcript. Verified that the CLI's own `claude --resume` picker reads back an appended
`custom-title`, so the archived path is genuinely in sync, not a local shadow.

**`nameSource` semantics in the pid registry** — `'derived'` means the cwd-derived autoname
(`agentmesh-da`); the key is **absent entirely** once a name is set on purpose. `'user'` belongs
to the *daemon/background-job* record, a different store — testing for it against the pid registry
never matches.

Resume restores a custom title on its own, but the registry name is minted fresh at startup and
falls back to the derived autoname, so `spawnTab` passes `--name` alongside `--resume` when the
row carries a real name.

## Always-named sessions (`autoname.js`)

Claude Code has **no setting** to force session naming — checked the whole settings key space.
`/rename` with no argument generates a name, but only when a human types it, and the `ai-title`
record it generates on its own is never promoted to the session's *name*.

So AgentMesh promotes it: `pushSessions()` runs `autoName()`, which pushes a session's AI title
(or, before that exists, its first prompt) into the name over the control socket, kebab-cased to
match Claude Code's own generated-name style. Anything already deliberately named is never
touched, and each session is attempted once per app run so a failing rename can't retry-storm.
`SCOPE` is `'tabs'` (only sessions in our own tabs); `'all'` covers every live session on the
machine, including ones started in another terminal.

## Reading the CLI's internals

Everything undocumented in here was read out of the shipped binary, not guessed. The method,
because it will be needed again on the next version bump:

```bash
readlink -f "$(which claude)"        # /opt/homebrew/Caskroom/claude-code@latest/<ver>/claude
strings -n 6 <that path> > cc-strings.txt   # ~280MB Mach-O, bun-compiled; ~2 min, 420k lines
```

The bundled JS survives as a handful of enormous single lines, so `grep` is useless for reading
it — match a needle and print a window of surrounding characters instead (a ~15-line Node script
doing `indexOf` + `slice`). Log strings are the best entry points: they're distinctive, they sit
inside the function that implements the thing, and they survive minification when identifiers
don't. `[uds-messaging] Unhandled control action:` is what led to the whole rename protocol.

Anything found this way gets **verified against a live session** before it's built on — spawn a
throwaway `claude` in a pty, drive it, and read the files back. See the testing section.

## Cross-session messaging protocol (the mesh substrate)

The same socket that carries `control`/`rename` is a general inbox — this is the thing that makes
"mesh" more than a sidebar. Confirmed surface, protocol version `peerProtocol: 1`:

- **Transport**: unix socket, newline-delimited JSON, one message per line. Buffer over **1 MiB
  without a newline drops the connection**. Path is `$XDG_RUNTIME_DIR/cc-socks/<pid>.sock`,
  falling back to `/tmp/cc-socks-<uid>/<pid>.sock`; the whole path must stay under ~104 bytes.
- **`{"type":"user","message":{"role":"user","content":"..."}}`** — injects a prompt into the
  session's queue. Optional `priority` (`next` is the default, `now` jumps the queue), `from`,
  `msg_id`, `uuid`, `file_attachments`. The CLI's own log line documents it:
  `echo '{"type":"user",...}' | socat - UNIX-CONNECT:<sock>`.
- Injected user messages run with **`skipSlashCommands: true`** — you cannot drive `/rename`,
  `/compact` or any other slash command by injecting it as a prompt. Control actions are the only
  out-of-band commands.
- **`{"type":"control","action":"rename","name":"..."}`** — the rename above. Unknown actions are
  logged and ignored, so probing costs nothing.
- **`{"type":"control","action":"peer_message_status","status":"held|denied|expired|delivered",
  "orig_msg_id":"..."}`** — delivery receipts. Peer messages can be **held for the recipient's
  approval** (permission-mode parity), so cross-session sends are not guaranteed delivery.
- Senders are identified by peer credentials on the socket (uid check, pid + ancestry read), and
  a message carrying `session_id` is dropped unless it matches the receiving session.

## Local dev + testing

- Repo is git-backed as of this work; feature work goes on a branch, not `main`.
- **Never `pkill electron` to test.** The Claude Code session doing the work usually runs *inside*
  an AgentMesh tab, and `win.on('closed')` kills every pty — that kills the conversation. Launch a
  **second instance** instead (`nohup npm start > log 2>&1 &`); nothing in `main.js` takes a
  single-instance lock, and the two windows don't interfere. Kill only by the exact pid you
  launched.
- Protocol work is tested against real throwaway sessions, spawned through `node-pty` the same way
  `spawnTab` does (including the env scrubbing). Two gotchas: a fresh cwd triggers the **trust
  prompt**, so write `\r` a few seconds after spawn; and registration is asynchronous, so poll
  `~/.claude/sessions/` for the new pid file rather than sleeping a fixed amount.
- Clean up after: throwaway sessions leave transcripts under `~/.claude/projects/<slug>/` that
  otherwise show up in the sidebar's `RECENT` group.

## Other `~/.claude` state (not used yet)

- `daemon/` — `roster.json`, `control.key`, `dispatch/` for background agents.
- `jobs/`, `tasks/`, `plans/` — background job dirs, scheduled tasks, plan mode artifacts.
- `session-env/<sessionId>/` — one dir per session, empty in practice on this machine.
- `history.jsonl`, `projects/`, `sessions/` — the three the sidebar actually reads.

## Sidebar behavior
- Each tab owns a real pty running its own `claude` — tabs are not one TUI switched with `/resume`. That's what makes them independently interruptible and closable.
- Click a row → if it's already bound to an open tab, focus that tab; otherwise open a **new** tab running `claude --resume <sessionId>` in the session's own cwd.
- Binding is by **pid ancestry**: one `ps -Ao pid=,ppid=` per refresh, then walk the live session's pid up its parent chain until it hits a tab's shell pid (`tabForPid` in `main.js`). The `claude` process sits 1–3 hops above the pty shell, so the 24-hop cap is generous.
- `syncTabs` binds each open terminal to the session that registered inside it, which is what the inline rename and the row's close button act on.
- Row meta is **where and what**, not bookkeeping: project, branch, prompt count. No relative age, no pid — both were noise, and dropping age also removed the 30s repaint timer that existed only to keep it honest.
- Open rows carry a hover `×` that closes that terminal. It exists only on rows we host; everything else has no terminal to close.
- `RECENT` group is collapsible and **collapsed by default** — live agents are the point, history is on demand. `LIVE` never collapses.
- Right-click → reveals the session's cwd in Finder (`sessions:reveal`).
- Double-click a row (or a tab label) → inline rename. Because double-click renames, a click that
  would *spawn* a tab waits 220ms to see whether a second click is coming — otherwise every rename
  leaves a stray `claude --resume` behind. Focusing an already-open tab is idempotent, so that
  path stays instant.
- `render()` is suppressed while an inline edit is open (`editingSessionId`) and flushes one
  repaint on close — the 4s poll would otherwise rip the input out from under the cursor.
- Live sessions always sort first, then by `updatedAt` descending.
- `sessions.watch()` combines `fs.watch` (instant on most changes) with a 4s `setInterval` fallback — `fs.watch` misses status flips inside `sessions/*.json` on some macOS setups, poll covers the gap. Watch callback is debounced 250ms since writes land in bursts.
- Sizing runs against `#tabs-content`'s `clientWidth`, never `window.innerWidth` — the sidebar eats part of the window, and it's user-resizable (180–520px, persisted in `localStorage`), so no constant can stand in for it.

## Env hygiene for spawned ptys
If AgentMesh is itself launched from inside a `claude` session, that session's env leaks into every pty and makes each inner `claude` believe it's a child/subagent — which silently disables transcript persistence (`Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker`). `main.js` deletes `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_MESSAGING_SOCKET`, and `CLAUDE_CODE_ENTRYPOINT` from the pty env.

## Tabs
- `main.js` keeps `ptys: Map<tabId, ptyProcess>`; `tabId` is a `crypto.randomUUID()` minted at spawn. Every pty IPC message (`pty-input`/`pty-resize`/`pty-close`/`pty-data`/`pty-exit`) carries it.
- Switching tabs never touches a pty. Each tab's xterm host is `position: absolute; inset: 0` and toggles `visibility: hidden` — every shell keeps running with its own scrollback.
- **There is no tab strip.** The sidebar is the switcher — it already lists every session on the machine and marks the ones we host, so a second row of the same sessions across the top showed the user the same thing twice. `#drag-strip` is what's left: an empty 36px band that keeps the window draggable and clears the traffic lights.
- **Zero tabs is a legitimate state.** Launch shows `#welcome` (folder picker + recent folders pulled straight out of the session rows, so there's no second history to keep in sync) and spawns nothing. A session's cwd is fixed at spawn and decides which project it belongs to, so the folder is asked for up front rather than inherited from wherever the app was launched.

## The pty *is* Claude Code
`spawnTab` runs `$SHELL -l -c 'exec claude …'`, not a shell it then types `claude` into. Two consequences, both wanted:
- Quitting Claude Code ends the pty, so `pty-exit` closes the terminal instead of leaving the user at a bare prompt in a tab that no longer stands for anything. Last one closed → back to `#welcome`.
- The `claude` process is the pty process, so its parent is Electron directly. The `tabForPid` ancestry walk resolves in one hop now instead of three.

The login shell is still worth paying for — it's what puts `claude` and the user's toolchain on `PATH` — but `exec` replaces it rather than leaving it as a parent.
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
- **Everything else** (sidebar, welcome pane): `Inter`, with `-apple-system` fallback. Set on `body`; xterm overrides itself via its own `fontFamily` option, so the split needs no extra selectors.
- One-cell inline margin on each side of the terminal comes from `--cell-w`, measured in `renderer.js` with **canvas `measureText`, not DOM layout** — it has to be known before the first `term.open()`, since xterm sizes against the container box at open time. `fitAll()` subtracts `2 * CELL_W` from the available width when computing cols.

## Chrome vs glass
- Sidebar is **deliberately opaque** (`--chrome-bg: #1b1e24`), which kills vibrancy behind it. Only the terminal pane keeps the blur.
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
