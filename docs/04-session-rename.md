# Session Rename — The UDS Control Socket

Claude Code exposes an undocumented control channel on the per-session socket in
`~/.claude/sessions/<pid>.json` (`messagingSocketPath`, i.e. `/tmp/cc-socks/<pid>.sock`).

## Protocol

One newline-terminated JSON line renames a live session:

```json
{"type":"control","action":"rename","name":"my-name","session_id":"<uuid>"}
```

### Implementation notes

- `session_id` is optional in the protocol but **always send it** — Claude Code validates it 
  and drops the message on mismatch. A pid file can outlive its process and the socket path 
  is keyed on a pid the OS is free to reuse.

- There is **no ack**. A flushed write is the strongest signal available; confirmation arrives 
  out of band when the CLI rewrites its pid file and `sessions.watch()` fires.

- One write updates everything at once: prompt-box banner, terminal title (OSC 0), the pid
  registry, and two transcript records (`custom-title` + `agent-name`). Same pipeline `/rename`
  (alias `/name`) uses.

## Implementation in rename.js

`rename.js` picks the path based on session state:
- **Socket for live sessions** — active processes listening on the control socket
- **Transcript append for dead sessions** — appends the same two records to the transcript

Verified that the CLI's own `claude --resume` picker reads back an appended `custom-title`,
so the archived path is genuinely in sync, not a local shadow.

## nameSource semantics in the pid registry

- `'derived'` — the cwd-derived autoname (`agentmesh-da`)
- **Absent entirely** — once a name is set on purpose, the key disappears
- `'user'` — belongs to the *daemon/background-job* record, a different store — testing for it 
  against the pid registry never matches

Resume restores a custom title on its own, but the registry name is minted fresh at startup and
falls back to the derived autoname, so `spawnTab` passes `--name` alongside `--resume` when the
row carries a real name.
