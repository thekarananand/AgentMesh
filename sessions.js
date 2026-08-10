// Reads Claude Code session metadata from the CLI's local state.
//
// Three sources, most-stable first:
//   1. ~/.claude/sessions/<pid>.json  — live registry (pid, name, status, socket)
//   2. ~/.claude/history.jsonl        — every prompt ever, with sessionId + project
//   3. ~/.claude/projects/*/*.jsonl   — transcripts; tail-scanned for ai-title/branch/cwd
//
// (3) is explicitly internal-and-unstable per Anthropic's docs, so every read of it
// is defensive: a parse failure degrades the row, it never throws.

const fs = require('fs');
const path = require('path');
const os = require('os');

const CLAUDE_DIR = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
const PROJECTS_DIR = path.join(CLAUDE_DIR, 'projects');
const SESSIONS_DIR = path.join(CLAUDE_DIR, 'sessions');
const HISTORY_FILE = path.join(CLAUDE_DIR, 'history.jsonl');
const ROSTER_FILE = path.join(CLAUDE_DIR, 'daemon', 'roster.json');

const TAIL_BYTES = 96 * 1024; // enough to catch ai-title + a metadata-bearing record
const MAX_SESSIONS = 60;

function safeParse(line) {
  try {
    return JSON.parse(line);
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------- live sessions

function pidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

// The bg daemon keeps a pool of pre-warmed workers so a FleetView dispatch can start
// without paying cold-start. Opening `claude agents` alone is enough to spawn one, and
// every spare registers a complete `kind: 'bg'` pid file in sessions/ before anyone has
// asked it to do anything — which is exactly the phantom row that appears next to a real
// agent every time the agents menu is used. (Verified: opening the TUI added
// `sessions/<pid>.json` with `name === jobId === sessionId.slice(0,8)`; it disappeared
// again when the TUI closed.)
//
// daemon/roster.json is the authoritative tell. An unclaimed spare is
// `dispatch.source: 'spare'` with an empty `seed.intent`; claiming rewrites the entry to
// `source: 'fleet'` carrying the dispatch prompt.
function readSpares() {
  const raw = safeParse(readFileQuiet(ROSTER_FILE) || '');
  if (!raw || !raw.workers) return null; // no roster — caller falls back to shape
  const set = new Set();
  for (const w of Object.values(raw.workers)) {
    if (!w || !w.sessionId) continue;
    const d = w.dispatch || {};
    if (d.source !== 'spare') continue;
    if (((d.seed && d.seed.intent) || '').trim()) continue; // claimed in place
    set.add(w.sessionId);
  }
  return set;
}

// roster.json is internal state like everything else here, so if it's missing or a
// version bump renames those fields, fall back to the registry shape of an unclaimed
// spare: a bg session whose name is nothing but its own short job id. A real background
// agent's name is its dispatch prompt.
function looksUnclaimedSpare(raw) {
  return raw.kind === 'bg' && Boolean(raw.jobId) && raw.name === raw.jobId;
}

function readLive() {
  const bySessionId = new Map();
  let files = [];
  try {
    files = fs.readdirSync(SESSIONS_DIR).filter((f) => f.endsWith('.json'));
  } catch {
    return bySessionId;
  }

  const spares = readSpares();

  for (const f of files) {
    const raw = safeParse(readFileQuiet(path.join(SESSIONS_DIR, f)) || '');
    if (!raw || !raw.sessionId) continue;
    if (raw.pid && !pidAlive(raw.pid)) continue; // stale registry entry
    if (spares ? spares.has(raw.sessionId) : looksUnclaimedSpare(raw)) continue;
    bySessionId.set(raw.sessionId, {
      pid: raw.pid,
      name: raw.name,
      nameSource: raw.nameSource,
      status: raw.status,
      // 'interactive' | 'bg' — a background agent is owned by the daemon, not by a
      // terminal, and the CLI refuses `--resume` on one while it runs.
      kind: raw.kind,
      jobId: raw.jobId,
      entrypoint: raw.entrypoint,
      startedAt: raw.startedAt,
      updatedAt: raw.updatedAt,
      version: raw.version,
      cwd: raw.cwd,
      socket: raw.messagingSocketPath,
    });
  }
  return bySessionId;
}

function readFileQuiet(p) {
  try {
    return fs.readFileSync(p, 'utf8');
  } catch {
    return null;
  }
}

// -------------------------------------------------------------------- history

// history.jsonl grows forever; re-parsing on every poll is wasteful, so cache on mtime.
let historyCache = { mtimeMs: -1, map: new Map() };

function readHistory() {
  let stat;
  try {
    stat = fs.statSync(HISTORY_FILE);
  } catch {
    return historyCache.map;
  }
  if (stat.mtimeMs === historyCache.mtimeMs) return historyCache.map;

  const map = new Map();
  const raw = readFileQuiet(HISTORY_FILE) || '';
  for (const line of raw.split('\n')) {
    if (!line) continue;
    const o = safeParse(line);
    if (!o || !o.sessionId) continue;
    const entry = map.get(o.sessionId) || { firstPrompt: null, lastPrompt: null, promptCount: 0 };
    if (!entry.firstPrompt) entry.firstPrompt = o.display;
    entry.lastPrompt = o.display;
    entry.lastTimestamp = o.timestamp;
    entry.project = o.project;
    entry.promptCount += 1;
    map.set(o.sessionId, entry);
  }

  historyCache = { mtimeMs: stat.mtimeMs, map };
  return map;
}

// ----------------------------------------------------------------- transcripts

// Read the last TAIL_BYTES of a transcript without loading the whole file.
function tailRead(file, bytes) {
  let fd;
  try {
    fd = fs.openSync(file, 'r');
    const size = fs.fstatSync(fd).size;
    const len = Math.min(bytes, size);
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, size - len);
    return buf.toString('utf8');
  } catch {
    return '';
  } finally {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch {}
    }
  }
}

