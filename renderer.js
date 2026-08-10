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

let CELL_W = measureCellWidth();
let CELL_H = measureCellHeight();

function applyCellVars() {
  document.documentElement.style.setProperty('--cell-w', `${CELL_W}px`);
  document.documentElement.style.setProperty('--cell-h', `${CELL_H}px`);
}
applyCellVars();

// The font now ships with the app instead of being installed on the machine, and a webfont
// is not guaranteed to be loaded when the first script runs — so the measurement above can
// land on the generic `monospace` fallback, which has different metrics. That would size
// every terminal against a cell the terminal isn't actually drawing in.
//
// So measure again once the face is really there. `document.fonts.load` resolves the
// specific face; `ready` waits for the rest of the page's fonts to settle behind it. In
// practice this is a few milliseconds off local disk, and nothing has opened a terminal
// yet (launch shows the welcome pane, not a tab) — the re-measure is the belt to that
// braces, for the case where a tab is already open.
fontsReady().then(() => {
  const w = measureCellWidth();
  const h = measureCellHeight();
  if (w === CELL_W && h === CELL_H) return;
  CELL_W = w;
  CELL_H = h;
  applyCellVars();
  fitAll();
});

function fontsReady() {
  try {
    return Promise.all([
      document.fonts.load(`${FONT_SIZE}px ${FONT_FAMILY}`),
      document.fonts.load(`bold ${FONT_SIZE}px ${FONT_FAMILY}`),
    ])
      .then(() => document.fonts.ready)
      .catch(() => null);
  } catch {
    return Promise.resolve(null);
  }
}

// Last segment of a path, for display. Splits on both separators because a Windows cwd
// arrives as `C:\Users\…` and a POSIX one as `/Users/…`, and the renderer has no `path`
// module to ask.
function baseName(dir) {
  if (!dir) return '';
  return dir.split(/[\\/]/).filter(Boolean).pop() || dir;
}

function el(tag, className, text) {
  const n = document.createElement(tag);
  if (className) n.className = className;
  if (text !== undefined) n.textContent = text;
  return n;
}

document.documentElement.style.setProperty('--header-h', `${window.ptyAPI.headerHeight}px`);

// Two platform facts the page needs and can't work out for itself, resolved in the main
// process (see platform.js). Applied here rather than in CSS media queries because neither
// one is a media feature — they're what the *window* was constructed with.
if (!window.platformAPI.vibrancy) document.body.classList.add('no-vibrancy');
if (!window.platformAPI.titleBarInset) document.body.classList.add('no-titlebar-inset');

// Degradations the app can't paper over. Deduplicated by `kind` so a poll that keeps
// failing doesn't stack the same sentence, and additive so a second, different problem
// doesn't overwrite the first.
const warningsEl = document.getElementById('warnings');
const warnings = new Map();

function renderWarnings() {
  warningsEl.replaceChildren(...Array.from(warnings.values(), (m) => el('div', 'warning', m)));
}

window.meshAPI.onWarning(({ kind, message }) => {
  if (!message || warnings.has(kind)) return;
  warnings.set(kind, message);
  renderWarnings();
});

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

// Fork parentage, recorded by us. sessions.js infers it from the transcript, which is
// internal state that may well stop carrying the original id — this path doesn't care:
// we forked it, we watched which session id the new tab registered, we know. Persisted
// because a fork outlives the window that made it.
const FORK_LEDGER_MAX = 200;
const forkLedger = new Map(JSON.parse(localStorage.getItem('forkOrigins') || '[]'));
const pendingForks = new Map(); // tabId -> parent sessionId, until the fork registers

function recordFork(sessionId, parentId) {
  forkLedger.delete(sessionId); // re-insert so the newest entry is last for trimming
  forkLedger.set(sessionId, parentId);
  while (forkLedger.size > FORK_LEDGER_MAX) forkLedger.delete(forkLedger.keys().next().value);
  localStorage.setItem('forkOrigins', JSON.stringify([...forkLedger]));
}

const parentOf = (row) => row.forkOf || forkLedger.get(row.sessionId) || null;

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

// Dictation shifts the whole window sideways, and this is why. xterm parks its hidden
// helper textarea *at the cursor cell* (`_syncTextArea` sets left/top/width to that one
// cell) and macOS dictation types the entire utterance into it. The caret immediately
// sits hundreds of pixels right of a box one cell wide, so Chromium scrolls every
// ancestor to reveal it — including `body`, which is `overflow: hidden` and therefore
// has no scrollbar to scroll back with. The UI just stays displaced.
//
// Nothing in this layout is ever meant to scroll horizontally, and vertically only two
// things are: the sidebar list and xterm's own viewport. So snap anything else back to
// the origin as it happens. Capture phase, because these scrolls don't bubble.
const SCROLLERS = '#list, #recent-dirs, .xterm-viewport';
document.addEventListener(
  'scroll',
  (e) => {
    const el = e.target === document ? document.scrollingElement : e.target;
    if (!el || el.nodeType !== 1) return;
    const own = el.matches?.(SCROLLERS);
    if (el.scrollLeft) el.scrollLeft = 0;
    if (!own && el.scrollTop) el.scrollTop = 0;
  },
  true
);

function setActive(tabId) {
  if (!tabs.has(tabId)) return;
  activeTabId = tabId;
  for (const [id, tab] of tabs) tab.host.classList.toggle('hidden', id !== tabId);
  showWelcome(false);
  // Switching to a session in another repo is moving the window there — the header and
  // the scoped list follow, or the tab you just opened could be filtered out of the list.
  setWindowDir(tabs.get(tabId).cwd || windowDir);
  syncNewAgentButton();
  render(); // the sidebar carries a card per hosted tab, so which one is active shows there
  tabs.get(tabId).term.focus();
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
    render();
    return;
  }
  activeTabId = null;
  const next = tabs.keys().next().value;
  // Zero terminals is a legitimate state now: quitting Claude Code closes its
  // terminal, and what's left is the same picker the app launches with.
  if (next) setActive(next);
  else {
    showWelcome(true);
    render();
  }
}

// xterm measures its cell against the container at `open()` time, so opening a terminal
// before the bundled font has loaded bakes the fallback's metrics into that terminal for
// good — a resize won't fix it, because the font it re-measures is now the right one but
// the geometry it was built with isn't. Waiting costs nothing after the first tab: the
// promise is already settled and this is one microtask.
function openTab(opts) {
  fontsReady().then(() => createTerminal(opts));
}

