# AgentMesh

A control plane for running many Claude Code agents at once.

Each tab is a real terminal running its own `claude`. The sidebar lists every Claude Code
session on the machine — live and recent, across every project — read straight out of the
CLI's own local state, so a session started in Terminal.app shows up here too. Click a row to
attach, fork, or resume it.

<!-- A screenshot belongs here. -->

## Requirements

**One thing:** the [Claude Code CLI](https://claude.com/claude-code) on your `PATH`. AgentMesh
spawns it per tab and reads its state under `~/.claude`. If `claude` runs in your shell, it
runs here.

Fonts ship with the app. Nothing else needs installing.

Supported: **macOS** (developed and tested on), **Linux** (supported, less exercised),
**Windows** (best-effort — see below).

## Install

Download a build from Releases, or build one yourself:

```bash
git clone <repo> && cd AgentMesh
npm install          # rebuilds node-pty against Electron's ABI via postinstall
npm start            # run from source
npm run dist         # build an installer for this platform into dist/
```

If npm refuses to run the `node-pty` install script (npm 11+ gates lifecycle scripts), run
`npm approve-scripts` once — the native module cannot build without it.

macOS builds are unsigned unless you supply signing credentials, so the first launch is
blocked by Gatekeeper. Either right-click → Open, or:

```bash
xattr -dr com.apple.quarantine /Applications/AgentMesh.app
```

Linux builds need a compiler toolchain, because `node-pty` publishes no Linux prebuild:
`python3`, `make`, and a C++ compiler (`build-essential` on Debian/Ubuntu).

## Settings

`settings.json` in the app's per-user data directory — `~/Library/Application
Support/AgentMesh/` on macOS, `~/.config/AgentMesh/` on Linux, `%APPDATA%\AgentMesh\` on
Windows. Absent by default; every key is optional.

```jsonc
{
  // Absolute path to the CLI. null = find `claude` on PATH.
  "claudeBin": null,

  // Promote each session's AI-generated title into its actual name, so no session sits
  // in the list under a meaningless cwd-derived autoname.
  //   scope "tabs" — only sessions running in AgentMesh's own tabs (default)
  //   scope "all"  — every live session on the machine, including ones you started
  //                  elsewhere. It renames things this app didn't launch.
  "autoname": { "enabled": true, "scope": "tabs" },

  // The plan-usage footer, and with it the app's only network call: a read-only
  // GET /api/oauth/usage against your own Anthropic account, using the token the CLI
  // already stored. Nothing is ever sent.
  "usage": { "enabled": true }
}
```

## What it does to your machine

Nothing permanent, and nothing outside two places.

- **Reads** `~/.claude`: the live session registry, `history.jsonl`, and session transcripts.
  All read-only. Set `CLAUDE_CONFIG_DIR` and it follows.
- **Writes**, only when renaming a session: a control message to the session's own socket
  (the same path `/rename` uses), or — for a session that has already exited — the same two
  records the CLI itself appends to that session's transcript. No state file is ever
  rewritten in place.
- One network call, described above, off-switchable.

It never modifies, patches or vendors the `claude` binary.

## Platform notes

| | macOS | Linux | Windows |
|---|---|---|---|
| Terminals, sidebar, sessions | yes | yes | expected, unverified |
| Window blur (vibrancy) | yes | opaque theme | opaque theme |
| Live rename over the session socket | yes | yes | unverified — falls back to the transcript |
| Plan-usage footer | Keychain | `~/.claude/.credentials.json` | `~/.claude/.credentials.json` |

Windows is written for but has not been run: it is unknown whether the CLI exposes its
per-session control socket there at all. Everything degrades to a working app with less
information rather than failing — but if you run it on Windows, please report what happens.

## Development

```bash
npm start            # run from source
npm run pack         # unpacked build in dist/, for testing packaging itself
npm run fonts        # regenerate assets/fonts/*.woff2 from installed TrueType originals
npm run icon         # regenerate build/icon.png from build/icon.svg (macOS only)
```

`npm run fonts` and `npm run icon` are authoring tools. Their outputs are committed — a clone
builds without either.

Testing note: **never `pkill electron`.** Launch a second instance and kill it by its exact
Electron pid; the app's `closed` handler kills every pty, which will take down whatever
Claude Code sessions are running inside the other window.

See `CLAUDE.md` for the architecture, the CLI internals this is built on, and how they were
verified.

## Third-party

- [JetBrains Mono Nerd Font](https://github.com/ryanoasis/nerd-fonts) — SIL OFL 1.1
- [Inter](https://rsms.me/inter/) — SIL OFL 1.1
- [xterm.js](https://xtermjs.org), [node-pty](https://github.com/microsoft/node-pty),
  [Electron](https://electronjs.org)

License texts for the bundled fonts sit beside them in `assets/fonts/`.
