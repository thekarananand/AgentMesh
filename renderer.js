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

// Canvas metrics give width but not line height, and xterm's row height is the font's
// natural line box at lineHeight 1 — so measure that with a throwaway span rather than
// guessing a ratio off the font size.
function measureCellHeight() {
  const probe = document.createElement('span');
  probe.textContent = 'M';
  probe.style.cssText = `position:absolute;visibility:hidden;white-space:pre;font:${FONT_SIZE}px/1 ${FONT_FAMILY}`;
  document.body.appendChild(probe);
  const h = probe.getBoundingClientRect().height;
  probe.remove();
  return Math.round(h) || FONT_SIZE;
}

const CELL_W = measureCellWidth();
const CELL_H = measureCellHeight();
document.documentElement.style.setProperty('--cell-w', `${CELL_W}px`);
document.documentElement.style.setProperty('--cell-h', `${CELL_H}px`);

function el(tag, className, text) {
  const n = document.createElement(tag);
  if (className) n.className = className;
  if (text !== undefined) n.textContent = text;
  return n;
}

document.documentElement.style.setProperty('--header-h', `${window.ptyAPI.headerHeight}px`);

// --------------------------------------------------------------------- tabs
//
// Each tab owns a real pty process in the main process (see main.js `spawnTab`).
// Switching tabs never touches the pty — every terminal keeps running and keeps its
// own scrollback, it's just its xterm host div that toggles hidden.
//
// There is no tab strip. The sidebar is the switcher: it already lists every session
// on the machine and marks the ones we're hosting, so a second row of the same
// sessions along the top was showing the user the same thing twice.

const contentEl = document.getElementById('tabs-content');

const tabs = new Map(); // tabId -> { term, host, sessionId, title }
let activeTabId = null;

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
  const rows = Math.floor((contentEl.clientHeight - 2 * CELL_H) / height);
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
  for (const [id, tab] of tabs) tab.host.classList.toggle('hidden', id !== tabId);
  showWelcome(false);
  syncNewAgentButton();
  tabs.get(tabId).term.focus();
}

function activeCwd() {
  const tab = tabs.get(activeTabId);
  return (tab && tab.cwd) || null;
}

function closeTab(tabId) {
  const tab = tabs.get(tabId);
  if (!tab) return;
  window.ptyAPI.close(tabId);
  tab.term.dispose();
  tab.host.remove();
  tabs.delete(tabId);

  if (activeTabId !== tabId) {
    syncNewAgentButton();
    return;
  }
  activeTabId = null;
  const next = tabs.keys().next().value;
  // Zero terminals is a legitimate state now: quitting Claude Code closes its
  // terminal, and what's left is the same picker the app launches with.
  if (next) setActive(next);
  else showWelcome(true);
}

function openTab(opts) {
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

  window.ptyAPI.create(opts).then((tabId) => {
    tabs.set(tabId, {
      term,
      host,
      sessionId: opts.resumeSessionId || null,
      title: opts.title || null,
      cwd: opts.launchCwd || null,
    });
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
    setActive(tabId);
    fitAll();
  });
}

// ------------------------------------------------------------------ welcome pane

const welcomeEl = document.getElementById('welcome');
const pickDirEl = document.getElementById('pick-dir');
const recentDirsEl = document.getElementById('recent-dirs');

function showWelcome(on) {
  welcomeEl.classList.toggle('hidden', !on);
  if (on) renderRecentDirs();
  syncNewAgentButton();
}

const newAgentEl = document.getElementById('new-agent');
const newAgentWhereEl = newAgentEl.querySelector('.where');

// The button reads as "another agent on *this*" when there's a session to inherit a
// folder from, and as a plain new-session button when there isn't.
function syncNewAgentButton() {
  const dir = activeCwd();
  newAgentWhereEl.textContent = dir ? dir.split('/').filter(Boolean).pop() : '';
  newAgentEl.title = dir
    ? `Start another Claude Code session in ${dir}`
    : 'Start a Claude Code session — pick a folder';
}