function createTerminal(opts) {
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
    // The fork's own session id doesn't exist yet — it registers seconds later. Hold the
    // parent against the tab and bind it in syncTabs once the row shows up.
    if (opts.forkSession && opts.resumeSessionId) pendingForks.set(tabId, opts.resumeSessionId);
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
    // Whatever row was pulsing got what it asked for — a fork mints a new session id, so
    // the row that was clicked will never bind to this tab and can't clear itself.
    for (const n of listEl.querySelectorAll('.row.pending')) n.classList.remove('pending');
    setActive(tabId);
    fitAll();
  });
}

// --------------------------------------------------------------- the window's folder
//
// One piece of state answering "where is this window pointed": it names what `New session`
// starts, and what the list is scoped to. It follows the active tab, because switching to
// a session in another repo *is* moving the window there — but it also stands on its own
// with no tabs open, which is what makes the header control a folder switcher rather than
// a disguised session launcher.

const dirEl = document.getElementById('dir');
const dirNameEl = dirEl.querySelector('.dir-name');

let windowDir = localStorage.getItem('windowDir') || null;

// That path was written on some previous launch and nothing guarantees it still resolves —
// a renamed repo, an unmounted disk, a restored machine. Left alone it scopes the list to a
// folder with nothing in it, next to a header naming a directory that isn't there. Checked
// once, asynchronously, because it costs a stat and the answer only matters for the chrome.
if (windowDir) {
  window.meshAPI.dirExists(windowDir).then((exists) => {
    if (!exists) setWindowDir(null);
  });
}

function setWindowDir(dir) {
  if (dir === windowDir) return;
  windowDir = dir || null;
  if (windowDir) localStorage.setItem('windowDir', windowDir);
  else localStorage.removeItem('windowDir');
  syncDirButton();
  syncNewAgentButton();
}

function syncDirButton() {
  dirEl.classList.toggle('unset', !windowDir);
  dirNameEl.textContent = windowDir ? baseName(windowDir) : 'Choose folder';
  dirEl.title = windowDir
    ? `${windowDir} — click to point this window somewhere else`
    : 'Choose the folder this window is pointed at';
}

// Changing the folder changes what the window is looking at. It does not start anything —
// that is what the button underneath is for.
dirEl.addEventListener('click', async () => {
  const picked = await window.meshAPI.pickDirectory();
  if (!picked) return;
  setWindowDir(picked);
  render(); // the list is scoped to this
});

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
const welcomeSubEl = document.getElementById('welcome-sub');

// Null until the main process has finished looking; `found: false` is a different state
// from "haven't checked yet", and only the first one should disable anything.
let claudeInfo = null;

function claudeMissing() {
  return Boolean(claudeInfo && !claudeInfo.found);
}

function syncNewAgentButton() {
  // Every tab is a `claude` process. With no binary there is nothing a click could start,
  // so the button says why instead of launching a terminal that exits immediately.
  newAgentEl.disabled = claudeMissing();
  newAgentEl.title = claudeMissing()
    ? 'Claude Code CLI not found — install it, then reopen AgentMesh'
    : windowDir
      ? `Start another Claude Code session in ${windowDir}`
      : 'Start a Claude Code session — pick a folder';
}

// The welcome pane is the first thing a fresh install sees, and on a machine with no CLI
// it is the only thing — so it carries the diagnosis rather than the sidebar strip alone.
function syncWelcomeCopy() {
  pickDirEl.disabled = claudeMissing();
  welcomeSubEl.textContent = claudeMissing()
    ? 'Claude Code CLI not found. Install it and make sure `claude` runs in your shell, then reopen AgentMesh.'
    : 'Pick a folder to start a Claude Code session in.';
  welcomeSubEl.classList.toggle('warn', claudeMissing());
}

function applyClaudeInfo(info) {
  claudeInfo = info || null;
  syncNewAgentButton();
  syncWelcomeCopy();
}

window.meshAPI.onClaudeInfo(applyClaudeInfo);
// Also ask directly: a reload after the main process already resolved would otherwise
// never see the push, which only fires once per resolve.
window.meshAPI.claudeInfo().then((info) => {
  if (info && info.source !== 'unresolved') applyClaudeInfo(info);
});

// A session's cwd is fixed at spawn and decides which project it belongs to, so the
// folder is asked for up front rather than inherited from wherever the app happened
// to be launched from.
async function newSessionHere(dir) {
  const target = dir || (await window.meshAPI.pickDirectory());
  if (!target) return;
  setWindowDir(target);
  openTab({ launchCwd: target });
}

// The welcome pane's picker always asks — there is nowhere else to be yet.
pickDirEl.addEventListener('click', () => newSessionHere());
// The one action that starts something, in the folder the header names.
newAgentEl.addEventListener('click', () => newSessionHere(windowDir));

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
    const node = el('button', 'recent-dir');
    node.type = 'button';
    const icon = el('span', 'icon');
    icon.innerHTML = FOLDER_SVG;
    node.appendChild(icon);
    node.appendChild(el('span', 'name', baseName(dir)));
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

// -------------------------------------------------------------------- motion
//
// A spring, not a duration. Two designer-facing parameters instead of the physics
// triplet: `response` is roughly how fast the value reaches the target in seconds
// (a spring has no fixed duration — settle time emerges), `damping` is the ratio,
// 1.0 critically damped, below 1.0 overshoots.
//
// The reason anything gesture-driven uses this and not a CSS transition: a spring is
// interruptible by construction. Re-targeting keeps the *live* position and the live
// velocity, so grabbing a moving thing and reversing it has no discontinuity in it.

const REDUCED_MOTION = window.matchMedia('(prefers-reduced-motion: reduce)');

class Spring {
  constructor(value, apply) {
    this.x = value;
    this.v = 0;
    this.target = value;
    this.apply = apply;
    this.raf = null;
    this.onRest = null;
  }

  stop() {
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = null;
    this.v = 0;
  }

  // Drive the value directly — what a finger dragging it does.
  set(x) {
    this.stop();
    this.x = x;
    this.target = x;
    this.apply(x);
  }

  to(target, { response = 0.4, damping = 1, velocity } = {}) {
    this.target = target;
    if (velocity !== undefined) this.v = velocity;
    this.omega = (2 * Math.PI) / response;
    this.zeta = damping;

    if (REDUCED_MOTION.matches) {
      this.stop();
      this.x = target;
      this.apply(target);
      if (this.onRest) this.onRest();
      return;
    }
    if (this.raf) return; // already running — it will pick up the new target

    let last = performance.now();
    const tick = (now) => {
      const dt = Math.min((now - last) / 1000, 1 / 30); // a dropped frame must not explode it
      last = now;
      const a = -(this.omega ** 2) * (this.x - this.target) - 2 * this.zeta * this.omega * this.v;
      this.v += a * dt;
      this.x += this.v * dt;

      if (Math.abs(this.x - this.target) < 0.25 && Math.abs(this.v) < 2) {
        this.x = this.target;
        this.v = 0;
        this.raf = null;
        this.apply(this.x);
        if (this.onRest) this.onRest();
        return;
      }
      this.apply(this.x);
      this.raf = requestAnimationFrame(tick);
    };
    this.raf = requestAnimationFrame(tick);
  }
}

