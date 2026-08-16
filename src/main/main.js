const { app, BrowserWindow, ipcMain, dialog, shell: electronShell } = require('electron');
const path = require('path');
const fs = require('fs');
const os = require('os');
const crypto = require('crypto');
const pty = require('node-pty');
const platform = require('../lib/platform');
const config = require('../lib/config');
const claudeCli = require('../lib/claude');
const sessions = require('../lib/sessions');
const { rename } = require('../lib/rename');
const { autoName } = require('../lib/autoname');
const usage = require('../lib/usage');

let win;
let unwatch;
let unpollUsage;

// A GUI launch (Finder/Dock) doesn't source the login shell's rc files the way a
// terminal-launched process does, so LANG/LC_ALL commonly arrive unset here even when
// a real terminal on the same machine has them. That leaves Electron's own process —
// and everything it spawns, node-pty included — running under the POSIX "C" locale,
// which is ASCII-only. Only filling the gap when unset, so an existing locale is never
// second-guessed.
if (!process.env.LANG) process.env.LANG = 'en_US.UTF-8';
if (!process.env.LC_ALL) process.env.LC_ALL = 'en_US.UTF-8';

const ptys = new Map(); // tabId -> node-pty process

// Strip Claude Code's own session env — if AgentMesh itself was launched from inside
// a `claude` session, these leak in via process.env and make every inner `claude` think
// it's a child/subagent of that outer session, which disables transcript persistence.
const ptyEnv = { ...process.env };
delete ptyEnv.CLAUDE_CODE_CHILD_SESSION;
delete ptyEnv.CLAUDE_CODE_SESSION_ID;
delete ptyEnv.CLAUDE_CODE_MESSAGING_SOCKET;
delete ptyEnv.CLAUDE_CODE_ENTRYPOINT;

// A live session's pid is the `claude` process, whose ancestor is the tab's shell.
// Walking up the parent chain is what binds a registry entry to a tab we own.
function tabForPid(pid, parents, shellToTab) {
  let cur = pid;
  for (let hops = 0; cur && hops < 24; hops++) {
    if (shellToTab.has(cur)) return shellToTab.get(cur);
    const next = parents.get(cur);
    if (!next || next === cur) break;
    cur = next;
  }
  return null;
}

// Set once the ancestry walk has failed, so the renderer can say *why* rows stopped binding
// to tabs instead of the feature just quietly not happening. Sticky: one failure is enough
// to distrust the binding, and the poll would otherwise flap the warning on and off.
let pidWalkBroken = null;

function listAnnotated() {
  const rows = sessions.list();
  if (!rows.some((r) => r.live && r.pid)) return rows;

  const shellToTab = new Map();
  for (const [tabId, proc] of ptys) shellToTab.set(proc.pid, tabId);
  const { map: parents, ok, error } = platform.parentMap();
  if (!ok && !pidWalkBroken) pidWalkBroken = error || 'process list unavailable';

  for (const row of rows) {
    row.tabId = row.live && row.pid ? tabForPid(row.pid, parents, shellToTab) : null;
  }
  return rows;
}

let autoNaming = false;
let pidWalkReported = false;

function pushSessions() {
  if (!win || win.isDestroyed()) return;
  const rows = listAnnotated();
  try {
    win.webContents.send('sessions-update', rows);
  } catch {}

  if (pidWalkBroken && !pidWalkReported) {
    pidWalkReported = true;
    try {
      win.webContents.send('app-warning', {
        kind: 'pid-walk',
        message: `Can't read the process list, so sessions won't bind to their tabs (${pidWalkBroken})`,
      });
    } catch {}
  }

  // Promote a describing title into a real session name for anything still running
  // under its cwd-derived autoname. Guarded against re-entry because a successful
  // rename pushes again — that second push is what repaints the row.
  if (autoNaming) return;
  autoNaming = true;
  autoName(rows, pushSessions).finally(() => {
    autoNaming = false;
  });
}

