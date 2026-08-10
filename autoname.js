// Every session gets a real name, whether or not anyone typed one.
//
// Claude Code has no setting for this. Left alone it gives a session a cwd-derived
// autoname (`agentmesh-da`) that says nothing about what the session is doing, and
// separately generates an `ai-title` record that describes the session perfectly —
// but never promotes that title to the session's *name*. `/rename` with no argument
// would generate one, but only if a human types it.
//
// So AgentMesh closes the gap: once a session has enough context to describe itself,
// its AI title (or failing that, its first prompt) is pushed into the name over the
// same control socket a manual rename uses. The result is indistinguishable from the
// user having renamed it — it shows up in the prompt box, the terminal title, the
// CLI's own resume picker, and here.
//
// Anything already carrying a deliberate name is left completely alone.

const { rename } = require('./rename');
const config = require('./config');

// Scope comes from the user's settings, because renaming is a write into state this app
// doesn't own:
//   'tabs' — only sessions running in one of our own tabs (the default).
//   'all'  — every live session on the machine, including ones started in another
//            terminal. Correct for a fleet view, but it renames things this app didn't
//            launch, which is not a decision to make on someone else's behalf.
// `autoname.enabled: false` turns the whole thing off and leaves Claude Code's own
// cwd-derived autonames alone.
function settings() {
  return config.get().autoname;
}

const MAX_WORDS = 4;

// Claude Code's own generated names are lowercase kebab-case, 2-4 words
// ("fix-login-bug"). Match that so auto-named sessions don't look foreign next to
// hand-named ones.
function kebab(text, maxWords = MAX_WORDS) {
  const words = String(text || '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, ' ')
    .trim()
    .split(/\s+/)
    .filter(Boolean);
  return words.slice(0, maxWords).join('-');
}

// One attempt per session per app run. Without this, a session whose rename keeps
// failing would be retried on every watcher tick for as long as it stays open.
const attempted = new Set();

function candidateName(row) {
  // The AI title is a real summary of the session; the first prompt is a decent
  // stand-in before that title exists. A session with neither has nothing to say
  // about itself yet, so naming it now would just bake in noise.
  return kebab(row.aiTitle) || kebab(row.intent) || null;
}

function shouldName(row) {
  if (!settings().enabled) return false;
  if (!row.live || !row.socket) return false;
  if (row.customTitle) return false; // already deliberately named — never touch
  if (settings().scope === 'tabs' && !row.tabId) return false;
  if (attempted.has(row.sessionId)) return false;
  return Boolean(candidateName(row));
}

// Fire-and-forget: called from the same place that pushes sessions to the renderer,
// so a failure here can never block or break the session list.
async function autoName(rows, onDone) {
  const targets = (rows || []).filter(shouldName);
  if (!targets.length) return;

  let renamed = 0;
  for (const row of targets) {
    attempted.add(row.sessionId);
    try {
      const res = await rename(row, candidateName(row));
      if (res && res.ok) renamed += 1;
    } catch {}
  }
  if (renamed && typeof onDone === 'function') onDone();
}

module.exports = { autoName, kebab };