// Where a flick would come to rest. This is the exponential-decay form Apple ships in
// the Designing Fluid Interfaces sample, not the v²/2a from a physics textbook.
function project(velocity, decelerationRate = 0.998) {
  return ((velocity / 1000) * decelerationRate) / (1 - decelerationRate);
}

// Past a limit the thing keeps following the finger, but less and less. A hard stop
// reads as frozen; progressive resistance reads as "responsive, but this is the edge".
function rubberband(overshoot, dimension, constant = 0.55) {
  return (overshoot * dimension * constant) / (dimension + constant * Math.abs(overshoot));
}

// Velocity at release, from a short position history rather than the last two events —
// one stray sample at the end of a drag otherwise decides the whole throw.
function velocityFrom(history, window = 120) {
  const now = performance.now();
  const recent = history.filter((s) => now - s.t <= window);
  if (recent.length < 2) return 0;
  const a = recent[0];
  const b = recent[recent.length - 1];
  const dt = (b.t - a.t) / 1000;
  return dt > 0 ? (b.x - a.x) / dt : 0;
}

// ----------------------------------------------------------- sidebar resizing

const sidebarEl = document.getElementById('sidebar');
const resizerEl = document.getElementById('sidebar-resizer');

const MIN_SIDEBAR = 180;
const MAX_SIDEBAR = 520;
const DEFAULT_SIDEBAR = 248;
// Widths worth landing on exactly. A flick that would come to rest near one of these
// gets pulled onto it; anything else keeps whatever width the drag ended at.
const SNAP_POINTS = [MIN_SIDEBAR, DEFAULT_SIDEBAR, MAX_SIDEBAR];
const SNAP_RADIUS = 24;

const clampWidth = (px) => Math.min(MAX_SIDEBAR, Math.max(MIN_SIDEBAR, px));

function paintSidebarWidth(px) {
  const w = Math.round(px);
  sidebarEl.style.flex = `0 0 ${w}px`;
  sidebarEl.style.width = `${w}px`;
}

const savedWidth = Number(localStorage.getItem('sidebarWidth'));
const widthSpring = new Spring(clampWidth(savedWidth || DEFAULT_SIDEBAR), paintSidebarWidth);
widthSpring.onRest = () => fitAll();
paintSidebarWidth(widthSpring.x);

function settleSidebar(target, velocity) {
  localStorage.setItem('sidebarWidth', String(Math.round(target)));
  // Handing velocity to a spring that's already at its target only makes it wobble.
  widthSpring.to(target, {
    response: 0.4,
    damping: 1, // repositioning, no flick behind it — Apple's move preset
    velocity: Math.abs(target - widthSpring.x) < 0.5 ? 0 : velocity,
  });
}

resizerEl.addEventListener('pointerdown', (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  resizerEl.setPointerCapture(e.pointerId); // keep tracking once the pointer leaves the 6px strip
  widthSpring.stop();

  // Track from where they grabbed, not from the strip's centre — snapping the edge to
  // the pointer on grab is a visible jump and breaks the 1:1 illusion immediately.
  const grabOffset = e.clientX - widthSpring.x;
  const history = [{ x: e.clientX, t: performance.now() }];

  document.body.classList.add('resizing');
  resizerEl.classList.add('dragging');

  const onMove = (ev) => {
    const raw = ev.clientX - grabOffset;
    const clamped = clampWidth(raw);
    const over = raw - clamped;
    widthSpring.set(clamped + (over ? rubberband(over, window.innerWidth) : 0));
    history.push({ x: ev.clientX, t: performance.now() });
    if (history.length > 8) history.shift();
  };

  const onUp = () => {
    resizerEl.removeEventListener('pointermove', onMove);
    resizerEl.removeEventListener('pointerup', onUp);
    resizerEl.removeEventListener('pointercancel', onUp);
    document.body.classList.remove('resizing');
    resizerEl.classList.remove('dragging');

    const v = velocityFrom(history);
    const projected = clampWidth(widthSpring.x + project(v));
    const snap = SNAP_POINTS.find((p) => Math.abs(projected - p) <= SNAP_RADIUS);
    settleSidebar(snap === undefined ? clampWidth(widthSpring.x) : snap, v);
  };

  resizerEl.addEventListener('pointermove', onMove);
  resizerEl.addEventListener('pointerup', onUp);
  resizerEl.addEventListener('pointercancel', onUp);
});

// double-click resets to the default width
resizerEl.addEventListener('dblclick', () => settleSidebar(DEFAULT_SIDEBAR, 0));

// ------------------------------------------------------------------- sidebar

const listEl = document.getElementById('list');
const filterEl = document.getElementById('filter');

let rows = [];
// Default is scoped, and per *window*: with several AgentMesh windows open on different
// repos, a window should answer "what's running on what I'm looking at" first and act as a
// machine-wide mesh view second. The window's own current folder is the active tab's cwd.
let projectOnly = localStorage.getItem('projectOnly') !== 'false';
let scopedDir = null; // the folder actually being filtered on, null when showing everything
let recentOpen = false; // history is noise until asked for; live agents are the point

// ------------------------------------------------------------------- octicons
//
// Shipped 16px paths from @primer/octicons, inlined rather than pulled in as a
// dependency — five glyphs is not a package.
const DOT_SVG =
  '<svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">' +
  '<path d="M8 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8Z"/>' +
  '</svg>';
// Primer's own Spinner markup: a 25%-opacity ring plus a quarter-arc, rotated 1s linear.
const SPINNER_SVG =
  '<svg viewBox="0 0 16 16" fill="none" aria-hidden="true">' +
  '<circle cx="8" cy="8" r="7" stroke="currentColor" stroke-opacity="0.25" stroke-width="2"/>' +
  '<path d="M15 8a7.002 7.002 0 0 0-7-7" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>' +
  '</svg>';
const CHEVRON_SVG =
  '<svg viewBox="0 0 16 16" aria-hidden="true">' +
  '<path d="M6.22 3.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L9.94 8 6.22 4.28a.75.75 0 0 1 0-1.06Z"/>' +
  '</svg>';
const X_SVG =
  '<svg viewBox="0 0 16 16" aria-hidden="true">' +
  '<path d="M3.72 3.72a.75.75 0 0 1 1.06 0L8 6.94l3.22-3.22a.749.749 0 0 1 1.275.326.749.749 0 0 1-.215.734L9.06 8l3.22 3.22a.749.749 0 0 1-.326 1.275.749.749 0 0 1-.734-.215L8 9.06l-3.22 3.22a.751.751 0 0 1-1.042-.018.751.751 0 0 1-.018-1.042L6.94 8 3.72 4.78a.75.75 0 0 1 0-1.06Z"/>' +
  '</svg>';
