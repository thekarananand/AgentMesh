# AgentMesh — Claude Code terminal (Electron)

Electron app: tabbed xterm.js terminals, each backed by its own node-pty running its own `claude`. Sidebar lists Claude Code sessions across all projects on the machine (live + recent), sourced from the CLI's own local state — not a terminal wrapper, a mesh view over every running/past `claude` session, with click-to-attach.

## Intent — read this before proposing work

Three goals at once, in this priority order. A change that serves one but breaks another is not a win.

1. **Orchestration control plane for many agents.** The point is running several Claude Code
   agents in parallel — across different projects, not just tabs on one repo — and being able to
   see all of them, tell them apart, and eventually route work *between* them. The peer-message
   socket documented below is the substrate for that, which is why it's mapped out in far more
   detail than anything currently calls for. Terminal tabs are the plumbing, not the product.
2. **A better daily driver than Terminal.app for Claude Code.** Whatever the orchestration story
   becomes, this is the app the user actually lives in all day. Latency, key handling, fonts,
   vibrancy and window chrome are product surface, not polish — a control plane nobody wants to
   sit inside doesn't get used.
3. **Shippable to other developers, not a personal hack.** Assume a fresh machine and someone
   else's `~/.claude`. No hardcoded paths to this user's home, no assumption a given session,
   project or CLI version exists. Everything read out of the CLI is version-fragile by nature, so
   degrade to a working app with less information rather than throwing.

**Portability is a standing requirement, not a milestone that was reached once.** Every change
must keep the app installable and runnable on a machine that is not this one: a different user,
a different home directory, a different OS, a fresh `~/.claude`. Concretely, and enforced on
every new piece of work:

- **No dependency the app doesn't carry or check for.** If something must exist on the machine,
  it is either bundled in the repo (fonts, icon), resolved at runtime with a real failure
  message (the `claude` binary — see `claude.js`), or written down in the table below. A
  hand-installed Homebrew cask that nothing verifies is exactly the bug this rule exists to
  prevent: it worked here and silently produced a wrong app everywhere else.