// A session's cwd is fixed at spawn and decides which project it belongs to, so the
// folder is asked for up front rather than inherited from wherever the app happened
// to be launched from.
async function newSessionHere(dir) {
  const target = dir || (await window.meshAPI.pickDirectory());
  if (!target) return;
  openTab({ launchCwd: target });
}

pickDirEl.addEventListener('click', () => newSessionHere());
// Header '+' always asks, so there's still a way to start somewhere else entirely.
document.getElementById('new-session').addEventListener('click', () => newSessionHere());
// The button inherits the active session's folder — running a second agent on what
// you're already working on is the common case, and a dialog every time is friction.
newAgentEl.addEventListener('click', () => newSessionHere(activeCwd()));

// Every folder that has ever held a session is already in the sidebar's data, so the
// picker gets a shortcut list for free — no separate history to keep in sync.
function renderRecentDirs() {
  const seen = new Set();
  const dirs = [];
  for (const row of rows) {
    if (!row.cwd || seen.has(row.cwd)) continue;
    seen.add(row.cwd);
    dirs.push(row.cwd);
    if (dirs.length >= 8) break;
  }

  recentDirsEl.replaceChildren();
  if (!dirs.length) return;

  recentDirsEl.appendChild(el('div', 'recent-label', 'RECENT FOLDERS'));
  for (const dir of dirs) {
    const node = el('div', 'recent-dir');
    node.appendChild(el('span', 'name', dir.split('/').filter(Boolean).pop() || dir));
    node.appendChild(el('span', 'path', dir));
    node.title = dir;
    node.addEventListener('click', () => newSessionHere(dir));
    recentDirsEl.appendChild(node);
  }
}

window.ptyAPI.onData((tabId, data) => {
  tabs.get(tabId)?.term.write(data);
});

// The pty is Claude Code itself (main.js execs it), so this fires the moment the user
// quits Claude — the terminal goes with it rather than falling back to a bare shell.
window.ptyAPI.onExit((tabId) => {
  if (tabs.has(tabId)) closeTab(tabId);
});

// The opening state is set at the bottom of this file, once `rows` exists — the
// welcome pane reads it for its recent-folder list.

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

// ------------------------------------------------------------------- renaming
//
// A rename is handed to Claude Code rather than stored here: live sessions take it
// over their control socket, dead ones get it appended to their transcript (see
// rename.js). Either way the name comes back to us through the session watcher, so
// `/rename` typed into a tab and a rename done here can't drift apart.

// The session list repaints on every poll, which would rip the input out from under
// the cursor — hold repaints while an edit is open and flush one when it closes.
let editingSessionId = null;
let renderPending = false;

function endEditing() {
  editingSessionId = null;
  if (renderPending) {
    renderPending = false;
    render();
  }
}

// Swaps `labelEl` for a text input in place; restores it on cancel or failure.
function beginRename(labelEl, sessionId, currentTitle) {
  if (editingSessionId) return;
  editingSessionId = sessionId;

  const input = el('input', 'rename-input');
  input.value = currentTitle || '';
  input.spellcheck = false;
  labelEl.replaceWith(input);
  input.focus();
  input.select();

  let settled = false;
  const restore = () => {
    input.replaceWith(labelEl);
    endEditing();
  };

  const commit = async () => {
    if (settled) return;
    settled = true;
    const name = input.value.trim();
    if (!name || name === currentTitle) {
      restore();
      return;
    }
    labelEl.textContent = name; // optimistic; the watcher confirms or corrects it
    input.replaceWith(labelEl);
    const res = await window.meshAPI.rename(sessionId, name);
    if (!res || !res.ok) {
      labelEl.textContent = currentTitle;
      labelEl.classList.add('rename-failed');
      setTimeout(() => labelEl.classList.remove('rename-failed'), 1200);
    }
    endEditing();
  };

  const cancel = () => {
    if (settled) return;
    settled = true;
    restore();
  };

  input.addEventListener('keydown', (e) => {
    e.stopPropagation();
    if (e.key === 'Enter') commit();
    else if (e.key === 'Escape') cancel();
  });
  input.addEventListener('blur', commit);
  input.addEventListener('click', (e) => e.stopPropagation());
  input.addEventListener('dblclick', (e) => e.stopPropagation());
}