// git-branch: the fork affordance on a session someone else is already driving.
const FORK_SVG =
  '<svg viewBox="0 0 16 16" aria-hidden="true">' +
  '<path d="M9.5 3.25a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.128a2.251 2.251 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.5 0v1.878A2.49 2.49 0 0 1 6 7h4a1 1 0 0 0 1-1v-.628A2.25 2.25 0 0 1 9.5 3.25Zm-6 0a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Zm8.25-.75a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z"/>' +
  '</svg>';
const FOLDER_SVG =
  '<svg viewBox="0 0 16 16" aria-hidden="true">' +
  '<path d="M1.75 1A1.75 1.75 0 0 0 0 2.75v10.5C0 14.216.784 15 1.75 15h12.5A1.75 1.75 0 0 0 16 13.25v-8.5A1.75 1.75 0 0 0 14.25 3H7.5a.25.25 0 0 1-.2-.1l-.9-1.2C6.07 1.26 5.55 1 5 1H1.75Z"/>' +
  '</svg>';

// ------------------------------------------------------------- status glyphs
//
// Modelled on GitHub Actions' run status, and it's the same four questions: is it
// running, is it blocked on me, did it finish, is it over.

const STATUS_LABEL = {
  busy: 'Working',
  waiting: 'Waiting on you',
  idle: 'Idle',
  dead: 'Not running',
};

// The registry uses more status strings than it documents (`busy`, `shell`, and
// whatever the next version adds), and every one of them except `idle`/`waiting` means
// the session is doing something — so treat unknown as working rather than as done.
function statusOf(row) {
  if (!row.live) return 'dead';
  if (row.status === 'idle') return 'idle';
  if (row.status === 'waiting') return 'waiting';
  return 'busy';
}

// busy -> not-busy is the interesting edge ("it finished, it's your turn"), and it has
// to be remembered outside the DOM: `lastStatus` holds the previous poll's state per
// session, `landed` records the sessions whose arrival pulse has already played so a
// row that sits idle for an hour doesn't blink once a minute.
const lastStatus = new Map();
const landed = new Set();

// Mutates an existing glyph in place. Rewriting the markup on every 4s poll would
// restart the spinner's rotation each time, which is exactly the kind of jitter that
// reads as carelessness — so an unchanged state touches nothing at all.
function applyStatus(node, row) {
  const state = statusOf(row);
  const prev = lastStatus.get(row.sessionId);
  lastStatus.set(row.sessionId, state);
  if (state === 'busy') landed.delete(row.sessionId); // a fresh run earns a fresh pulse
  if (node.dataset.state === state) return;

  node.dataset.state = state;
  node.className = `status ${state}`;
  node.innerHTML = state === 'busy' ? SPINNER_SVG : DOT_SVG;
  node.title = STATUS_LABEL[state];

  if (state === 'busy') {
    // Negative delay lines every spinner up on the same rotation angle no matter when
    // its row was built — Primer's computeSyncDelay, same trick.
    node.firstChild.style.animationDelay = `-${performance.now() % 1000}ms`;
    return;
  }
  if (prev === 'busy' && !landed.has(row.sessionId)) {
    landed.add(row.sessionId);
    node.classList.add('landed');
    // Not `{ once: true }`: the inner pop animation bubbles its own animationend first
    // and would consume the one-shot listener before the ring is finished.
    const done = (e) => {
      if (e.target !== node) return;
      node.classList.remove('landed');
      node.removeEventListener('animationend', done);
    };
    node.addEventListener('animationend', done);
  }
}

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

