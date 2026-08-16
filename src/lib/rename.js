// Renaming a Claude Code session, in sync with the CLI itself.
//
// Two paths, picked by whether the session is still running:
//
//   live     — one JSON line to the session's own UDS control socket
//              (`messagingSocketPath` in ~/.claude/sessions/<pid>.json). Claude Code
//              handles it in-process: prompt box, terminal title, the pid registry and
//              the transcript all update at once. Same pipeline `/rename` uses.
//   archived — no socket to talk to, so append the two records the CLI would have
//              written. `claude --resume` reads them back, so the name survives and
//              shows up in the CLI's own resume picker.
//
// Neither path keeps state of its own: Claude Code's files stay the single source of
// truth, which is what makes a rename from the sidebar and a `/rename` typed into a
// tab impossible to disagree.

const fs = require('fs');
const net = require('net');
const { canUseSocket } = require('./platform');

const CONNECT_TIMEOUT_MS = 1500;
const MAX_NAME = 200;

// The wire format is newline-delimited JSON, and JSON.stringify escapes any newline
// inside the value — but a name with raw control characters in it is still nobody's
// intent, so flatten to a single line before it ever reaches the socket.
function normalize(name) {
  if (typeof name !== 'string') return null;
  const clean = name.replace(/[\r\n\t]+/g, ' ').trim().slice(0, MAX_NAME);
  return clean.length ? clean : null;
}

function pidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

// `session_id` is optional in the protocol, but Claude Code validates it when present
// and drops the message on a mismatch. Always send it: a pid file can outlive its
// process, and the socket path is keyed on a pid the OS is free to reuse.
function renameLive(socketPath, sessionId, name) {
  return new Promise((resolve) => {
    let settled = false;
    const done = (ok, error) => {
      if (settled) return;
      settled = true;
      resolve({ ok, error: error || null });
    };

    let sock;
    try {
      sock = net.createConnection(socketPath);
    } catch (err) {
      done(false, String(err && err.message));
      return;
    }

    const timer = setTimeout(() => {
      sock.destroy();
      done(false, 'socket timeout');
    }, CONNECT_TIMEOUT_MS);
    timer.unref?.();

    sock.on('error', (err) => {
      clearTimeout(timer);
      done(false, String(err && err.message));
    });

    sock.on('connect', () => {
      const line = JSON.stringify({ type: 'control', action: 'rename', name, session_id: sessionId }) + '\n';
      // No ack exists in the protocol — a flushed write is the strongest signal we get.
      // Confirmation arrives out of band when the CLI rewrites its pid file and the
      // session watcher fires.
      sock.end(line, () => {
        clearTimeout(timer);
        done(true);
      });
    });
  });
}

function renameArchived(file, sessionId, name) {
  try {
    const records =
      JSON.stringify({ type: 'custom-title', customTitle: name, sessionId }) +
      '\n' +
      JSON.stringify({ type: 'agent-name', agentName: name, sessionId }) +
      '\n';

    // Transcripts are append-only and normally newline-terminated, but a session
    // killed mid-write can leave a partial last line — don't glue onto it.
    let prefix = '';
    try {
      const { size } = fs.statSync(file);
      if (size > 0) {
        const fd = fs.openSync(file, 'r');
        try {
          const buf = Buffer.alloc(1);
          fs.readSync(fd, buf, 0, 1, size - 1);
          if (buf[0] !== 0x0a) prefix = '\n';
        } finally {
          fs.closeSync(fd);
        }
      }
    } catch {}

    fs.appendFileSync(file, prefix + records);
    return { ok: true, error: null };
  } catch (err) {
    return { ok: false, error: String(err && err.message) };
  }
}

// `row` is a session row as produced by sessions.list().
async function rename(row, rawName) {
  const name = normalize(rawName);
  if (!name) return { ok: false, via: null, error: 'empty name' };
  if (!row) return { ok: false, via: null, error: 'unknown session' };

  // `canUseSocket` rather than `fs.existsSync`: a live named pipe on Windows does not exist
  // as far as `existsSync` is concerned, so the plain check would route every Windows rename
  // down the transcript path forever, whether or not the CLI is listening.
  if (row.socket && row.pid && pidAlive(row.pid) && canUseSocket(row.socket)) {
    const res = await renameLive(row.socket, row.sessionId, name);
    if (res.ok) return { ok: true, via: 'socket', name, error: null };
    // Fall through: a live session that won't take the message still has a
    // transcript, and a name written there beats no rename at all.
    if (!row.file) return { ok: false, via: 'socket', error: res.error };
  }

  if (!row.file) return { ok: false, via: null, error: 'no socket and no transcript' };
  const res = renameArchived(row.file, row.sessionId, name);
  return { ok: res.ok, via: 'transcript', name, error: res.error };
}

module.exports = { rename, normalize };
