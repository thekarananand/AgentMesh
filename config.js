// User settings — the things that should differ between machines and people.
//
// AgentMesh reads and writes another program's state (`~/.claude`), and two of its habits
// are opinions rather than facts: that every session should be force-named, and that the
// app may call home for the account's plan usage. Both are right for the person who wrote
// them and neither should be imposed on a stranger's install with no way out. So they live
// in a file the user owns, alongside the escape hatch for a CLI that PATH can't find.
//
// Deliberately *not* here: sidebar width, the window's folder, the scope toggle, the fork
// ledger. Those are UI state, they already persist per-user in the renderer's storage, and
// moving them into a config file would turn "the sidebar is 280px" into something a user is
// invited to hand-edit. This file is behavior, not layout.
//
// Pure Node, no Electron import — the caller passes the directory, which keeps this
// testable standalone like sessions.js and rename.js.

const fs = require('fs');
const path = require('path');

const FILE_NAME = 'settings.json';

const DEFAULTS = {
  // Absolute path to the Claude Code binary. `null` means "find it on PATH", which is right
  // on every machine where a login shell can see it.
  claudeBin: null,

  // Promote a session's AI title into its actual name (autoname.js). On by default because
  // an unnamed session in a list of twelve is the problem this app exists to fix — but
  // `scope: 'all'` would rename sessions started in someone else's terminal, so the default
  // stays confined to sessions running in our own tabs.
  autoname: { enabled: true, scope: 'tabs' },

  // The single network call in the app: the account's own plan windows (usage.js). Opt-out
  // rather than opt-in, since the CLI already makes the same call, but a user who wants a
  // strictly offline app gets one.
  usage: { enabled: true },
};

const SCOPES = new Set(['tabs', 'all']);

let file = null;
let current = structuredClone(DEFAULTS);

function isPlainObject(v) {
  return Boolean(v) && typeof v === 'object' && !Array.isArray(v);
}

// One level of merge, and every value type-checked against its default. A settings file is
// hand-editable by definition, so a typo (`"scope": true`) has to degrade to the default
// rather than propagate into a rename call.
function coerce(raw) {
  const out = structuredClone(DEFAULTS);
  if (!isPlainObject(raw)) return out;

  if (typeof raw.claudeBin === 'string' && raw.claudeBin.trim()) out.claudeBin = raw.claudeBin.trim();

  if (isPlainObject(raw.autoname)) {
    if (typeof raw.autoname.enabled === 'boolean') out.autoname.enabled = raw.autoname.enabled;
    if (SCOPES.has(raw.autoname.scope)) out.autoname.scope = raw.autoname.scope;
  }

  if (isPlainObject(raw.usage) && typeof raw.usage.enabled === 'boolean') {
    out.usage.enabled = raw.usage.enabled;
  }

  return out;
}

// `dir` is the app's per-user data directory (app.getPath('userData') in main.js). A missing
// or unreadable file is the normal first-run case, not an error — defaults stand and nothing
// is written until something actually changes.
function init(dir) {
  file = path.join(dir, FILE_NAME);
  try {
    current = coerce(JSON.parse(fs.readFileSync(file, 'utf8')));
  } catch {
    current = structuredClone(DEFAULTS);
  }
  return current;
}

function get() {
  return current;
}

// Write via a temp file and rename: a half-written settings.json read on the next launch
// would silently reset every setting to its default, which is the kind of data loss a user
// blames on the app rather than on a crash.
function save() {
  if (!file) return false;
  try {
    const tmp = `${file}.tmp`;
    fs.writeFileSync(tmp, `${JSON.stringify(current, null, 2)}\n`);
    fs.renameSync(tmp, file);
    return true;
  } catch {
    return false;
  }
}

function set(patch) {
  current = coerce({ ...current, ...(isPlainObject(patch) ? patch : {}) });
  save();
  return current;
}

module.exports = { init, get, set, DEFAULTS, FILE_NAME };