// Coarse buckets, not a clock. Past a week the exact date is more use than "9d ago",
// and nothing in between is worth a second of precision.
function relTime(ms) {
  if (!ms) return null;
  const secs = Math.max(0, (Date.now() - ms) / 1000);
  if (secs < 45) return 'just now';
  const mins = secs / 60;
  if (mins < 60) return `${Math.round(mins)}m ago`;
  const hours = mins / 60;
  if (hours < 24) return `${Math.round(hours)}h ago`;
  const days = hours / 24;
  if (days < 7) return `${Math.round(days)}d ago`;
  return new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

// Where the session is and when it last moved. Not its pid, not its age in bytes, and
// not the prompt count — that was bookkeeping nobody scans a sidebar for. The project
// drops out while the list is scoped to one folder, where every row would repeat the same
// word — `scopedDir`, not `projectOnly`, because the toggle can be on with nothing to
// scope to and those rows do still span projects.
function renderMeta(node, row) {
  const parts = [];
  if (row.live && row.kind === 'bg') parts.push('background');
  if (row.project && !scopedDir) parts.push(row.project);
  if (row.gitBranch && row.gitBranch !== 'HEAD') parts.push(row.gitBranch);
  const when = relTime(row.updatedAt);
  if (when) parts.push(when);

  const children = [];
  parts.forEach((p, i) => {
    if (i) children.push(el('span', 'sep', '·'));
    children.push(document.createTextNode(p));
  });
  node.replaceChildren(...children);
}

function tooltip(row) {
  return [
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
}

// A live session is already owned by the process driving it, and `--resume` is not an
// attach: on an interactive session it starts a *second* writer on one transcript, and
// on a background agent the CLI refuses it outright ("is currently running as a
// background agent … add --fork-session to branch off a copy").
//
// So there is exactly one honest thing a click on a live-elsewhere row can do, and it
// is a fork. That used to be a hidden ⌥ and a message explaining why the plain click
// did nothing — a modifier key is not an affordance. The row now *says* it is in use
// and carries a fork button, so the click has no surprise left in it to warn about.
function activateRow(row) {
  if (row.live) {
    if (row.kind === 'bg') {
      openTab({ launchCwd: row.cwd, agentsView: true, title: 'agents' });
      return;
    }
    openTab({
      resumeSessionId: row.sessionId,
      forkSession: true, // a fork mints its own id, so the origin's name must not carry over
      title: row.title,
      launchCwd: row.cwd,
    });
    return;
  }

  openTab({
    resumeSessionId: row.sessionId,
    resumeName: row.customTitle, // only a real name carries over, never an AI title
    title: row.title,
    launchCwd: row.cwd,
  });
}

// A row is built once per session and then updated in place — see `render()` for why
// that matters. Everything that varies is read off `cur`, which `_update` swaps, so
// the listeners installed here never go stale.
function makeRow(row) {
  const node = el('div', 'row entering');
  node.dataset.key = row.sessionId;
  node.tabIndex = -1; // roving; render() promotes exactly one row to 0
  node.setAttribute('role', 'option');

  const status = el('span', 'status');
  const title = el('div', 'row-title');
  const label = el('span', 'row-label');
  // Two mutually exclusive states worth a word: this is a copy of another session, or
  // this session belongs to a process we are not.
  const chip = el('span', 'chip');
  title.append(label, chip);
  const metaEl = el('div', 'row-meta');

  const close = el('span', 'row-close');
  close.innerHTML = X_SVG;
  close.title = 'Close this terminal';

  const forkBtn = el('span', 'row-fork');
  forkBtn.innerHTML = FORK_SVG;
  forkBtn.title = 'Open a copy — this session is being driven by another process';

  node.append(status, title, metaEl, close, forkBtn);

  // The arrival animation is one-shot; the landing pulse on the status glyph bubbles
  // its own animationend through here, so match on the target before clearing.
  node.addEventListener('animationend', (e) => {
    if (e.target === node) node.classList.remove('entering');
  });

  let cur = row;
  // Set the moment a click commits, cleared once the tab it spawns actually exists.
  // The safety timer is for the spawn that never registers — a stuck pulse would be a
  // worse lie than no feedback at all.
  let pendingTimer = null;
  const clearPending = () => {
    if (!pendingTimer) return;
    clearTimeout(pendingTimer);
    pendingTimer = null;
    node.classList.remove('pending');
  };
  const markPending = () => {
    clearPending();
    node.classList.add('pending');
    pendingTimer = setTimeout(clearPending, 8000);
  };

  node._update = (next, depth = 0) => {
    cur = next;
    const ours = Boolean(next.tabId && tabs.has(next.tabId));
    // Live, interactive, and driven by something other than this window. Background
    // agents are excluded: they're daemon-owned by definition, `background` in the
    // subtitle already says so, and clicking one attaches through FleetView.
    const elsewhere = next.live && !ours && next.kind !== 'bg';

    if (ours) clearPending(); // the thing the click promised now exists
    node.classList.toggle('open', ours);
    // The one row you are actually looking at. `.open` is true of every hosted tab at
    // once and so can't answer "where am I" — this can, and only ever for one row.
    node.classList.toggle('active-tab', ours && next.tabId === activeTabId);
    node.setAttribute('aria-selected', String(ours && next.tabId === activeTabId));
    node.classList.toggle('elsewhere', elsewhere);
    node.classList.toggle('dim', !next.live);
    node.classList.toggle('child', depth > 0);
    node.style.setProperty('--depth', String(depth));
    applyStatus(status, next);

    if (label.textContent !== next.title) label.textContent = next.title;
    label.classList.toggle('badge-ai', next.titleSource === 'ai');
    label.classList.toggle('badge-name', next.titleSource === 'name');

    // A fork carries its origin's title verbatim, so without this the two are the same
    // card twice. The nesting says which one it came from; the chip says what it is.
    const isFork = Boolean(parentOf(next));
    const chipText = isFork ? 'fork' : elsewhere ? 'in use' : '';
    chip.textContent = chipText;
    chip.className = `chip${chipText ? ` ${isFork ? 'fork' : 'inuse'}` : ''}`;
    chip.hidden = !chipText;

    renderMeta(metaEl, next);
    node.title = tooltip(next); // full metadata on hover — cheap inspector, no detail pane yet
  };

  close.addEventListener('click', (e) => {
    e.stopPropagation();
    closeTab(cur.tabId);
  });

  forkBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    clearTimeout(openTimer);
    openTimer = null;
    markPending();
    activateRow(cur);
  });

  // Double-click renames, so a click that would *spawn* a tab has to wait long enough
  // to find out whether a second click is coming — otherwise every rename leaves a
  // stray `claude --resume` behind. Focusing an already-open tab is free and
  // idempotent, so that path stays instant and skips the delay entirely.
  let openTimer = null;
  node.addEventListener('click', () => {
    if (cur.tabId && tabs.has(cur.tabId)) {
      setActive(cur.tabId);
      return;
    }
    clearTimeout(openTimer);
    // The bar goes up on the click, not when the timer expires — the 220ms wait is ours,
    // and making the user stare at an unchanged row through it is the latency the whole
    // response principle is about.
    markPending();
    openTimer = setTimeout(() => {
      openTimer = null;
      activateRow(cur);
    }, 220);
  });

  node._rename = () => beginRename(label, cur.sessionId, cur.title);

  node.addEventListener('dblclick', (e) => {
    e.preventDefault();
    clearTimeout(openTimer);
    openTimer = null;
    clearPending(); // the first click of a rename never meant to open anything
    node._rename();
  });
  node.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    window.meshAPI.reveal(cur.sessionId);
  });

  node._update(row);
  return node;
}

// Also built once: rebuilding it would restart the chevron's rotation on every poll.
function makeGroup(label, collapsible, onToggle) {
  const node = el('div', 'group');
  let chev = null;
  if (collapsible) {
    node.classList.add('collapsible');
    chev = el('span', 'chev');
    chev.innerHTML = CHEVRON_SVG;
    node.appendChild(chev);
    node.addEventListener('click', onToggle);
  }
  node.appendChild(el('span', 'label', label));
  const count = el('span', 'count');
  node.appendChild(count);

  node._update = (n, open) => {
    count.textContent = String(n);
    if (chev) chev.classList.toggle('open', Boolean(open));
  };
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

    const parent = pendingForks.get(row.tabId);
    if (parent && parent !== row.sessionId) {
      recordFork(row.sessionId, parent);
      pendingForks.delete(row.tabId);
    }
  }
}

// The list repaints every 4s whether anything changed or not, so it cannot be torn
// down and rebuilt: detaching a node cancels its CSS animations (the spinner restarts,
// the arrival animation replays) and drops :hover and any open text selection with it.
// Nodes are therefore cached per session and only ever moved.
const rowNodes = new Map();
const tabNodes = new Map();

// LIVE was one bucket answering three different questions, so it's three groups now,
// ordered by what they want from you: WAITING is blocked on you and is why you looked
// at the sidebar at all; RUNNING is working and needs nothing; IDLE is up and costs
// nothing to click into. Unknown registry statuses land in RUNNING (see statusOf).
const waitingGroupEl = makeGroup('WAITING', false);
const runningGroupEl = makeGroup('RUNNING', false);
const idleGroupEl = makeGroup('IDLE', false);
// Same amber the waiting glyph uses — the header and the dots under it say one thing.
waitingGroupEl.classList.add('urgent');
const recentGroupEl = makeGroup('RECENT', true, () => {
  recentOpen = !recentOpen;
  render();
});
const emptyEl = el('div', null, 'No sessions found.');
emptyEl.id = 'empty';

