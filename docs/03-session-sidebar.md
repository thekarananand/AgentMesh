# Session Sidebar — Data Sources

## Reading the session registry

- **`claude agents --json` / `~/.claude/sessions/<pid>.json`** — **live** sessions only, most 
  stable API. Has `sessionId`, `pid`, `status` (busy/idle), `cwd`, socket path. 
  `sessions.js` reads the JSON files directly (skip shelling out) and validates the pid is 
  still alive with `process.kill(pid, 0)` — the registry file can outlive the process on a crash.

- **`~/.claude/history.jsonl`** — flat log of every prompt ever, any project. Cheap source for 
  first-prompt/last-prompt/prompt-count per sessionId. Grows forever — cache the parsed Map 
  keyed on the file's `mtimeMs`, don't reparse every poll.

- **`~/.claude/projects/<slug>/<sessionId>.jsonl`** — transcripts. **Explicitly unstable format 
  per Anthropic docs ("changes between versions")**, so every read is defensive (try/catch 
  around JSON.parse per line, never throw). Only tail the last ~96KB (`tailRead`) — these 
  files grow unbounded and we only want the trailing `ai-title`/`mode`/`cwd` records, not the 
  whole conversation.

## Title priority

Deliberate name > AI-generated title > first prompt > raw session id:

1. Live registry `name` when `nameSource !== 'derived'` — user explicitly named it
2. Transcript's `custom-title` or `agent-name` — written by rename/autoname
3. AI-generated `ai-title` record from transcript
4. First prompt from history
5. Raw session id as fallback

See [04-session-rename.md](04-session-rename.md) for why `nameSource` is tested that way.

## Transcript records in the wild

Append-only records, last-wins semantics:
- `user` — user message
- `attachment` — file attachment
- `last-prompt` — final prompt in session
- `mode` — current mode
- `permission-mode` — permission state
- `ai-title` — AI-generated title
- `custom-title` — user-set name
- `agent-name` — alternative name field
- `cwd` — working directory

`scanTranscript` reads the tail **backwards** and takes the first hit of each kind.
