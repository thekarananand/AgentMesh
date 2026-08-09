const THEME = {
  background: '#08090900',
  foreground: '#abb2bf',
  cursor: '#abb2bf',
  cursorAccent: '#080909',
  selectionBackground: '#555a6345',
  black: '#3f4451',
  brightBlack: '#4f5666',
  red: '#e05561',
  brightRed: '#ff616e',
  green: '#8cc265',
  brightGreen: '#a5e075',
  yellow: '#d18f52',
  brightYellow: '#f0a45d',
  blue: '#4aa5f0',
  brightBlue: '#4dc4ff',
  magenta: '#c162de',
  brightMagenta: '#de73ff',
  cyan: '#42b3c2',
  brightCyan: '#4cd1e0',
  white: '#d7dae0',
  brightWhite: '#e6e6e6',
};

const FONT_FAMILY = '"JetBrainsMono Nerd Font Mono", monospace';
const FONT_SIZE = 13;

// Canvas metrics, not DOM layout — this has to resolve before the first term.open(),
// since xterm sizes itself against the container box at open time.
function measureCellWidth() {
  const ctx = document.createElement('canvas').getContext('2d');
  ctx.font = `${FONT_SIZE}px ${FONT_FAMILY}`;
  return ctx.measureText('M').width;
}

const CELL_W = measureCellWidth();
document.documentElement.style.setProperty('--cell-w', `${CELL_W}px`);

function el(tag, className, text) {
  const n = document.createElement(tag);
  if (className) n.className = className;
  if (text !== undefined) n.textContent = text;
  return n;
}

// --------------------------------------------------------------------- tabs
//
// Each tab owns a real pty process in the main process (see main.js `spawnTab`).
// Switching tabs never touches the pty — every tab's shell keeps running and
// keeps its own scrollback, it's just its xterm host div that toggles hidden.

const tabbarEl = document.getElementById('tabbar');
const tabNewEl = document.getElementById('tab-new');
const contentEl = document.getElementById('tabs-content');

const tabs = new Map(); // tabId -> { term, host, tabEl }
let activeTabId = null;
let tabCounter = 0;

function measureCell(term) {
  const core = term._core;
  return {
    width: core._renderService.dimensions.css.cell.width,
    height: core._renderService.dimensions.css.cell.height,
  };
}

function fitAll() {
  if (!tabs.size) return;
  const any = tabs.get(activeTabId) || tabs.values().next().value;
  const { width, height } = measureCell(any.term);
  const cols = Math.floor((contentEl.clientWidth - 2 * CELL_W) / width);
  const rows = Math.floor(contentEl.clientHeight / height);
  if (cols < 2 || rows < 2) return;
  for (const [tabId, tab] of tabs) {
    tab.term.resize(cols, rows);
    window.ptyAPI.resize(tabId, cols, rows);
  }
}
window.addEventListener('resize', fitAll);

function setActive(tabId) {
  if (!tabs.has(tabId)) return;
  activeTabId = tabId;
  for (const [id, tab] of tabs) {
    tab.host.classList.toggle('hidden', id !== tabId);
    tab.tabEl.classList.toggle('active', id === tabId);
  }
  tabs.get(tabId).term.focus();
}

function closeTab(tabId) {
  const tab = tabs.get(tabId);
  if (!tab) return;
  window.ptyAPI.close(tabId);
  tab.term.dispose();
  tab.host.remove();
  tab.tabEl.remove();
  tabs.delete(tabId);

  if (activeTabId === tabId) {
    const next = tabs.keys().next().value;
    if (next) setActive(next);
    else openTab({}); // never leave zero tabs open
  }
}