// A scoped window is showing a slice and used to present it as the whole. This is the
// count of what's hidden and the control that unhides it in one object — the same reason
// the fork button replaced a hidden ⌥click: the state and the way out of it belong on
// screen, not in something you have to already know.
const moreEl = el('button', 'list-more');
moreEl.type = 'button';
moreEl._update = (n) => {
  moreEl.textContent = `${n} more elsewhere on this machine`;
  moreEl.title = 'Show every session, in every folder';
};
moreEl.addEventListener('click', () => {
  projectOnly = false;
  localStorage.setItem('projectOnly', 'false');
  render();
});

function rowNode(row, depth = 0) {
  let node = rowNodes.get(row.sessionId);
  if (!node) {
    node = makeRow(row);
    rowNodes.set(row.sessionId, node);
  }
  revive(node); // a row can come back mid-fade — RECENT collapsed and reopened
  node._update(row, depth);
  return node;
}

// A tab this window is hosting that no session row accounts for: a `claude agents`
// FleetView, or a session too young to have registered itself yet. The sidebar lists
// sessions but the close button closes *terminals*, so without these the tab exists
// with no card, and nothing in the UI can shut it.
function makeTabRow(tabId) {
  const node = el('div', 'row entering open');
  node.dataset.key = `tab:${tabId}`;
  node.tabIndex = -1;
  node.setAttribute('role', 'option');
  const status = el('span', 'status idle');
  status.innerHTML = DOT_SVG;
  const title = el('div', 'row-title');
  const label = el('span', 'row-label');
  title.appendChild(label);
  const metaEl = el('div', 'row-meta');
  const close = el('span', 'row-close');
  close.innerHTML = X_SVG;
  close.title = 'Close this terminal';
  node.append(status, title, metaEl, close);

  node.addEventListener('animationend', (e) => {
    if (e.target === node) node.classList.remove('entering');
  });
  node.addEventListener('click', () => setActive(tabId));
  close.addEventListener('click', (e) => {
    e.stopPropagation();
    closeTab(tabId);
  });

  node._update = (tab) => {
    const dir = tab.cwd ? baseName(tab.cwd) : null;
    label.textContent = tab.title || dir || 'Session';
    metaEl.textContent = tab.title && dir ? dir : tab.cwd || 'starting…';
    node.classList.toggle('active-tab', tabId === activeTabId);
    node.setAttribute('aria-selected', String(tabId === activeTabId));
    node.title = tab.cwd || '';
  };
  return node;
}

function tabRowNode(tabId, tab) {
  let node = tabNodes.get(tabId);
  if (!node) {
    node = makeTabRow(tabId);
    tabNodes.set(tabId, node);
  }
  revive(node);
  node._update(tab);
  return node;
}

// A fork inherits its origin's title, so flat ordering renders it as a duplicate card.
// Children hang off their parent instead, one indent step per generation. A fork whose
// origin isn't in the same group (parent idle, fork busy) stands on its own.
function nest(list) {
  const byId = new Map(list.map((r) => [r.sessionId, r]));
  const children = new Map();
  const roots = [];
  for (const row of list) {
    const origin = parentOf(row);
    const parent = origin && byId.has(origin) ? origin : null;
    if (!parent) {
      roots.push(row);
      continue;
    }
    if (!children.has(parent)) children.set(parent, []);
    children.get(parent).push(row);
  }

  const out = [];
  const placed = new Set(); // transcripts are internal state; never trust them not to cycle
  const walk = (row, depth) => {
    if (placed.has(row.sessionId)) return;
    placed.add(row.sessionId);
    out.push(rowNode(row, depth));
    for (const kid of children.get(row.sessionId) || []) walk(kid, depth + 1);
  };
  for (const row of roots) walk(row, 0);
  for (const row of list) walk(row, 0); // anything a cycle stranded still gets a card
  return out;
}

// ------------------------------------------------------------------ leaving
//
// A row that arrived with motion and then blinks out of existence reads as a glitch, not
// as a departure — enter and exit have to travel the same path. The node is frozen at the
// position it already occupies and taken out of flow first, so the rows behind it close
// the gap on the same frame the fade begins; the alternative is a 200ms hole in the list.
//
// Ghosts stay in the DOM while they fade, which is why `syncChildren` walks past them
// instead of counting them: re-inserting a live node to "fix" an order that ghosts made
// look wrong would detach it, and detaching restarts every CSS animation on it.

const LEAVE_MS = 200;
const leaveTimers = new WeakMap();

function revive(node) {
  const t = leaveTimers.get(node);
  if (t) {
    clearTimeout(t);
    leaveTimers.delete(node);
  }
  node.classList.remove('leaving');
  node.style.position = '';
  node.style.top = '';
  node.style.left = '';
  node.style.width = '';
  node.style.height = '';
}

// Measure every victim before absolutizing any of them: the first node leaving the flow
// pulls the ones under it up, and a position read after that freezes them where they
// jumped to, not where the eye last saw them.
function dismissAll(nodes) {
  if (REDUCED_MOTION.matches) {
    for (const node of nodes) node.remove();
    return;
  }
  // offsetWidth/Height are border-box; `.row` is content-box, so `.leaving` flips
  // box-sizing to keep the frozen frame the same size as the row it replaces.
  const frames = nodes.map((node) => ({
    node,
    top: node.offsetTop,
    left: node.offsetLeft,
    width: node.offsetWidth,
    height: node.offsetHeight,
  }));
  for (const { node, top, left, width, height } of frames) {
    node.style.top = `${top}px`;
    node.style.left = `${left}px`;
    node.style.width = `${width}px`;
    node.style.height = `${height}px`;
    node.style.position = 'absolute';
    node.classList.add('leaving');
    leaveTimers.set(
      node,
      setTimeout(() => {
        node.remove();
        revive(node); // cached in rowNodes; it may well come back
      }, LEAVE_MS)
    );
  }
}

const isGhost = (n) => n && n.classList && n.classList.contains('leaving');
const nextLive = (n) => {
  while (isGhost(n)) n = n.nextSibling;
  return n;
};

// Reorder in place: remove what's gone, then walk the desired list inserting only
// where the DOM already disagrees. Nodes that keep their position are never touched.
function syncChildren(parent, desired) {
  const keep = new Set(desired);
  const leaving = [];
  for (const child of Array.from(parent.children)) {
    if (keep.has(child) || isGhost(child)) continue;
    if (child.classList.contains('row')) leaving.push(child);
    else parent.removeChild(child);
  }
  dismissAll(leaving);

  let ref = nextLive(parent.firstChild);
  for (const node of desired) {
    if (ref === node) {
      ref = nextLive(ref.nextSibling);
      continue;
    }
    parent.insertBefore(node, ref);
  }
}

