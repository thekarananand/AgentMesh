const { app, BrowserWindow, ipcMain, shell: electronShell } = require('electron');
const path = require('path');
const os = require('os');
const crypto = require('crypto');
const { execFileSync } = require('child_process');
const pty = require('node-pty');
const { HEADER_HEIGHT } = require('./titlebar');
const sessions = require('./sessions');
const { rename } = require('./rename');
const { autoName } = require('./autoname');

let win;
let unwatch;

const ptys = new Map(); // tabId -> node-pty process
const cwd = process.cwd();
const shellBin = os.platform() === 'win32' ? 'powershell.exe' : (process.env.SHELL || '/bin/bash');

// Strip Claude Code's own session env — if AgentMesh itself was launched from inside
// a `claude` session, these leak in via process.env and make every inner `claude` think
// it's a child/subagent of that outer session, which disables transcript persistence.
const ptyEnv = { ...process.env };
delete ptyEnv.CLAUDE_CODE_CHILD_SESSION;
delete ptyEnv.CLAUDE_CODE_SESSION_ID;
delete ptyEnv.CLAUDE_CODE_MESSAGING_SOCKET;
delete ptyEnv.CLAUDE_CODE_ENTRYPOINT;

// pid -> ppid for every process on the machine, in one `ps` call.
function parentMap() {
  const map = new Map();
  try {
    const out = execFileSync('ps', ['-Ao', 'pid=,ppid='], { encoding: 'utf8' });
    for (const line of out.split('\n')) {
      const [pid, ppid] = line.trim().split(/\s+/).map(Number);
      if (pid) map.set(pid, ppid);
    }
  } catch {}
  return map;
}

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

function listAnnotated() {
  const rows = sessions.list(cwd);
  if (!rows.some((r) => r.live && r.pid)) return rows;

  const shellToTab = new Map();
  for (const [tabId, proc] of ptys) shellToTab.set(proc.pid, tabId);
  const parents = parentMap();

  for (const row of rows) {
    row.tabId = row.live && row.pid ? tabForPid(row.pid, parents, shellToTab) : null;
  }
  return rows;
}

let autoNaming = false;

function pushSessions() {
  if (!win || win.isDestroyed()) return;
  const rows = listAnnotated();
  try {
    win.webContents.send('sessions-update', rows);
  } catch {}

  // Promote a describing title into a real session name for anything still running
  // under its cwd-derived autoname. Guarded against re-entry because a successful
  // rename pushes again — that second push is what repaints the row.
  if (autoNaming) return;
  autoNaming = true;
  autoName(rows, pushSessions).finally(() => {
    autoNaming = false;
  });
}

function shellQuote(s) {
  return `'${String(s).replace(/'/g, `'\\''`)}'`;
}

// Each tab is its own real shell process, not a shared TUI switched with /resume —
// that's what makes tabs independently interruptible and closable.
function spawnTab({ resumeSessionId, resumeName, launchCwd } = {}) {
  const tabId = crypto.randomUUID();
  const proc = pty.spawn(shellBin, [], {
    name: 'xterm-color',
    cols: 80,
    rows: 30,
    cwd: launchCwd || cwd,
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

  // `--resume` restores the custom title into the resume picker on its own, but the
  // live registry name is minted fresh at startup and falls back to the cwd-derived
  // autoname. Passing --name keeps the prompt box and terminal title in agreement
  // with the row that was clicked.
  let cmd = 'claude';
  if (resumeSessionId) cmd += ` --resume ${resumeSessionId}`;
  if (resumeSessionId && resumeName) cmd += ` --name ${shellQuote(resumeName)}`;
  proc.write(cmd + '\r');

  return tabId;
}

function createWindow() {
  win = new BrowserWindow({
    width: 1100,
    height: 700,
    titleBarStyle: 'hiddenInset',
    trafficLightPosition: { x: 16, y: HEADER_HEIGHT / 2 - 6 },
    transparent: true,
    vibrancy: 'under-window',
    visualEffectState: 'active',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      sandbox: false,
    },
  });

  win.loadFile('index.html');

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

  ipcMain.on('sessions:reveal', (event, sessionId) => {
    const row = sessions.list(cwd).find((s) => s.sessionId === sessionId);
    if (row && row.cwd) electronShell.openPath(row.cwd);
  });

  ipcMain.handle('sessions:rename', async (event, { sessionId, name }) => {
    const row = sessions.list(cwd).find((s) => s.sessionId === sessionId);
    const result = await rename(row, name);
    // The live path lands in files the session watcher is already watching, but the
    // archived path only touches a transcript — push either way so the row updates
    // without waiting on the poll.
    pushSessions();
    return result;
  });

  win.webContents.on('did-finish-load', pushSessions);
  unwatch = sessions.watch(pushSessions);

  win.on('closed', () => {
    if (unwatch) unwatch();
    for (const proc of ptys.values()) proc.kill();
    ptys.clear();
    win = null;
  });
}

app.whenReady().then(() => {
  createWindow();
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