function openTab(opts) {
  tabCounter += 1;
  const label = opts.title || `session ${tabCounter}`;

  const host = el('div', 'term-host hidden');
  contentEl.appendChild(host);

  const term = new Terminal({
    cursorBlink: true,
    fontFamily: FONT_FAMILY,
    fontSize: FONT_SIZE,
    lineHeight: 1,
    letterSpacing: 0,
    theme: THEME,
  });
  term.open(host);

  const tabEl = el('div', 'tab');
  const labelEl = el('span', 'tab-label', label);
  const closeEl = el('span', 'tab-close', '×');
  tabEl.appendChild(labelEl);
  tabEl.appendChild(closeEl);
  tabbarEl.insertBefore(tabEl, tabNewEl);

  window.ptyAPI.create(opts).then((tabId) => {
    tabs.set(tabId, { term, host, tabEl, labelEl });
    term.onData((data) => window.ptyAPI.sendInput(tabId, data));

    // Shift+Enter (and Ctrl+Enter) insert a newline instead of submitting. xterm sends
    // a bare CR for both, which Claude Code reads as "send", so intercept and emit
    // ESC+CR — alt/option-Enter, which Claude Code's input natively treats as a line
    // break with no terminal-side keybinding required.
    //
    // preventDefault() is load-bearing: xterm's _keyDown clears _keyDownHandled BEFORE
    // consulting this handler and returns early without suppressing the browser default,
    // so _keyPress still fires and emits charCode 13 (a bare CR) right after our ESC+CR.
    // Claude Code then sees the newline immediately followed by a submit. Killing the
    // default keydown stops Chromium from ever dispatching that keypress. The keypress
    // branch below is a belt-and-braces guard for engines that dispatch it anyway.
    term.attachCustomKeyEventHandler((e) => {
      if (e.key !== 'Enter' || e.altKey || e.metaKey) return true;
      if (!e.shiftKey && !e.ctrlKey) return true;
      if (e.type === 'keypress') return false;
      if (e.type !== 'keydown') return true;
      e.preventDefault();
      window.ptyAPI.sendInput(tabId, '\x1b\r');
      return false;
    });
    tabEl.addEventListener('click', () => setActive(tabId));
    closeEl.addEventListener('click', (e) => {
      e.stopPropagation();
      closeTab(tabId);
    });
    setActive(tabId);
    fitAll();
  });
}

tabNewEl.addEventListener('click', () => openTab({}));

window.ptyAPI.onData((tabId, data) => {
  tabs.get(tabId)?.term.write(data);
});

window.ptyAPI.onExit((tabId) => {
  if (tabs.has(tabId)) closeTab(tabId);
});

openTab({}); // first tab on launch

// ----------------------------------------------------------- sidebar resizing

const sidebarEl = document.getElementById('sidebar');
const resizerEl = document.getElementById('sidebar-resizer');

const MIN_SIDEBAR = 180;
const MAX_SIDEBAR = 520;

function setSidebarWidth(px) {
  const w = Math.min(MAX_SIDEBAR, Math.max(MIN_SIDEBAR, Math.round(px)));
  sidebarEl.style.flex = `0 0 ${w}px`;
  sidebarEl.style.width = `${w}px`;
  localStorage.setItem('sidebarWidth', String(w));
  return w;
}

const savedWidth = Number(localStorage.getItem('sidebarWidth'));
if (savedWidth) setSidebarWidth(savedWidth);

resizerEl.addEventListener('mousedown', (e) => {
  e.preventDefault();
  document.body.classList.add('resizing');
  resizerEl.classList.add('dragging');

  const onMove = (ev) => setSidebarWidth(ev.clientX);
  const onUp = () => {
    document.removeEventListener('mousemove', onMove);
    document.removeEventListener('mouseup', onUp);
    document.body.classList.remove('resizing');
    resizerEl.classList.remove('dragging');
    fitAll(); // reflow the terminals once, at drop
  };

  document.addEventListener('mousemove', onMove);
  document.addEventListener('mouseup', onUp);
});

// double-click resets to the default width
resizerEl.addEventListener('dblclick', () => {
  setSidebarWidth(248);
  fitAll();
});

// ------------------------------------------------------------------- sidebar

const listEl = document.getElementById('list');
const countEl = document.getElementById('count');
const filterEl = document.getElementById('filter');

let rows = [];
let projectOnly = false;
let recentOpen = false; // history is noise until asked for; live agents are the point