// Plan usage is one account-wide fact, so it rides its own channel rather than being
// stapled onto every session row.
function pushUsage(value) {
  if (!win || win.isDestroyed()) return;
  try {
    win.webContents.send('usage-update', value);
  } catch {}
}

// Each tab is its own real shell process, not a shared TUI switched with /resume —
// that's what makes tabs independently interruptible and closable.
function spawnTab({ resumeSessionId, resumeName, launchCwd, forkSession, agentsView } = {}) {
  const tabId = crypto.randomUUID();

  let cmd = claudeCli.command();
  if (agentsView) {
    // A background agent is driven by the daemon over its own pty host, so there is no
    // resume that attaches to it — `claude agents` (FleetView) is the CLI's only attach
    // path, and --cwd scopes it to the folder the clicked row lives in.
    cmd += ' agents';
    if (launchCwd) cmd += ` --cwd ${platform.quoteArg(launchCwd)}`;
  } else if (resumeSessionId) {
    cmd += ` --resume ${resumeSessionId}`;
    // A fork gets its own session id, so carrying the original's name over would put
    // two differently-backed rows under one label.
    if (forkSession) cmd += ' --fork-session';
    // `--resume` restores the custom title into the resume picker on its own, but the
    // live registry name is minted fresh at startup and falls back to the cwd-derived
    // autoname. Passing --name keeps the prompt box and terminal title in agreement
    // with the row that was clicked.
    else if (resumeName) cmd += ` --name ${platform.quoteArg(resumeName)}`;
  }

  const { bin, args } = platform.shellCommand(cmd);

  const proc = pty.spawn(bin, args, {
    name: 'xterm-color',
    cols: 80,
    rows: 30,
    // Never process.cwd(): launched from Finder or a packaged Dock icon that is `/`, and a
    // session's cwd is what decides which project it belongs to. The folder is asked for up
    // front; home is only the floor when something spawned without one.
    cwd: launchCwd || os.homedir(),
    env: ptyEnv,
  });

  proc.onData((data) => {
    if (win && !win.isDestroyed()) win.webContents.send('pty-data', { tabId, data });
  });

  proc.onExit(() => {
    ptys.delete(tabId);
    if (win && !win.isDestroyed()) win.webContents.send('pty-exit', { tabId });
    pushSessions();
  });

  ptys.set(tabId, proc);
  // One more agent is the moment the burn rate changes, so don't sit on a stale bar
  // until the next poll.
  usage.refresh().then(pushUsage);
  return tabId;
}

