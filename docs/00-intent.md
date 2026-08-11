# Intent — read this before proposing work

Three goals at once, in this priority order. A change that serves one but breaks another is not a win.

1. **Orchestration control plane for many agents.** The point is running several Claude Code
   agents in parallel — across different projects, not just tabs on one repo — and being able to
   see all of them, tell them apart, and eventually route work *between* them. The peer-message
   socket documented in [07-cross-session-messaging.md](07-cross-session-messaging.md) is the 
   substrate for that, which is why it's mapped out in far more detail than anything currently 
   calls for. Terminal tabs are the plumbing, not the product.
2. **A better daily driver than Terminal.app for Claude Code.** Whatever the orchestration story
   becomes, this is the app the user actually lives in all day. Latency, key handling, fonts,
   vibrancy and window chrome are product surface, not polish — a control plane nobody wants to
   sit inside doesn't get used.
3. **Shippable to other developers, not a personal hack.** Assume a fresh machine and someone
   else's `~/.claude`. No hardcoded paths to this user's home, no assumption a given session,
   project or CLI version exists. Everything read out of the CLI is version-fragile by nature, so
   degrade to a working app with less information rather than throwing.

## Portability is a standing requirement

Every change must keep the app installable and runnable on a machine that is not this one: 
a different user, a different home directory, a different OS, a fresh `~/.claude`.

### No hardcoded dependencies
- If something must exist on the machine, it is either bundled in the repo (fonts, icon), 
  resolved at runtime with a real failure message (the `claude` binary — see `claude.js`), 
  or listed in the runtime dependencies table.
- A hand-installed Homebrew cask that nothing verifies is exactly the bug this rule exists 
  to prevent: it worked here and silently produced a wrong app everywhere else.

### Document every dependency
- Update `README.md` (what a user must install) and runtime dependencies table (what the app 
  assumes at runtime, and what happens when it's missing).
- "It's obvious" and "it's already on my machine" are how the last set of these got missed.

### OS branching
- Every OS branch goes in `platform.js`, never inline. A `process.platform` check in
  `main.js` or `renderer.js` is a bug in the making — the second one always gets forgotten.
- See [21-platforms.md](21-platforms.md) for the complete platform matrix.

### Configuration belongs in config.js
- User-specific behavior belongs in `config.js`, not a constant.
- Anything that writes to someone else's `~/.claude`, or talks to the network, needs an off switch.
- See [17-settings.md](17-settings.md).

## Decided — do not relitigate

- **Never fork, patch or vendor the `claude` binary.** Read its local state and speak its
  protocols. Reading strings out of the shipped binary to *learn* the protocol is fine and
  expected (see [06-reading-internals.md](06-reading-internals.md)); shipping a modified one is not.

- **No direct mutation of `~/.claude` state files.** Writes go over the control socket, or append
  records in the exact format the CLI itself writes (`custom-title`/`agent-name`) so its own
  `--resume` picker reads them back. Never rewrite or hand-edit a pid registry, transcript or
  `history.jsonl` in place.

- **Local, single-user, no server.** No remote machines, no daemon of our own, no auth flow of our
  own. Everything is this machine's filesystem and unix sockets, with exactly one exception:
  a read-only `GET /api/oauth/usage` against the user's own Anthropic account, using the OAuth
  token the CLI already stored — the same call `/usage` makes (see [08-plan-usage.md](08-plan-usage.md)).
  Nothing is ever POSTed, no credential is written, no third party is contacted. Adding a 
  *second* network call is a new decision, not a precedent already set.

- **Undocumented behavior must be verified on a live throwaway session** before code depends on it.
  Binary strings tell you what to try; a real session tells you what happens. Anything in this
  documentation that came from the binary was confirmed this way.