// Where the session is and what it's working on — not how old it is or what pid it
// happens to have. Both were bookkeeping the user has no use for.
function meta(row) {
  const parts = [];
  if (row.live && row.kind === 'bg') parts.push('background');
  if (row.project) parts.push(row.project);
  if (row.gitBranch && row.gitBranch !== 'HEAD') parts.push(row.gitBranch);
  if (row.promptCount) parts.push(`${row.promptCount} prompts`);

  const n = el('div', 'row-meta');
  parts.forEach((p, i) => {
    if (i) n.appendChild(el('span', 'sep', '·'));
    n.appendChild(document.createTextNode(p));
  });
  return n;
}

// A one-off message on the row itself. Used where a click can't do the obvious thing
// and the reason is worth a sentence — no dialog, no toast layer.
function flashRow(node, text) {
  const existing = node.querySelector('.row-hint');
  if (existing) existing.remove();
  const hint = el('div', 'row-hint', text);
  node.appendChild(hint);
  setTimeout(() => hint.remove(), 2600);
}

// A live session is already owned by the process driving it, and `--resume` is not an
// attach: on an interactive session it starts a *second* writer on one transcript, and
// on a background agent the CLI refuses it outright ("is currently running as a
// background agent … add --fork-session to branch off a copy"). So a live row only ever
// resumes when this window is the one hosting it; otherwise it routes to the CLI's own
// attach path, or forks on request.
function activateRow(row, node, { fork } = {}) {
  if (row.live) {
    if (row.kind === 'bg') {
      openTab({ launchCwd: row.cwd, agentsView: true, title: 'agents' });
      return;
    }
    if (!fork) {
      flashRow(node, 'Running in another window — ⌥click to fork a copy');
      return;
    }
  }

  openTab({
    resumeSessionId: row.sessionId,
    resumeName: row.customTitle, // only a real name carries over, never an AI title
    forkSession: Boolean(fork && row.live),
    title: row.title,
    launchCwd: row.cwd,
  });
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

  if (row.tabId && tabs.has(row.tabId)) {
    node.classList.add('open');
    const close = el('span', 'row-close', '×');
    close.title = 'Close this terminal';
    close.addEventListener('click', (e) => {
      e.stopPropagation();
      closeTab(row.tabId);
    });
    node.appendChild(close);
  }

  // Double-click renames, so a click that would *spawn* a tab has to wait long enough
  // to find out whether a second click is coming — otherwise every rename leaves a
  // stray `claude --resume` behind. Focusing an already-open tab is free and
  // idempotent, so that path stays instant and skips the delay entirely.
  let openTimer = null;
  node.addEventListener('click', (e) => {
    if (row.tabId && tabs.has(row.tabId)) {
      setActive(row.tabId);
      return;
    }
    const fork = e.altKey;
    clearTimeout(openTimer);
    openTimer = setTimeout(() => {
      openTimer = null;
      activateRow(row, node, { fork });
    }, 220);
  });

  node.addEventListener('dblclick', (e) => {
    e.preventDefault();
    clearTimeout(openTimer);
    openTimer = null;
    beginRename(label, row.sessionId, row.title);
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

// Bind each terminal to the session actually running in it, once that session
// registers itself. A tab opened on a blank folder doesn't know its session id yet.
function syncTabs(visible) {
  for (const row of visible) {
    const tab = row.tabId && tabs.get(row.tabId);
    if (!tab) continue;
    tab.sessionId = row.sessionId;
    tab.title = row.title;
  }
}

function render() {
  if (editingSessionId) {
    renderPending = true;
    return;
  }
  const visible = projectOnly ? rows.filter((r) => r.isCurrentProject) : rows;
  listEl.replaceChildren();
  syncTabs(visible);
  if (!welcomeEl.classList.contains('hidden')) renderRecentDirs();

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

showWelcome(true); // nothing is spawned until a folder is picked