function createWindow() {
  win = new BrowserWindow({
    width: 1100,
    height: 700,
    // Frameless-with-vibrancy on macOS, frameless-and-opaque everywhere else — see
    // platform.js for why that is one decision rather than five options.
    ...platform.windowOptions(),
    webPreferences: {
      preload: path.join(__dirname, '../preload/preload.js'),
      contextIsolation: true,
      sandbox: false,
    },
  });

  win.loadFile(path.join(__dirname, '../renderer/index.html'));

  ipcMain.handle('pty-create', (event, opts) => spawnTab(opts || {}));

  ipcMain.on('pty-input', (event, { tabId, data }) => {
    ptys.get(tabId)?.write(data);
  });

  ipcMain.on('pty-resize', (event, { tabId, cols, rows }) => {
    ptys.get(tabId)?.resize(cols, rows);
  });

  ipcMain.on('pty-close', (event, { tabId }) => {
    ptys.get(tabId)?.kill();
    ptys.delete(tabId);
    pushSessions();
  });

  ipcMain.handle('sessions:list', () => listAnnotated());

  ipcMain.handle('claude:info', () => claudeCli.info());

  // The window's folder is remembered across launches as a raw absolute path, and paths
  // move: a repo gets renamed, an external disk isn't mounted, the app is opened on a
  // machine restored from someone else's backup. A folder that no longer exists would scope
  // the list to nothing and spawn sessions into a directory that isn't there.
  ipcMain.handle('fs:dir-exists', (event, dir) => {
    try {
      return typeof dir === 'string' && fs.statSync(dir).isDirectory();
    } catch {
      return false;
    }
  });

  ipcMain.handle('usage:get', () => usage.snapshot());
  ipcMain.handle('usage:refresh', async () => {
    const value = await usage.refresh({ force: true });
    pushUsage(value);
    return value;
  });

  ipcMain.handle('dialog:pick-directory', async () => {
    const res = await dialog.showOpenDialog(win, {
      title: 'Start a session in…',
      buttonLabel: 'Open',
      properties: ['openDirectory', 'createDirectory'],
    });
    return res.canceled || !res.filePaths.length ? null : res.filePaths[0];
  });

  // Search for files by name in common locations (used for drag-and-drop)
  ipcMain.handle('fs:find-files', async (event, filenames) => {
    const searchPaths = [
      os.homedir(),
      path.join(os.homedir(), 'Desktop'),
      path.join(os.homedir(), 'Downloads'),
      os.tmpdir(),
    ];

    const results = {};
    for (const filename of filenames) {
      let found = null;

      for (const searchPath of searchPaths) {
        try {
          const fullPath = path.join(searchPath, filename);
          if (fs.existsSync(fullPath)) {
            found = fullPath;
            break;
          }
        } catch {}
      }

      results[filename] = found;
    }

    return results;
  });

  ipcMain.on('sessions:reveal', (event, sessionId) => {
    const row = sessions.list().find((s) => s.sessionId === sessionId);
    if (row && row.cwd) electronShell.openPath(row.cwd);
  });

  ipcMain.handle('sessions:rename', async (event, { sessionId, name }) => {
    const row = sessions.list().find((s) => s.sessionId === sessionId);
    const result = await rename(row, name);
    // The live path lands in files the session watcher is already watching, but the
    // archived path only touches a transcript — push either way so the row updates
    // without waiting on the poll.
    pushSessions();
    return result;
  });

  // Find the CLI before anyone can click New session. If it isn't there, say so in the one
  // place the user is already looking — spawning a tab that dies at exit 127 is not an
  // error message.
  win.webContents.on('did-finish-load', () => {
    pushSessions();
    claudeCli.resolve({ override: config.get().claudeBin }).then((cli) => {
      if (!win || win.isDestroyed()) return;
      try {
        win.webContents.send('claude-info', cli);
      } catch {}
      if (cli.found) return;
      win.webContents.send('app-warning', {
        kind: 'claude-missing',
        message:
          cli.source === 'config-broken'
            ? `Configured Claude Code path didn't answer: ${cli.path}`
            : "Claude Code CLI not found on this machine's PATH — sessions can't be started.",
      });
    });
  });
  unwatch = sessions.watch(pushSessions);
  unpollUsage = usage.poll(pushUsage);

  // Coming back to the window is when a stale number is most likely to mislead. The
  // refresh floor inside usage.js is what keeps alt-tabbing from turning into traffic.
  win.on('focus', () => usage.refresh().then(pushUsage));

  win.on('closed', () => {
    if (unwatch) unwatch();
    if (unpollUsage) unpollUsage();
    for (const proc of ptys.values()) proc.kill();
    ptys.clear();
    win = null;
  });
}

app.whenReady().then(() => {
  // Before the window, because autoname and the usage poller both read settings the moment
  // the first session push happens.
  config.init(app.getPath('userData'));
  createWindow();
});

app.on('window-all-closed', () => {
  if (!platform.isMac) app.quit();
});

// The counterpart macOS needs: closing the window there keeps the app alive, so without
// this there is no way to get a window back short of quitting and relaunching.
app.on('activate', () => {
  if (!win || win.isDestroyed()) createWindow();
});
