# Always-Named Sessions (autoname.js)

Claude Code has **no setting** to force session naming — checked the whole settings key space.
`/rename` with no argument generates a name, but only when a human types it, and the 
`ai-title` record it generates on its own is never promoted to the session's *name*.

So AgentMesh promotes it:
- `pushSessions()` runs `autoName()`
- Pushes a session's AI title (or, before that exists, its first prompt) into the name 
  over the control socket
- Kebab-cased to match Claude Code's own generated-name style
- Anything already deliberately named is never touched
- Each session is attempted once per app run so a failing rename can't retry-storm

## User configuration

Scope is a **user setting** (`config.js`), not a constant, because this writes into state 
the app doesn't own:

- `autoname.scope: 'tabs'` (default) — only sessions in our own tabs
- `autoname.scope: 'all'` — covers every live session on the machine, including ones started 
  in another terminal (not a decision to make on a stranger's behalf)
- `autoname.enabled: false` — turns it off entirely

See [17-settings.md](17-settings.md).