// `--fork-session` copies the transcript wholesale under a new id, custom title and
// all, so a fork and its origin otherwise render as two identical cards. The copied
// records keep the *original* session's id in their `sessionId` field while the file
// is named after the new one — so a first line that disagrees with the filename is
// the fork's parent. The first line of a transcript never changes, so cache forever.
const forkCache = new Map(); // sessionId -> parent id | null

function headRead(file, bytes = 8 * 1024) {
  let fd;
  try {
    fd = fs.openSync(file, 'r');
    const len = Math.min(bytes, fs.fstatSync(fd).size);
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, 0);
    return buf.toString('utf8');
  } catch {
    return '';
  } finally {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch {}
    }
  }
}

function forkParent(file, sessionId) {
  if (forkCache.has(sessionId)) return forkCache.get(sessionId);
  const first = headRead(file).split('\n').find(Boolean);
  const o = safeParse(first || '');
  const parent = o && o.sessionId && o.sessionId !== sessionId ? o.sessionId : null;
  forkCache.set(sessionId, parent);
  return parent;
}

// Pull whatever metadata the tail happens to carry. Everything here is best-effort:
// the record shapes are internal to Claude Code and change between versions.
function scanTranscript(file) {
  const chunk = tailRead(file, TAIL_BYTES);
  const lines = chunk.split('\n');
  // first line is probably truncated mid-JSON
  if (lines.length > 1) lines.shift();

  const meta = {
    customTitle: null,
    agentName: null,
    aiTitle: null,
    gitBranch: null,
    cwd: null,
    version: null,
    mode: null,
    permissionMode: null,
  };
  // Scanning backwards means the newest record of each kind wins, which is how
  // Claude Code itself resolves these — a rename appends, it never rewrites.
  for (let i = lines.length - 1; i >= 0; i--) {
    const o = safeParse(lines[i]);
    if (!o) continue;
    if (!meta.customTitle && o.type === 'custom-title' && o.customTitle) meta.customTitle = o.customTitle;
    if (!meta.agentName && o.type === 'agent-name' && o.agentName) meta.agentName = o.agentName;
    if (!meta.aiTitle && o.type === 'ai-title' && o.aiTitle) meta.aiTitle = o.aiTitle;
    if (!meta.mode && o.type === 'mode') meta.mode = o.mode;
    if (!meta.permissionMode && o.type === 'permission-mode') meta.permissionMode = o.permissionMode;
    if (!meta.cwd && o.cwd) {
      meta.cwd = o.cwd;
      meta.gitBranch = o.gitBranch || null;
      meta.version = o.version || null;
    }
    if (meta.customTitle && meta.aiTitle && meta.cwd && meta.mode) break;
  }
  return meta;
}

// The live registry drops `nameSource` entirely once a name is set on purpose
// (`--name`, `/rename`, or a control-rename over the session socket). It only
// writes `nameSource: 'derived'` for the cwd-derived autoname, so anything else
// with a name is a real user-chosen title.
function liveCustomName(l) {
  if (!l || !l.name) return null;
  if (l.nameSource === 'derived') return null;
  // Background agents get their whole dispatch prompt as the registry name, newlines
  // and all — one row is one line, so flatten it rather than let it blow up the list.
  return l.name.replace(/\s+/g, ' ').trim() || null;
}

// ------------------------------------------------------------------ assembly

