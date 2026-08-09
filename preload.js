const { contextBridge, ipcRenderer } = require('electron');
const { HEADER_HEIGHT } = require('./titlebar');

contextBridge.exposeInMainWorld('ptyAPI', {
  create: (opts) => ipcRenderer.invoke('pty-create', opts),
  onData: (callback) => ipcRenderer.on('pty-data', (event, { tabId, data }) => callback(tabId, data)),
  onExit: (callback) => ipcRenderer.on('pty-exit', (event, { tabId }) => callback(tabId)),
  sendInput: (tabId, data) => ipcRenderer.send('pty-input', { tabId, data }),
  resize: (tabId, cols, rows) => ipcRenderer.send('pty-resize', { tabId, cols, rows }),
  close: (tabId) => ipcRenderer.send('pty-close', { tabId }),
  headerHeight: HEADER_HEIGHT,
});

contextBridge.exposeInMainWorld('meshAPI', {
  list: () => ipcRenderer.invoke('sessions:list'),
  onUpdate: (callback) => ipcRenderer.on('sessions-update', (event, rows) => callback(rows)),
  reveal: (sessionId) => ipcRenderer.send('sessions:reveal', sessionId),
  rename: (sessionId, name) => ipcRenderer.invoke('sessions:rename', { sessionId, name }),
});