// ------------------------------------------------------------------ keyboard
//
// The sidebar is the switcher for every terminal in the window, and until now it was
// reachable by exactly one input device. A list of things you move between is a listbox:
// one row holds tabindex 0 and the arrows move it (a roving tabindex), so Tab passes the
// whole list in one press instead of stopping on fifty rows.
//
// ⌘0 puts you in the list, ⌘1–9 jump straight to a terminal, Escape hands focus back to
// the one that's on screen — so the round trip out to the sidebar and back never needs
// the trackpad.

let focusKey = null; // dataset.key of the row that owns tabindex 0

const listRows = () => Array.from(listEl.querySelectorAll('.row:not(.leaving)'));

function syncRovingFocus() {
  const nodes = listRows();
  if (!nodes.some((n) => n.dataset.key === focusKey)) {
    focusKey = nodes.length ? nodes[0].dataset.key : null;
  }
  for (const n of nodes) n.tabIndex = n.dataset.key === focusKey ? 0 : -1;
}

function focusRow(node) {
  if (!node) return;
  focusKey = node.dataset.key;
  for (const n of listRows()) n.tabIndex = n === node ? 0 : -1;
  node.focus();
  node.scrollIntoView({ block: 'nearest' });
}

function moveFocus(delta) {
  const nodes = listRows();
  if (!nodes.length) return;
  const at = nodes.findIndex((n) => n.dataset.key === focusKey);
  const next = at < 0 ? 0 : Math.min(nodes.length - 1, Math.max(0, at + delta));
  focusRow(nodes[next]);
}

// Focus follows the pointer into the list, so clicking a row and then pressing ↓ carries
// on from what you clicked rather than from wherever the keyboard was last.
listEl.addEventListener('pointerdown', (e) => {
  const row = e.target.closest && e.target.closest('.row');
  if (row && !row.classList.contains('leaving')) focusKey = row.dataset.key;
});

listEl.addEventListener('keydown', (e) => {
  if (e.target.tagName === 'INPUT') return; // the rename input owns its own keys
  const row = e.target.closest && e.target.closest('.row');

  switch (e.key) {
    case 'ArrowDown':
      e.preventDefault();
      moveFocus(1);
      return;
    case 'ArrowUp':
      e.preventDefault();
      moveFocus(-1);
      return;
    case 'Home':
      e.preventDefault();
      focusRow(listRows()[0]);
      return;
    case 'End': {
      e.preventDefault();
      const nodes = listRows();
      focusRow(nodes[nodes.length - 1]);
      return;
    }
    case 'Enter':
    case ' ':
      if (!row) return;
      e.preventDefault();
      row.click();
      return;
    case 'F2':
      // The rename affordance was double-click and nothing else, which no keyboard can
      // reach. Rows we host but have no session for (`_rename` unset) can't be renamed.
      if (!row || !row._rename) return;
      e.preventDefault();
      row._rename();
      return;
    case 'Escape':
      e.preventDefault();
      tabs.get(activeTabId)?.term.focus();
      return;
    default:
  }
});

// ⌘0 / Ctrl+0 into the sidebar, ⌘1–9 to the nth terminal. These are caught on the document
// because the terminal has focus almost all the time and xterm never forwards those chords
// to the pty. Accepting either modifier rather than branching on platform: Ctrl-digit is not
// a chord Claude Code's TUI or a shell claims, so there is nothing to collide with on macOS.
window.addEventListener('keydown', (e) => {
  const accel = e.metaKey || e.ctrlKey;
  if (!accel || e.altKey || e.shiftKey || (e.metaKey && e.ctrlKey)) return;
  if (document.activeElement && document.activeElement.tagName === 'INPUT') return;

  if (e.key === '0') {
    e.preventDefault();
    const nodes = listRows();
    if (nodes.length) focusRow(nodes.find((n) => n.dataset.key === focusKey) || nodes[0]);
    else newAgentEl.focus();
    return;
  }
  const n = Number(e.key);
  if (!Number.isInteger(n) || n < 1 || n > 9) return;
  const tabId = Array.from(tabs.keys())[n - 1];
  if (!tabId) return;
  e.preventDefault();
  setActive(tabId);
});

function render() {
  if (editingSessionId) {
    renderPending = true;
    return;
  }
  // No folder chosen means nothing to scope to, and an empty list would be a dead end —
  // the welcome pane is where you go to start something, so it shows everything.
  scopedDir = projectOnly ? windowDir : null;
  const visible = scopedDir ? rows.filter((r) => r.cwd === scopedDir) : rows;
  syncFilterChip();
  syncTabs(visible);
  if (!welcomeEl.classList.contains('hidden')) renderRecentDirs();

  // Sessions that have fallen off the list keep no node, no remembered status and no
  // spent landing pulse — otherwise all three grow for the life of the process.
  const alive = new Set(rows.map((r) => r.sessionId));
  for (const id of rowNodes.keys()) if (!alive.has(id)) rowNodes.delete(id);
  for (const id of lastStatus.keys()) if (!alive.has(id)) lastStatus.delete(id);
  for (const id of landed) if (!alive.has(id)) landed.delete(id);
  for (const id of tabNodes.keys()) if (!tabs.has(id)) tabNodes.delete(id);

  // Tabs we host that no row claims — they belong at the top of RUNNING, because they
  // are the most running thing on the list: this window is looking at them.
  const claimed = new Set(rows.map((r) => r.tabId).filter(Boolean));
  const orphanTabs = [];
  for (const [tabId, tab] of tabs) {
    if (!claimed.has(tabId)) orphanTabs.push(tabRowNode(tabId, tab));
  }

  // How many sessions the scope is holding back. Zero when unscoped, and the control is
  // omitted then — it would be a button that reports nothing and does nothing.
  const hidden = scopedDir ? rows.length - visible.length : 0;
  if (hidden > 0) moreEl._update(hidden);

  if (!visible.length && !orphanTabs.length) {
    // An empty scoped list is a dead end unless it says what it's empty *of*, and
    // offers the list that isn't.
    // Three different emptinesses, and they mean different things. A scoped list is hiding
    // sessions that exist. An unscoped list with nothing behind it on a machine that has
    // never run the CLI is a first launch, not a failure — the sidebar is a view onto
    // Claude Code's own state, so before there is any state there is nothing to show.
    emptyEl.textContent = scopedDir
      ? `No sessions in ${baseName(scopedDir)}.`
      : rows.length
        ? 'No sessions found.'
        : 'No Claude Code sessions yet — start one and it shows up here.';
    syncChildren(listEl, hidden > 0 ? [emptyEl, moreEl] : [emptyEl]);
    syncRovingFocus();
    return;
  }

  const waiting = visible.filter((r) => statusOf(r) === 'waiting');
  const running = visible.filter((r) => statusOf(r) === 'busy');
  const idle = visible.filter((r) => statusOf(r) === 'idle');
  const past = visible.filter((r) => !r.live);
  const desired = [];

  if (waiting.length) {
    waitingGroupEl._update(waiting.length);
    desired.push(waitingGroupEl, ...nest(waiting));
  }
  // A tab we host but no row accounts for is running by definition — this window is
  // looking at it — so it sits with RUNNING rather than earning a group of its own.
  if (running.length || orphanTabs.length) {
    runningGroupEl._update(running.length + orphanTabs.length);
    desired.push(runningGroupEl, ...orphanTabs, ...nest(running));
  }
  if (idle.length) {
    idleGroupEl._update(idle.length);
    desired.push(idleGroupEl, ...nest(idle));
  }
  if (past.length) {
    recentGroupEl._update(past.length, recentOpen);
    desired.push(recentGroupEl);
    if (recentOpen) desired.push(...nest(past));
  }
  if (hidden > 0) desired.push(moreEl);

  syncChildren(listEl, desired);
  syncRovingFocus();
}

