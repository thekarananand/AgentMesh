const { contextBridge, ipcRenderer, webUtils } = require('electron');
const { HEADER_HEIGHT } = require('./titlebar');
const platform = require('./platform');

contextBridge.exposeInMainWorld('ptyAPI', {
  create: (opts) => ipcRenderer.invoke('pty-create', opts),
  onData: (callback) => ipcRenderer.on('pty-data', (event, { tabId, data }) => callback(tabId, data)),
  onExit: (callback) => ipcRenderer.on('pty-exit', (event, { tabId }) => callback(tabId)),
  sendInput: (tabId, data) => ipcRenderer.send('pty-input', { tabId, data }),
  resize: (tabId, cols, rows) => ipcRenderer.send('pty-resize', { tabId, cols, rows }),
  close: (tabId) => ipcRenderer.send('pty-close', { tabId }),
  headerHeight: HEADER_HEIGHT,
});

// The renderer's two platform-shaped decisions, resolved in the main process so the page
// never has to ask what OS it's on: whether the OS is painting a blurred backdrop behind
// the terminal (if not, the page has to paint an opaque one itself), and whether anything
// overlaps the sidebar's top-left corner (macOS traffic lights).
contextBridge.exposeInMainWorld('platformAPI', {
  vibrancy: platform.usesVibrancy(),
  titleBarInset: platform.reservesTitleBarInset(),
});

contextBridge.exposeInMainWorld('meshAPI', {
  list: () => ipcRenderer.invoke('sessions:list'),
  onUpdate: (callback) => ipcRenderer.on('sessions-update', (event, rows) => callback(rows)),
  // Degradations worth naming rather than leaving as a feature that quietly stopped.
  onWarning: (callback) => ipcRenderer.on('app-warning', (event, warning) => callback(warning)),
  reveal: (sessionId) => ipcRenderer.send('sessions:reveal', sessionId),
  rename: (sessionId, name) => ipcRenderer.invoke('sessions:rename', { sessionId, name }),
  pickDirectory: () => ipcRenderer.invoke('dialog:pick-directory'),

  // Which Claude Code binary every tab will run, and whether there is one at all.
  dirExists: (dir) => ipcRenderer.invoke('fs:dir-exists', dir),
  findFiles: (filenames) => ipcRenderer.invoke('fs:find-files', filenames),

  // `File.path` was removed from the DOM File object (superseded by this call) — dropped
  // files and folders alike only carry a real fs path through webUtils now.
  getPathForFile: (file) => webUtils.getPathForFile(file),

  claudeInfo: () => ipcRenderer.invoke('claude:info'),
  onClaudeInfo: (callback) => ipcRenderer.on('claude-info', (event, info) => callback(info)),

  // Normalized percentages only — the OAuth token never leaves the main process.
  usage: () => ipcRenderer.invoke('usage:get'),
  refreshUsage: () => ipcRenderer.invoke('usage:refresh'),
  onUsageUpdate: (callback) => ipcRenderer.on('usage-update', (event, value) => callback(value)),
});
