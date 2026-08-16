// Every OS branch in the app lives here.
//
// The app is written against macOS — vibrancy behind the terminal, traffic lights over the
// sidebar, `ps` for the pid ancestry walk, a unix socket per session. None of that is
// portable, and scattering `process.platform` checks through main.js/renderer.js would put
// the same three-way decision in six places. So each branch is one exported function, and
// the callers stay platform-blind.
//
// Rule for everything in here: an unsupported platform gets a *working* app with less
// information, never a throw. The one exception is `parentMap`, which reports failure
// instead of swallowing it — see below.

const os = require('os');
const fs = require('fs');
const { execFileSync } = require('child_process');
const { HEADER_HEIGHT } = require('./titlebar');

const PLATFORM = os.platform();
const isMac = PLATFORM === 'darwin';
const isWindows = PLATFORM === 'win32';

// ------------------------------------------------------------------ window chrome

// macOS is the only platform with NSVisualEffectView, and the whole visual design leans on
// it: the body is tinted at 27% alpha (index.html) and the OS supplies everything behind it.
// Ship that same markup on Windows or Linux and you get a near-invisible window, not a
// slightly flatter one — so vibrancy and window transparency are one decision, made here,
// and the renderer paints an opaque background when it's off.
function usesVibrancy() {
  return isMac;
}

// Spread into the BrowserWindow constructor.
function windowOptions() {
  if (isMac) {
    return {
      titleBarStyle: 'hiddenInset',
      trafficLightPosition: { x: 16, y: HEADER_HEIGHT / 2 - 6 },
      transparent: true,
      vibrancy: 'under-window',
      visualEffectState: 'active',
    };
  }

  // Frameless everywhere else, because the sidebar header is the drag region and a native
  // title bar above it would be a second, redundant strip. Windows gets the system window
  // controls drawn into that strip via titleBarOverlay; Linux has no equivalent overlay, so
  // it is frameless and moved by dragging the sidebar.
  if (isWindows) {
    return {
      titleBarStyle: 'hidden',
      titleBarOverlay: { color: '#1b1e24', symbolColor: '#abb2bf', height: HEADER_HEIGHT },
      backgroundColor: '#0f1116',
    };
  }

  return { frame: false, backgroundColor: '#0f1116' };
}

// The traffic lights sit *over* the sidebar on macOS, which is what the sidebar header's
// left pad reserves room for. Nothing overlaps the left edge anywhere else, and Windows'
// overlay controls sit top-right, so that pad is dead space off macOS.
function reservesTitleBarInset() {
  return isMac;
}

// ------------------------------------------------------------------- pid ancestry

function parseUnixPs(out) {
  const map = new Map();
  for (const line of out.split('\n')) {
    const [pid, ppid] = line.trim().split(/\s+/).map(Number);
    if (pid) map.set(pid, ppid);
  }
  return map;
}

// PowerShell's CSV has a quoted header row then one quoted pair per process.
function parseWindowsCsv(out) {
  const map = new Map();
  for (const line of out.split('\n')) {
    const cells = line.trim().split(',').map((c) => Number(c.replace(/^"|"$/g, '')));
    if (cells.length < 2) continue;
    const [pid, ppid] = cells;
    if (Number.isFinite(pid) && Number.isFinite(ppid) && pid) map.set(pid, ppid);
  }
  return map;
}

// pid -> ppid for every process on the machine, in one call.
//
// This one reports failure rather than degrading quietly. Binding a session row to the tab
// hosting it is an ancestry walk over this map, and everything downstream of that binding —
// the row's close button, autonaming, "which tab is this session in" — simply stops with no
// symptom if the map comes back empty. A silent no-op is the worst possible failure here,
// so the caller gets `ok` and can say so.
function parentMap() {
  try {
    if (isWindows) {
      const out = execFileSync(
        'powershell.exe',
        [
          '-NoLogo',
          '-NoProfile',
          '-Command',
          'Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId | ConvertTo-Csv -NoTypeInformation',
        ],
        { encoding: 'utf8', timeout: 5000, windowsHide: true }
      );
      return { map: parseWindowsCsv(out), ok: true };
    }
    const out = execFileSync('ps', ['-Ao', 'pid=,ppid='], { encoding: 'utf8', timeout: 5000 });
    return { map: parseUnixPs(out), ok: true };
  } catch (err) {
    return { map: new Map(), ok: false, error: String((err && err.message) || err) };
  }
}

// ------------------------------------------------------------------------ shells

// Quoting a value for the shell that `shellCommand` picked. These are not interchangeable:
// POSIX ends the quote, escapes the quote, reopens; PowerShell doubles it in place. Using
// the POSIX form on PowerShell produces an argument containing literal backslashes, which
// is how a folder with an apostrophe in it silently launches in the wrong directory.
function quoteArg(s) {
  const str = String(s);
  if (isWindows) return `'${str.replace(/'/g, "''")}'`;
  return `'${str.replace(/'/g, `'\\''`)}'`;
}

// The pty runs Claude Code *itself*, not a shell someone then types `claude` into.
//
// The login shell is still worth paying for: it is the only reason `claude` and the rest of
// the user's toolchain are on PATH. But `exec` hands the pty over, so quitting Claude Code
// ends the terminal instead of dropping the user at a bare prompt in a tab that no longer
// stands for anything. PowerShell has no `exec`, so Windows keeps the host process — the
// tab still closes on exit, one process later.
function shellCommand(cmd) {
  if (isWindows) {
    const bin = process.env.COMSPEC && /pwsh/i.test(process.env.COMSPEC) ? 'pwsh.exe' : 'powershell.exe';
    // No -NoProfile on purpose: the profile is the Windows analogue of `-l`, and it is
    // where a user's PATH edits tend to live.
    return { bin, args: ['-NoLogo', '-Command', cmd] };
  }
  const bin = process.env.SHELL || '/bin/bash';
  return { bin, args: ['-l', '-c', `exec ${cmd}`] };
}

// Same login shell, but for reading a command's output rather than handing it the pty —
// so no `exec`, which would replace the shell before a builtin like `command -v` could run.
// The login shell is the point: PATH is whatever the user's profile builds, and that is the
// only reason `claude` is findable at all.
function shellCapture(cmd) {
  if (isWindows) {
    const bin = process.env.COMSPEC && /pwsh/i.test(process.env.COMSPEC) ? 'pwsh.exe' : 'powershell.exe';
    return { bin, args: ['-NoLogo', '-Command', cmd] };
  }
  return { bin: process.env.SHELL || '/bin/bash', args: ['-l', '-c', cmd] };
}

// ------------------------------------------------------------------------ sockets

// Claude Code hands us the control socket path in its own pid registry, so we never build
// one — but we do have to decide whether it is connectable before trying. On Windows that
// path would be a named pipe, and `fs.existsSync` returns false for a live named pipe, so
// the existence check that guards the unix path would permanently route Windows down the
// transcript fallback. Unverified either way: no Windows machine has been checked, and the
// CLI may not expose this channel there at all.
function canUseSocket(socketPath) {
  if (!socketPath || typeof socketPath !== 'string') return false;
  if (isWindows) return /^\\\\[.?]\\pipe\\/i.test(socketPath);
  try {
    return fs.existsSync(socketPath);
  } catch {
    return false;
  }
}

module.exports = {
  PLATFORM,
  isMac,
  isWindows,
  usesVibrancy,
  windowOptions,
  reservesTitleBarInset,
  parentMap,
  quoteArg,
  shellCommand,
  shellCapture,
  canUseSocket,
};