function listTranscripts() {
  const out = [];
  let dirs = [];
  try {
    dirs = fs.readdirSync(PROJECTS_DIR, { withFileTypes: true });
  } catch {
    return out;
  }

  for (const d of dirs) {
    if (!d.isDirectory()) continue;
    const dir = path.join(PROJECTS_DIR, d.name);
    let files = [];
    try {
      files = fs.readdirSync(dir);
    } catch {
      continue;
    }
    for (const f of files) {
      if (!f.endsWith('.jsonl')) continue;
      const full = path.join(dir, f);
      let stat;
      try {
        stat = fs.statSync(full);
      } catch {
        continue;
      }
      if (stat.size < 200) continue; // empty/aborted session
      out.push({
        sessionId: path.basename(f, '.jsonl'),
        file: full,
        slug: d.name,
        mtimeMs: stat.mtimeMs,
        size: stat.size,
      });
    }
  }

  out.sort((a, b) => b.mtimeMs - a.mtimeMs);
  return out;
}

function list(currentCwd) {
  const live = readLive();
  const history = readHistory();
  const transcripts = listTranscripts().slice(0, MAX_SESSIONS);
  const seen = new Set();
  const rows = [];

  for (const t of transcripts) {
    const meta = scanTranscript(t.file);
    const l = live.get(t.sessionId);
    const h = history.get(t.sessionId) || {};
    const cwd = l?.cwd || meta.cwd || h.project || null;

    // A live session's registry entry is fresher than its transcript, so it wins;
    // the transcript records are what survive after the process is gone.
    const custom = liveCustomName(l) || meta.customTitle || meta.agentName;

    seen.add(t.sessionId);
    rows.push({
      sessionId: t.sessionId,
      // custom name beats AI title beats first prompt beats the raw id
      title: custom || meta.aiTitle || h.firstPrompt || t.sessionId.slice(0, 8),
      titleSource: custom ? 'name' : meta.aiTitle ? 'ai' : h.firstPrompt ? 'prompt' : 'id',
      liveName: l?.name || null,
      customTitle: custom || null,
      aiTitle: meta.aiTitle || null,
      file: t.file,
      forkOf: forkParent(t.file, t.sessionId),
      intent: h.firstPrompt || null,
      lastPrompt: h.lastPrompt || null,
      promptCount: h.promptCount || 0,
      cwd,
      project: cwd ? path.basename(cwd) : t.slug,
      gitBranch: meta.gitBranch,
      version: l?.version || meta.version,
      mode: meta.mode,
      permissionMode: meta.permissionMode,
      updatedAt: t.mtimeMs,
      startedAt: l?.startedAt || null,
      size: t.size,
      live: Boolean(l),
      status: l?.status || null,
      kind: l?.kind || null,
      jobId: l?.jobId || null,
      pid: l?.pid || null,
      socket: l?.socket || null,
      isCurrentProject: Boolean(currentCwd && cwd === currentCwd),
    });
  }

  // A live session can be younger than its transcript flush, or have no transcript
  // yet at all — surface it anyway so the sidebar never misses a running agent.
  for (const [sessionId, l] of live) {
    if (seen.has(sessionId)) continue;
    rows.push({
      sessionId,
      title: l.name || sessionId.slice(0, 8),
      titleSource: liveCustomName(l) ? 'name' : 'id',
      liveName: l.name,
      customTitle: liveCustomName(l),
      aiTitle: null,
      file: null,
      forkOf: null,
      intent: null,
      lastPrompt: null,
      promptCount: 0,
      cwd: l.cwd,
      project: l.cwd ? path.basename(l.cwd) : '?',
      gitBranch: null,
      version: l.version,
      mode: null,
      permissionMode: null,
      updatedAt: l.updatedAt || l.startedAt,
      startedAt: l.startedAt,
      size: 0,
      live: true,
      status: l.status,
      kind: l.kind || null,
      jobId: l.jobId || null,
      pid: l.pid,
      socket: l.socket,
      isCurrentProject: Boolean(currentCwd && l.cwd === currentCwd),
    });
  }

  rows.sort((a, b) => {
    if (a.live !== b.live) return a.live ? -1 : 1;
    return b.updatedAt - a.updatedAt;
  });
  return rows;
}

// Fire `cb` when session state changes, debounced — writes land in bursts.
function watch(cb, intervalMs = 4000) {
  let timer = null;
  const ping = () => {
    clearTimeout(timer);
    timer = setTimeout(cb, 250);
  };

  const watchers = [];
  for (const dir of [SESSIONS_DIR, PROJECTS_DIR]) {
    try {
      watchers.push(fs.watch(dir, { recursive: dir === PROJECTS_DIR }, ping));
    } catch {}
  }
  try {
    watchers.push(fs.watch(HISTORY_FILE, ping));
  } catch {}

  // fs.watch misses status flips inside sessions/*.json on some macOS setups
  const poll = setInterval(cb, intervalMs);

  return () => {
    clearTimeout(timer);
    clearInterval(poll);
    for (const w of watchers) {
      try {
        w.close();
      } catch {}
    }
  };
}

module.exports = { list, watch, CLAUDE_DIR };