- **Document every dependency you add**, in `README.md` (what a user must install) and in the
  table below (what the app assumes at runtime, and what happens when it's missing). "It's
  obvious" and "it's already on my machine" are how the last set of these got missed.
- **Every OS branch goes in `platform.js`**, never inline. A `process.platform` check in
  `main.js` or `renderer.js` is a bug in the making — the second one always gets forgotten.
- **User-specific behavior belongs in `config.js`**, not a constant. Anything that writes to
  someone else's `~/.claude`, or talks to the network, needs an off switch.

### Runtime dependencies — the complete list

| Dependency | How it's satisfied | If missing |
|---|---|---|
| Claude Code CLI (`claude`) | Resolved at startup via login shell, or `claudeBin` in settings | New-session controls disable, welcome pane and sidebar say why |
| Fonts (JetBrains Mono Nerd Font, Inter) | **Bundled** in `assets/fonts` as woff2, `@font-face` in `index.html` | n/a — cannot be missing |
| App icon | **Bundled** as committed `build/icon.png` | n/a |
| `node-pty` native binary | `npm install` postinstall runs `electron-builder install-app-deps` | Build fails loudly |
| Process list (`ps` / PowerShell CIM) | `platform.parentMap()` | Rows stop binding to tabs; a warning names it |
| Per-session control socket | Path comes from the CLI's own pid registry | Rename falls back to transcript records |
| macOS Keychain / `.credentials.json` | `usage.js`, platform-branched | Usage footer hides |

Authoring-only tools (`npm run fonts`, `npm run icon`) may depend on whatever is convenient —
their outputs are committed, so no one else ever runs them.

Much of the work is filling gaps the CLI leaves — always-named sessions, real attach for bg
agents, hiding phantom prewarm spares, cross-project visibility. That's the *source* of features,
not the goal itself; a CLI gap is worth closing when it serves one of the three above.

### Decided — do not relitigate

- **Never fork, patch or vendor the `claude` binary.** Read its local state and speak its
  protocols. Reading strings out of the shipped binary to *learn* the protocol is fine and
  expected (see below); shipping a modified one is not.
- **No direct mutation of `~/.claude` state files.** Writes go over the control socket, or append
  records in the exact format the CLI itself writes (`custom-title`/`agent-name`) so its own
  `--resume` picker reads them back. Never rewrite or hand-edit a pid registry, transcript or
  `history.jsonl` in place.
- **Local, single-user, no server.** No remote machines, no daemon of our own, no auth flow of our
  own. Everything is this machine's filesystem and unix sockets, with exactly one exception:
  a read-only `GET /api/oauth/usage` against the user's own Anthropic account, using the OAuth
  token the CLI already stored — the same call `/usage` makes (see the plan-usage section). Nothing
  is ever POSTed, no credential is written, no third party is contacted. Adding a *second* network
  call is a new decision, not a precedent already set.
- **Undocumented behavior must be verified on a live throwaway session before code depends on it.**
  Binary strings tell you what to try; a real session tells you what happens. Anything in this file
  that came from the binary was confirmed this way.

## Files
- `main.js` — BrowserWindow config, per-tab node-pty spawn (`spawnTab`), pty env scrubbing, pid→tab binding (`tabForPid`/`listAnnotated`, over the map `platform.parentMap()` returns), IPC wiring, session push to renderer
- `preload.js` — contextBridge, exposes `window.ptyAPI` (per-tab terminal IO, keyed by `tabId`), `window.meshAPI` (sessions, plan usage, CLI info, warnings) and `window.platformAPI` (the two facts the page can't work out for itself: vibrancy, title-bar inset)
- `renderer.js` — tab manager, xterm.js setup + theme, cell measurement, fit logic, sidebar render, sidebar resizer
- `sessions.js` — reads `~/.claude/sessions/*.json` + `history.jsonl` + tails `~/.claude/projects/*/*.jsonl` for title/branch/cwd; pure Node, no Electron deps, unit-testable standalone
- `rename.js` — renames a session in sync with the CLI: control socket for live ones, transcript records for dead ones; pure Node
- `autoname.js` — promotes a session's AI title into its actual name so no session stays on its cwd-derived autoname
- `usage.js` — the account's plan windows (5h / weekly): credential read, `/api/oauth/usage` fetch, normalizing, poll + backoff; pure Node, the OAuth token never leaves it
- `platform.js` — **every OS branch in the app**: window chrome, pid→ppid map, shell selection + argument quoting, socket usability. Pure Node; nothing else may test `process.platform`
- `config.js` — user settings (`settings.json` in the per-user data dir): `claudeBin`, `autoname`, `usage`. Pure Node, type-checked on read, atomic write
- `claude.js` — locates the CLI (config override → login-shell `command -v`), probes its version, and is what `spawnTab` puts at the front of the pty command
- `index.html` — shell page, `@font-face` for the bundled fonts, sidebar + terminal split layout, welcome/folder-picker pane
- `titlebar.js` — shared `HEADER_HEIGHT` constant (main + preload both require it)
- `assets/fonts/` — the bundled woff2 faces and their OFL license texts. Shipped, not installed
- `build/` — packaging inputs: `icon.svg` (source art), `icon.png` (committed, what electron-builder consumes), `entitlements.mac.plist`
- `tools/` — authoring scripts, never part of a build: `build-fonts.js` (ttf→woff2), `make-icon.js` (svg→png)

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
Scope is a **user setting** (`config.js`), not a constant, because this writes into state the
app doesn't own: `autoname.scope` is `'tabs'` by default (only sessions in our own tabs);
`'all'` covers every live session on the machine, including ones started in another terminal,
which is not a decision to make on a stranger's behalf. `autoname.enabled: false` turns it off
entirely.

## Reading the CLI's internals

Everything undocumented in here was read out of the shipped binary, not guessed. The method,
because it will be needed again on the next version bump:

```bash
readlink -f "$(which claude)"        # e.g. $(brew --prefix)/Caskroom/claude-code@latest/<ver>/claude
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

## Plan usage — the sidebar footer (`usage.js`)

The one **account-wide** fact in an app otherwise built out of per-session facts, which is why
it lives in its own footer strip rather than on any row. Running six agents at once burns the
5-hour window fast and the CLI can only answer for it from *inside* a session (`/usage`) — the
wrong shape for a question about the account.

**Source: `GET https://api.anthropic.com/api/oauth/usage`.** Exactly what the CLI's own
`fetchUtilization` calls (5s timeout, `Content-Type: application/json`, `anthropic-beta:
oauth-2025-04-20`). Verified live, HTTP 200:

```json
{ "five_hour": { "utilization": 8.0,  "resets_at": "2026-08-10T00:00:00.292938+00:00" },
  "seven_day": { "utilization": 21.0, "resets_at": "2026-08-15T15:59:59.292960+00:00" },
  "seven_day_opus": null, "seven_day_sonnet": null,
  "extra_usage": { "is_enabled": false, "monthly_limit": null, "used_credits": null, … },
  "limits": [ { "kind":"session", "group":"session", "percent":8, "severity":"normal",
                "resets_at":"…", "scope":null, "is_active":false }, … ],
  "spend": { … } }
```

- **Two unit conventions for one idea.** Top-level windows are `utilization` 0–100 with an ISO
  `resets_at`. The CLI's *other* path — the `anthropic-ratelimit-unified-*` response headers —
  uses a 0–1 fraction and epoch seconds. `normalize()` is the only place either shape is
  touched; nothing downstream may assume one.
- **Read the response by allowlist.** The live endpoint already returns keys the CLI's own list
  doesn't cover (`tangelo`, `nimbus_quill`, `amber_ladder`, `seven_day_cowork`,
  `seven_day_omelette` — unreleased, codenamed). Iterating whatever comes back would put mystery
  bars in someone else's sidebar. `WINDOWS` in `usage.js` is that allowlist, and its order is the
  render order.
- `Number(null)` is `0`, not `NaN` — an unset `monthly_limit` reads as a limit of zero unless
  null-checked first. Bit us once already.

**Credentials — read, never written, never refreshed.** macOS Keychain, service
`Claude Code-credentials`, one JSON blob: `{"claudeAiOauth":{accessToken, refreshToken, expiresAt,
scopes, subscriptionType:"pro", rateLimitTier}}`. `subscriptionType` is the plan chip. Fallback
for non-macOS is `~/.claude/.credentials.json` (the path the binary itself references). AgentMesh
never rotates either — the CLI does that, and AgentMesh always has live sessions making it happen.
The `security` read carries a timeout on purpose: if the Keychain item's ACL doesn't cover us the
read blocks on a **GUI prompt**, and a hung poller is worse than a missing bar. The token stays
inside `usage.js` and the main process; only normalized numbers cross the contextBridge.

**Why the local-only path can't be the primary.** `~/.claude.json → cachedUsageUtilization` holds
the same response body, but it's written from `loadPlanRateLimits` — i.e. **only when a human opens
`/usage`**, throttled to one write per 5 min (`yey=300000`) and discarded on read past 1 h
(`_ey=3600000`). It was absent entirely on this machine. Good as a cold-start seed and an offline
fallback, nothing more. Header-derived utilization never touches disk at all — it lives in CLI
process memory.

**Polling.** 5 min (the CLI's own persist throttle), plus window focus and `spawnTab`, with a 60s
floor between actual network calls so alt-tabbing can't turn into traffic. Failures back off
exponentially to 30 min and keep serving the last good value flagged `stale` — the footer dims
rather than vanishing. A 401 re-reads credentials once and retries once, since the CLI may have
rotated the token underneath us. `ANTHROPIC_API_KEY` / Bedrock / Vertex / Foundry in the env means
plan limits don't apply, and the footer renders nothing at all (`#usage:empty { display: none }`).

**Footer UI.** Widest window first — `week` on top, `session` under it, sitting directly above the
countdown that belongs to it. Bars are green / amber ≥75% / red ≥90%, the same three-state
vocabulary as the row status glyphs. Other windows' reset dates live in each row's hover title so
they don't fight for the one line underneath.

`renderUsage()` is **not** part of `render()`: that rebuilds every row on the 4s session poll,
while usage arrives on its own 5-minute cadence. The countdown is the **one timer in the sidebar** —
60s, all the precision `3h 12m` has, writing a single text node. The row ages this file killed off
were dropped because they cost a full repaint of every row; this costs one assignment.

## Local dev + testing

- Repo is git-backed as of this work; feature work goes on a branch, not `main`.
- **Never `pkill electron` to test.** The Claude Code session doing the work usually runs *inside*
  an AgentMesh tab, and `win.on('closed')` kills every pty — that kills the conversation. Launch a
  **second instance** instead (`nohup npm start > log 2>&1 &`); nothing in `main.js` takes a
  single-instance lock, and the two windows don't interfere. Kill only by the exact pid you
  launched.
- **"The exact pid you launched" means the Electron pid, not the `npm start` pid.** `npm start`
  → `node .bin/electron .` → the Electron main process; killing the npm wrapper tears the tree
  down, and this is how the *user's* window got killed once by a cleanup aimed at a test instance.
  Two further traps that made it hard to see coming: `pgrep -f AgentMesh/node_modules/electron`
  matches only the **helper** processes (the main binary lives at `Contents/MacOS/Electron`), and
  macOS pids wrap past 99999 back to low three-digit numbers, so a "new" instance can have a
  *smaller* pid than the one you started. Confirm with
  `ps -o pid=,ppid=,lstart=,command=` before killing anything, and check that the pid you're
  about to kill is not an ancestor of `$$`.
- Protocol work is tested against real throwaway sessions, spawned through `node-pty` the same way
  `spawnTab` does (including the env scrubbing). Two gotchas: a fresh cwd triggers the **trust
  prompt**, so write `\r` a few seconds after spawn; and registration is asynchronous, so poll
  `~/.claude/sessions/` for the new pid file rather than sleeping a fixed amount.
- Clean up after: throwaway sessions leave transcripts under `~/.claude/projects/<slug>/` that
  otherwise show up in the sidebar's `RECENT` group.
- **Renderer errors don't reach `npm start`'s stdout.** Launch with `ELECTRON_ENABLE_LOGGING=1`
  and the page's `console` lands in the log — otherwise a renderer that throws before first
  paint looks exactly like a window that opened fine.
- **Testing a *packaged* build**: run the binary directly with the cwd set to `/`
  (`cd / && dist/mac-arm64/AgentMesh.app/Contents/MacOS/AgentMesh`) — that reproduces the
  Finder/Dock launch, which is the only way the `process.cwd()` class of bug shows up. To
  exercise the packaged native module without clicking anything, `ELECTRON_RUN_AS_NODE=1` on
  that same binary with a script that requires node-pty **through the asar path** and spawns
  something (see the packaging section for why the unpacked path fails).
- **Platform branches are testable on one machine**: `platform.js` takes its answer from
  `os.platform()`, so a test can stub it and assert both the Unix and Windows shapes of
  `parentMap`, `quoteArg`, `shellCommand`, `windowOptions` and `canUseSocket` without a second
  OS. Do that before claiming a branch works — a branch that has never been evaluated is not a
  branch that has been written.

## Other `~/.claude` state (not used yet)

- `daemon/` — `roster.json`, `control.key`, `dispatch/` for background agents.
- `jobs/`, `tasks/`, `plans/` — background job dirs, scheduled tasks, plan mode artifacts.
- `session-env/<sessionId>/` — one dir per session, empty in practice on this machine.
- `history.jsonl`, `projects/`, `sessions/` — the three the sidebar actually reads.

## Sidebar behavior
- The plan-usage footer sits below the list, outside the scroller — it answers about the account,
  not any row. Its own section above covers it; it is not part of `render()`.
- **`#warnings` sits directly above it**, same `:empty`-is-hidden contract, amber, and also
  outside `render()`. It exists for degradations that would otherwise be invisible: no
  `claude` on PATH, or a process list the app can't read (which silently stops rows binding to
  tabs, and with them autonaming). Messages are deduplicated by `kind` and additive — a poll
  that keeps failing must not stack the same sentence, and a second problem must not overwrite
  the first. A feature that quietly stopped is worse than one that says why.
- **No CLI means no session to start**: `#new-agent` and the welcome pane's picker disable
  themselves and say so, rather than spawning a pty that exits 127 and a tab that vanishes.
  `claudeInfo === null` (not yet resolved) is a third state and disables nothing.
- The empty list distinguishes three emptinesses: scoped-and-empty (names the folder),
  unscoped-with-rows-behind-it, and **nothing at all** — which on a fresh machine is a first
  launch, not a failure, and says so.
- Each tab owns a real pty running its own `claude` — tabs are not one TUI switched with `/resume`. That's what makes them independently interruptible and closable.
- Click a row → if it's already bound to an open tab, focus that tab; if the session is **dead**, open a new tab running `claude --resume <sessionId>` in its own cwd. Live sessions this window doesn't host are never resumed — they fork, see below.
- Groups are `WAITING` / `RUNNING` / `IDLE` / `RECENT`, one per registry status, ordered by what they want from you. The old single `LIVE` bucket answered three questions at once. `WAITING` carries the amber status color on its header; unknown registry statuses fall into `RUNNING` (`statusOf` treats anything that isn't `idle`/`waiting` as working). Hosted tabs with no session row sit at the top of `RUNNING`.
- **Every hosted tab gets a card**, including ones no session row accounts for: a `claude agents` FleetView (hosts no sessionId of its own) and sessions too young to have registered. Before that, `.row-close` was gated on `.row.open`, which is `tabId && tabs.has(tabId)` — and `tabId` comes from walking the pid parent chain (`tabForPid`, main.js), which a daemon-owned bg agent never satisfies. Result: tabs that existed with no card and no way to close them.
- Card subtitle is `background · project · branch · <relative time>`. Not prompt count (bookkeeping), not pid or byte size (those live in the hover tooltip). `project` is dropped while the list is scoped to one folder, where every row would repeat the same word — keyed on `scopedDir`, not `projectOnly`, because the toggle can be armed with no folder to scope to.
- **`windowDir` is where the window is pointed** — one piece of renderer state answering that
  question, persisted in `localStorage`. It names what `#new-agent` starts and what the list is
  scoped to. Each window answers "what's running on what I'm looking at" first and acts as a
  machine-wide mesh view second, so `projectOnly` also starts **on** and persists.
  - It **follows the active tab**: `setActive` calls `setWindowDir(tab.cwd)`, because switching to
    a session in another repo *is* moving the window there — and without it the tab you just
    opened could be filtered out of the list showing it.
  - It also stands alone with no tabs, which is the whole point: `#dir` re-points the window
    without spawning anything.
  - It is **validated on load**, not trusted: the stored value is a raw absolute path from some
    previous launch, and repos get renamed and disks get unmounted. A folder that no longer
    exists would scope the list to nothing beside a header naming a directory that isn't there,
    so `meshAPI.dirExists` checks it once and clears it.
  - `sessions.js` used to compute an `isCurrentProject` flag against `process.cwd()`. That is
    where the *app* was launched — `/` for a Finder or Dock launch, which is every packaged
    launch — so it was meaningless and is gone. `process.cwd()` is not a default for anything
    any more; `spawnTab` falls back to `os.homedir()`.
  - Third state: toggle armed with **no folder chosen**. Filtering on nothing would empty the list
    next to the welcome pane, which is a dead end, so it shows everything and `#filter` dims
    (`.empty`) instead of lighting up.
  - `#filter` is a glyph, not a word: the folder it scopes to is named by `#dir` immediately to its
    left, so the toggle never has to repeat it.

## Forks are not duplicates

`--fork-session` copies the transcript wholesale, custom title included, so a fork and its origin
render as the same card twice. Parentage comes from two sources, ledger first:

- **`forkOrigins` in `localStorage`** (renderer.js) — when *we* fork, we hold the parent id against
  the new tab (`pendingForks`) and bind it in `syncTabs` once that tab's session registers. Depends
  on nothing internal to the CLI.
- **`forkParent()`** (sessions.js) — first line of the transcript carries a `sessionId` that
  disagrees with the filename → that's the parent. Cheap (8 KB head read, cached forever, the first
  line never changes) but **unconfirmed against a real fork**: across 65 transcripts on this machine
  no file carries a foreign session id, which only proves it doesn't false-positive. If forks turn
  out to be rewritten to the new id on copy, this path is a permanent no-op and the ledger is
  carrying the feature alone.

Forks render indented under their origin with a `fork` chip. A fork whose origin is in a different
group (parent idle, fork busy) stands on its own rather than dragging the parent across groups.

## Resume is not attach

`--resume` on a *running* session is wrong in two different ways, and the row click has to
branch on `kind` from the pid registry (`'interactive'` | `'bg'`):

- **`kind: 'interactive'`, running elsewhere** — the CLI allows it, and that's the trap. Each
  resume is a second live writer on one transcript. Left unguarded this compounds: one click per
  window per repaint produced 11 concurrent `claude --resume <same-id>` processes on this machine.
  AgentMesh never resumes one. The row carries an **`in use` chip and a fork button**, and a plain
  click forks (`--fork-session`, new session id, name deliberately not carried over). This used to
  be a hidden **⌥click** plus a transient row message explaining why the plain click did nothing —
  a modifier key is not an affordance, and an error after the fact is not a design.
- **`kind: 'bg'`** — the CLI refuses outright: `Error: Session <id> is currently running as a
  background agent (bg). Use \`claude agents\` to find and attach to it, or add --fork-session to
  branch off a copy.` The guard is `if (!t.forkSession) { … await $Fe(sessionId) … }` on every
  resume entrypoint (flag, file, title). Clicking a bg row opens a tab running
  `claude agents --cwd <row.cwd>` — FleetView is the only real attach path; there is **no
  `--attach` flag**, attaching is a keystroke inside that TUI (`bg-attach` / `job_attach` in the
  binary, driven over the daemon's `bg-pty-host` socket).

Background agents also differ in the registry: `kind: 'bg'`, a `jobId`, no `nameSource`, and
`name` is the **entire dispatch prompt including newlines** — `liveCustomName()` collapses
whitespace so one row stays one line.

## Pre-warmed spares — the phantom duplicate row

The bg daemon keeps a pool of **pre-warmed workers** so a FleetView dispatch skips cold start,
and **each spare registers a complete `sessions/<pid>.json` before anyone dispatches anything**.
That is the "duplicate session": one extra live `bg` row per use of the agents menu, same cwd,
named after nothing. Verified by spawning `claude agents` in a throwaway pty — opening the TUI
alone added a pid file (`kind:'bg'`, `status:'idle'`, `name === jobId === sessionId.slice(0,8)`),
and it vanished when the TUI closed. Nothing upstream to fix and no setting to disable prewarm;
the fix is to not count spares as sessions.

`~/.claude/daemon/roster.json` is the authoritative discriminator — `workers[short].dispatch`:

- unclaimed spare → `source: 'spare'`, `seed.intent: ''`, `launch.args` with no `--` prompt
- real dispatch → `source: 'fleet'`, `seed.intent` = the prompt, prompt after `--` in `launch.args`

`readSpares()` in `sessions.js` builds the skip set from that; `looksUnclaimedSpare()` is the
fallback for when the roster is missing or renames those fields (bg + `name === jobId`).
Claiming rewrites the entry in place, so a spare that picks up work reappears on the next poll.

Two processes per bg worker, both matching `claude bg-spare` in `ps` — argv does **not**
distinguish claimed from unclaimed, so don't try to filter on it. Also: `claude agents`
processes are `kind: 'bg'` too, and the process ancestry is
`bg-spare` ← `bg-pty-host` ← `claude daemon run`.
- Binding is by **pid ancestry**: one `ps -Ao pid=,ppid=` per refresh, then walk the live session's pid up its parent chain until it hits a tab's shell pid (`tabForPid` in `main.js`). The `claude` process sits 1–3 hops above the pty shell, so the 24-hop cap is generous.
- `syncTabs` binds each open terminal to the session that registered inside it, which is what the inline rename and the row's close button act on.
- Row meta is **where and what**, not bookkeeping: project, branch, prompt count. No relative age, no pid — both were noise, and dropping age also removed the 30s repaint timer that existed only to keep it honest.
- Status glyph follows **GitHub Actions**: amber Primer spinner while busy, green dot on idle, muted dot when dead. Colors are Primer's own dark tokens (`--fgColor-attention #d29922`, `--fgColor-success #3fb950`, `--fgColor-muted #9198a1`), markup is Octicons' `dot-fill` plus Primer's Spinner ring+arc. Every spinner gets a **negative `animation-delay`** (`-${performance.now() % 1000}ms`) so rows built at different times rotate in phase — same trick as Primer's `computeSyncDelay`. The busy→idle flip fires a one-shot ring pulse (`.landed`), tracked in `lastStatus`/`landed` **outside the DOM** because `render()` rebuilds every row on the 4s poll and would otherwise re-pulse forever. `prefers-reduced-motion` kills both animations.
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
- **There is no tab strip**, and nothing replaced it. The sidebar is the switcher — it already lists every session on the machine and marks the ones we host, so a second row of the same sessions across the top showed the user the same thing twice. The terminal runs to the top of the window: the traffic lights sit over the sidebar (which is what `#sidebar-header`'s 84px left pad is for), and the sidebar is drag region enough to move the window by, so the terminal pane needs to reserve nothing.
- **The sidebar header is a folder switcher, and nothing else.** `#dir` names the window's folder and opens the picker; `#filter` toggles the scope. Starting a session is `#new-agent`, one button, below, in the folder the header names.
  - It used to be a session count plus a `+`. The count was a number nobody acts on. The `+` was worse: it read as "new session" and behaved as "change folder", which is the most misleading pairing available — the affordance and the effect pointed at different things.
  - The button no longer repeats the folder name either (`.where` is gone). The header owns that string; printing it again 20px lower is the same fact twice.
- **Zero tabs is a legitimate state.** Launch shows `#welcome` (folder picker + recent folders pulled straight out of the session rows, so there's no second history to keep in sync) and spawns nothing. A session's cwd is fixed at spawn and decides which project it belongs to, so the folder is asked for up front rather than inherited from wherever the app was launched.

## The pty *is* Claude Code
`spawnTab` runs `$SHELL -l -c 'exec <claude> …'`, not a shell it then types `claude` into.
`<claude>` is the absolute path `claude.js` resolved at startup, not the bare word — the login
shell would usually find the same binary, but resolving once means every tab runs the CLI that
was actually probed rather than whatever a later PATH edit shadows it with. Bare `claude`
remains the fallback when resolution failed. Two consequences, both wanted:
- Quitting Claude Code ends the pty, so `pty-exit` closes the terminal instead of leaving the user at a bare prompt in a tab that no longer stands for anything. Last one closed → back to `#welcome`.
- The `claude` process is the pty process, so its parent is Electron directly. The `tabForPid` ancestry walk resolves in one hop now instead of three.

The login shell is still worth paying for — it's what puts `claude` and the user's toolchain on `PATH` — but `exec` replaces it rather than leaving it as a parent.
- Tab spawn/exit/close all call `pushSessions()` so pid→tab binding re-resolves without waiting on the 4s poll.
- `fitAll()` resizes **every** tab, not just the active one — a hidden tab still has a live pty whose `cols`/`rows` must track the window, or its output wraps wrong the moment you switch to it.

## Settings (`config.js`)

`settings.json` in `app.getPath('userData')`, absent by default, every key optional and
type-checked on read (a hand-edited `"scope": true` degrades to the default rather than
reaching a rename call). Written atomically via temp-file + rename, because a half-written
file read on the next launch would silently reset every setting.

Three keys, and the test for what belongs here: **does it write into state this app doesn't
own, or talk to the network?** `claudeBin` (PATH escape hatch), `autoname` (renames sessions
in `~/.claude`), `usage` (the one network call). Sidebar width, window folder, scope toggle
and the fork ledger stay in `localStorage` — that is UI state, it already persists per user,
and putting it in a JSON file invites hand-editing a layout constant.

## Keys
- `Shift+Enter` and `Ctrl+Enter` insert a newline instead of submitting: the renderer intercepts both in `attachCustomKeyEventHandler` and writes `\x1b\r` (ESC+CR) to the pty. xterm sends a bare CR for these by default, which is "send"; ESC+CR reads as alt/option-Enter, which Claude Code's input treats as a line break natively — no `/terminal-setup` or terminal-side keybinding needed.
- **`e.preventDefault()` in that handler is load-bearing.** xterm's `_keyDown` clears `_keyDownHandled` *before* consulting the custom handler and returns early without suppressing the browser default, so `_keyPress` still fires and emits charCode 13 — a bare CR landing right after our ESC+CR. Claude Code then sees newline-then-submit and the prompt sends anyway. Killing the default keydown stops Chromium dispatching that keypress at all. The handler also returns `false` for `e.type === 'keypress'` as a guard for engines that dispatch it regardless.

## Packaging (`electron-builder`)

`npm run dist` builds an installer; `npm run pack` builds unpacked into `dist/` for testing
the packaging itself. Config lives in `package.json` under `build`. Four things in it are
load-bearing and were each found by a failure:

- **`asarUnpack: node_modules/node-pty/**`.** node-pty ships `spawn-helper`, a real executable
  it `posix_spawn`s. It cannot run from inside an asar, and without this every tab fails to
  spawn with `posix_spawnp failed`.
- **node-pty must be `require`d through the asar path, not the unpacked one.** Its own code
  does `helperPath.replace('app.asar', 'app.asar.unpacked')`. Point a require at the unpacked
  copy directly and that rewrite fires on a path that is *already* unpacked, producing
  `app.asar.unpacked.unpacked` and the same `posix_spawnp failed`. This bit during testing,
  not in the app — worth knowing before diagnosing it as a packaging bug.
- **`files` is an explicit allowlist**, so `assets/**/*` and the two xterm files
  (`index.html` loads `node_modules/xterm/{lib/xterm.js,css/xterm.css}` by relative path) have
  to be named. Anything new the page loads at runtime must be added there or it silently
  won't ship.
- **`postinstall: electron-builder install-app-deps`** rebuilds node-pty against Electron's
  ABI. npm 11+ gates lifecycle scripts, so a fresh clone may need `npm approve-scripts` once.

macOS builds are unsigned without credentials; `build/entitlements.mac.plist` covers the
hardened runtime (JIT, library validation off for the native module, `inherit` for the pty's
children) and only matters once `CSC_*` exists. Linux has no node-pty prebuild — a build host
needs `python3` + a compiler.

## Fonts — bundled, not installed
- **Terminal**: `JetBrainsMono Nerd Font Mono`. Pick the `Mono` variant, not `Propo` — fixed-width glyphs align in monospace cells. Settings: fontSize 13, lineHeight 1 (flush, needed for clean ASCII art), letterSpacing 0.
- **Everything else** (sidebar, welcome pane): `Inter`. Variable, so two files cover every weight.
- Both ship as woff2 in `assets/fonts` with `@font-face` in `index.html`. They used to be
  Homebrew casks the user installed by hand, which made a clone on any other machine silently
  wrong — not just uglier, but **mis-measured**, since the cell width feeds every terminal's
  geometry. Regenerate with `npm run fonts` (needs the TrueType originals installed; the
  woff2s are committed, so nobody else runs it). Both are SIL OFL 1.1 — license texts ship
  beside them and stay there.
- **A page-declared `@font-face` shadows a system font of the same name**, which is what makes
  the bundling real rather than a fallback nobody reaches.
- One-cell margin on all four sides of the terminal comes from `--cell-w` / `--cell-h`, measured in `renderer.js` before the first `term.open()` — xterm sizes against the container box at open time, so neither can be read off a live terminal. Width uses **canvas `measureText`, not DOM layout**; height uses a hidden `font: <size>px/1 <family>` span probe. `fitAll()` subtracts `2 * CELL_W` from the available width and `2 * CELL_H` from the height.
- **The first measurement runs before the webfont is loaded, and lands on the fallback.**
  Measured: 7.8266px (Menlo) versus 7.8px (JetBrains Mono at 13px, 0.6em advance) — on a
  machine where the font was *also* installed system-wide, because `font-display: block`
  hadn't resolved yet. So `renderer.js` re-measures behind `document.fonts.load(...)` +
  `ready` and refits, and `openTab` waits on that same promise before `term.open()` — xterm
  bakes the container's metrics in at open time and a later resize will not undo it.

## Platforms — what runs where, and what degrades

Every branch is in `platform.js`. Nothing else in the app tests `process.platform`.

| | macOS | Linux | Windows |
|---|---|---|---|
| Terminals, sidebar, sessions | verified | supported | **written for, never run** |
| Window chrome | `hiddenInset` + traffic lights over the sidebar | frameless, opaque | frameless + `titleBarOverlay`, opaque |
| Vibrancy | yes | no — `body.no-vibrancy` paints `#0f1116` | same |
| pid→tab binding | `ps -Ao pid=,ppid=` | same | PowerShell `Get-CimInstance Win32_Process`, CSV-parsed |
| pty shell | `$SHELL -l -c 'exec …'` | same | `powershell.exe -NoLogo -Command …` (no `exec`; the host process stays) |
| Argument quoting | POSIX `'…'\''…'` | same | PowerShell `'…''…'` |
| Rename over the socket | unix socket | unix socket | named pipe if the CLI writes one — **unverified** |
| Credentials | Keychain via `security` | `~/.claude/.credentials.json` | `~/.claude/.credentials.json` |

Two things that are true because of how this was built, and stay true:

- **`transparent: true` off macOS is a see-through window, not a subtler one.** The 27%-alpha
  body only works because NSVisualEffectView paints behind it. Anywhere else the page has to
  paint its own background, which is what `body.no-vibrancy` is for.
- **Windows is honest best-effort.** Nothing about the Windows CLI's on-disk state has been
  checked against a real machine — in particular whether `messagingSocketPath` exists there at
  all. `canUseSocket()` is written so a named pipe would work and a missing one falls through
  to the transcript path rather than throwing. First person to run it should record what the
  pid registry actually contains.

## Chrome vs glass
- Sidebar is **deliberately opaque** (`--chrome-bg: #1b1e24`), which kills vibrancy behind it. Only the terminal pane keeps the blur.
- The sidebar resizer is a 6px absolutely-positioned strip straddling the sidebar's right edge (`right: -3px`). It **must** carry `-webkit-app-region: no-drag`, otherwise dragging it drags the whole window instead.
- While dragging, `body.resizing` sets `pointer-events: none` on the terminal pane so the pointer can't get swallowed by the xterm canvas mid-drag.
- Terminals refit **at drop**, not per `mousemove` — a `term.resize()` per mouse event thrashes xterm's renderer and the pty.
- Width clamps to 180–520px, persists in `localStorage` under `sidebarWidth`, double-click resets to 248.

## Theme — One Dark Pro Glass
Colors were copied from the **One Dark Pro Max** Zed extension's `one-dark-pro-glass` theme
JSON (its `terminal.background` of `#08090900` is where the zero-alpha trick below comes
from). Copied, not read: the full ANSI 16-color palette lives in the `renderer.js` theme
object and nothing loads that file at runtime — it isn't on anyone else's machine.

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