// Only fade the top edge once there's content underneath the header to fade.
listEl.addEventListener(
  'scroll',
  () => listEl.classList.toggle('scrolled', listEl.scrollTop > 2),
  { passive: true }
);

// Three states, not two: scoped to a folder, showing everything, and "scoped but there's
// no folder yet" — which behaves as ALL because that's what the list is doing, just dimmed
// so it's clear the toggle is still armed. The folder it scopes to is named by the header
// control right next to it, so the glyph never has to spell it out.
function syncFilterChip() {
  const armed = projectOnly && Boolean(windowDir);
  filterEl.classList.toggle('on', armed);
  filterEl.classList.toggle('empty', projectOnly && !windowDir);
  filterEl.setAttribute('aria-pressed', String(armed));
  filterEl.title = armed
    ? `Showing ${windowDir} only — click for every session on this machine`
    : windowDir
      ? `Showing every session on this machine — click to scope to ${windowDir}`
      : 'Showing every session on this machine — choose a folder above to scope this window';
}

filterEl.addEventListener('click', () => {
  projectOnly = !projectOnly;
  localStorage.setItem('projectOnly', String(projectOnly));
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

// ------------------------------------------------------------------- plan usage
//
// Deliberately not part of render(). That rebuilds every row on the 4s session poll,
// while this arrives on its own ~5min cadence — folding the two together would either
// throw the bar away 75 times between updates or drag the poll down to the slower one.

const usageEl = document.getElementById('usage');
const WARN_PCT = 75;
const CRIT_PCT = 90;

let usageData = null;
let usageTimer = null; // ticks the session countdown; cleared when there's nothing to count

function usageLevel(pct) {
  return pct >= CRIT_PCT ? 'crit' : pct >= WARN_PCT ? 'warn' : '';
}

// The footer counts down the 5-hour session window specifically. It's the one that
// actually gates the next few hours of work — the weekly window resets on a day, not a
// clock, so a countdown says nothing useful about it. Falls back to whatever window is
// closest to biting on plans that report no session window at all.
function countdownWindow(u) {
  const session = u.windows.find((w) => w.key === 'five_hour');
  if (session) return session;
  let best = null;
  for (const w of u.windows) if (!best || w.percent > best.percent) best = w;
  return best;
}

function resetsInText(iso) {
  const ms = new Date(iso).getTime() - Date.now();
  if (!Number.isFinite(ms)) return '';
  if (ms <= 0) return 'resetting…';
  const mins = Math.floor(ms / 60000);
  const hours = Math.floor(mins / 60);
  if (hours >= 24) return `resets in ${Math.floor(hours / 24)}d ${hours % 24}h`;
  return hours ? `resets in ${hours}h ${mins % 60}m` : `resets in ${mins}m`;
}

// The session window is always shown as a live countdown — "resets in 3h 12m" is the
// number you plan the afternoon around, and it's wrong the moment it stops moving. This
// is the one timer in the sidebar: 60s (all the precision "3h 12m" has), writing a single
// text node, never touching render(). The row ages that used to live here were dropped
// because they cost a full repaint of every row; this costs one assignment.
function paintUsageFoot() {
  if (!usageData) return;
  const el = usageEl.querySelector('.usage-reset');
  const foot = usageEl.querySelector('.usage-foot');
  const target = countdownWindow(usageData);
  if (!el || !target) return;

  el.textContent = target.resetsAt ? resetsInText(target.resetsAt) : '';
  // Amber on the countdown once anything is running hot, not just the window being
  // counted — the weekly cap biting is equally worth noticing.
  if (foot) foot.classList.toggle('urgent', usageData.windows.some((w) => w.percent >= WARN_PCT));

  if (!usageTimer) usageTimer = setInterval(paintUsageFoot, 60000);
}

function renderUsage(value) {
  usageData = value && value.windows && value.windows.length ? value : null;

  if (!usageData) {
    if (usageTimer) {
      clearInterval(usageTimer);
      usageTimer = null;
    }
    usageEl.textContent = ''; // `:empty` is the hidden state
    return;
  }

  usageEl.textContent = '';
  for (const w of usageData.windows) {
    const pct = Math.round(w.percent);
    const level = usageLevel(pct);
    const row = el('div', `usage-row${level ? ' ' + level : ''}`);
    // The footer counts down the session window; every other window's reset date lives
    // here rather than competing for the one line underneath.
    if (w.resetsAt) {
      const d = new Date(w.resetsAt);
      if (!Number.isNaN(d.getTime())) row.title = `${w.label} · resets ${d.toLocaleString()}`;
    }
    row.append(el('span', 'usage-label', w.label));
    const track = el('span', 'usage-track');
    const fill = el('i');
    // A non-zero window that rounds to 0% still deserves a visible sliver.
    fill.style.width = `${pct > 0 ? Math.max(pct, 2) : 0}%`;
    track.append(fill);
    row.append(track, el('span', 'usage-pct', `${pct}%`));
    usageEl.append(row);
  }

  const foot = el('div', 'usage-foot');
  foot.append(el('span', 'usage-reset'), el('span', 'usage-plan', usageData.plan || ''));
  usageEl.append(foot);

  usageEl.classList.toggle('stale', usageData.stale === true);
  paintUsageFoot();
}

usageEl.addEventListener('click', () => {
  if (!usageData) return;
  usageEl.classList.add('pending');
  window.meshAPI
    .refreshUsage()
    .then(renderUsage)
    .finally(() => usageEl.classList.remove('pending'));
});

window.meshAPI.onUsageUpdate(renderUsage);
window.meshAPI.usage().then(renderUsage);

// A window remembers where it was pointed, so the header and the scope survive a restart
// even though no session does.
syncDirButton();
showWelcome(true); // nothing is spawned until a folder is picked