function age(ms) {
  if (!ms) return '';
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return `${Math.floor(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

function meta(row) {
  const parts = [age(row.updatedAt)];
  if (row.project) parts.push(row.project);
  if (row.gitBranch && row.gitBranch !== 'HEAD') parts.push(row.gitBranch);
  if (row.promptCount) parts.push(`${row.promptCount} prompts`);
  if (row.live && row.pid) parts.push(`pid ${row.pid}`);

  const n = el('div', 'row-meta');
  parts.forEach((p, i) => {
    if (i) n.appendChild(el('span', 'sep', '·'));
    n.appendChild(document.createTextNode(p));
  });
  return n;
}

function makeRow(row) {
  const node = el('div', 'row');
  if (row.live) node.classList.add('live');
  if (row.status === 'busy') node.classList.add('busy');
  if (!row.live) node.classList.add('dim');

  const title = el('div', 'row-title');
  const dot = el('span', 'dot');
  if (row.live) dot.classList.add(row.status === 'busy' ? 'busy' : 'idle');
  title.appendChild(dot);

  const label = el('span', null, row.title);
  if (row.titleSource === 'ai') label.classList.add('badge-ai');
  if (row.titleSource === 'name') label.classList.add('badge-name');
  title.appendChild(label);

  node.appendChild(title);
  node.appendChild(meta(row));

  // full metadata on hover — cheap inspector until there's a detail pane
  node.title = [
    row.title,
    `id      ${row.sessionId}`,
    `cwd     ${row.cwd || '?'}`,
    row.intent ? `intent  ${row.intent}` : null,
    row.lastPrompt ? `last    ${row.lastPrompt}` : null,
    row.version ? `version ${row.version}` : null,
    row.permissionMode ? `perms   ${row.permissionMode}` : null,
    row.socket ? `socket  ${row.socket}` : null,
    `size    ${(row.size / 1024).toFixed(0)} KB`,
  ]
    .filter(Boolean)
    .join('\n');

  if (row.tabId && tabs.has(row.tabId)) node.classList.add('open');

  node.addEventListener('click', () => {
    // A live session whose pid resolves to one of our tabs is already on screen —
    // focus it instead of spawning a duplicate `claude --resume` of the same id.
    if (row.tabId && tabs.has(row.tabId)) {
      setActive(row.tabId);
      return;
    }
    openTab({ resumeSessionId: row.sessionId, title: row.title, launchCwd: row.cwd });
  });
  node.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    window.meshAPI.reveal(row.sessionId);
  });
  return node;
}

function makeGroup(label, count, collapsible, open, onToggle) {
  const node = el('div', 'group');
  if (collapsible) {
    node.classList.add('collapsible');
    node.appendChild(el('span', 'chev' + (open ? ' open' : ''), '›'));
    node.addEventListener('click', onToggle);
  }
  node.appendChild(document.createTextNode(`${label} · ${count}`));
  return node;
}

// Tab strip shows generic labels until a session registers; once a row binds to a
// tab we know its real title, so relabel the tab from the session metadata.
function syncTabLabels(visible) {
  for (const row of visible) {
    const tab = row.tabId && tabs.get(row.tabId);
    if (tab) tab.labelEl.textContent = row.title;
  }
}

function render() {
  const visible = projectOnly ? rows.filter((r) => r.isCurrentProject) : rows;
  listEl.replaceChildren();
  syncTabLabels(visible);

  if (!visible.length) {
    const empty = el('div', null, 'No sessions found.');
    empty.id = 'empty';
    listEl.appendChild(empty);
    countEl.textContent = 'SESSIONS';
    return;
  }

  const live = visible.filter((r) => r.live);
  const past = visible.filter((r) => !r.live);

  if (live.length) {
    listEl.appendChild(makeGroup('LIVE', live.length, false));
    live.forEach((r) => listEl.appendChild(makeRow(r)));
  }
  if (past.length) {
    listEl.appendChild(
      makeGroup('RECENT', past.length, true, recentOpen, () => {
        recentOpen = !recentOpen;
        render();
      })
    );
    if (recentOpen) past.forEach((r) => listEl.appendChild(makeRow(r)));
  }

  countEl.textContent = `SESSIONS ${visible.length}`;
}

filterEl.addEventListener('click', () => {
  projectOnly = !projectOnly;
  filterEl.textContent = projectOnly ? 'PROJECT' : 'ALL';
  filterEl.classList.toggle('on', projectOnly);
  render();
});

window.meshAPI.onUpdate((next) => {
  rows = next;
  render();
});

window.meshAPI.list().then((next) => {
  rows = next;
  render();
});

// keep relative ages honest without waiting on a filesystem event
setInterval(render, 30000);
