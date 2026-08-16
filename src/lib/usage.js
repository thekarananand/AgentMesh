// Reads the account's Claude plan usage — the 5-hour and weekly windows behind `/usage`.
//
// Unlike everything else in the sidebar this is an *account* fact, not a session fact, so
// there is nothing per-session to read and no socket to ask. Two sources:
//
//   1. GET https://api.anthropic.com/api/oauth/usage  — the same call the CLI's own
//      `/usage` panel makes, authenticated with the OAuth token the CLI already stored.
//      This is the only network call AgentMesh makes: read-only, the user's own account.
//   2. ~/.claude.json -> cachedUsageUtilization       — the CLI persists that response,
//      but only when a human opens `/usage`, and only every 5 minutes. Good enough to
//      paint something before the first fetch resolves, useless as the primary source.
//
// Credentials are read, never written and never refreshed — the CLI rotates them on its
// own and AgentMesh always has live sessions doing exactly that. The token stays inside
// this module and the main process; only normalized numbers are handed out.

const fs = require('fs');
const path = require('path');
const os = require('os');
const { execFile } = require('child_process');

const CLAUDE_DIR = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
const CONFIG_FILE = process.env.CLAUDE_CONFIG_DIR
  ? path.join(process.env.CLAUDE_CONFIG_DIR, '.claude.json')
  : path.join(os.homedir(), '.claude.json');
const CREDENTIALS_FILE = path.join(CLAUDE_DIR, '.credentials.json');

const KEYCHAIN_SERVICE = 'Claude Code-credentials';
const ENDPOINT = 'https://api.anthropic.com/api/oauth/usage';
const OAUTH_BETA = 'oauth-2025-04-20'; // the CLI sends this on every OAuth-authed call

const FETCH_TIMEOUT = 5000; // matches the CLI's own fetchUtilization
const POLL_MS = 5 * 60 * 1000; // the CLI's own persist throttle; polite for a poller
const MIN_INTERVAL = 60 * 1000; // floor between network calls, so focus events can't storm
const CACHE_MAX_AGE = 60 * 60 * 1000; // the CLI discards its persisted copy past an hour
const MAX_BACKOFF = 30 * 60 * 1000;

// Only these keys are read out of the response. The live endpoint already returns keys
// that aren't in the CLI's own allowlist (unreleased plans, codenamed), so iterating
// whatever comes back would put mystery bars in the sidebar on someone else's machine.
//
// Order is render order, widest window first: the weekly cap is the budget, the session
// window is what's left of right now, and it sits directly above the countdown that
// belongs to it.
const WINDOWS = [
  ['seven_day', 'week'],
  ['five_hour', 'session'],
  ['seven_day_opus', 'opus'],
  ['seven_day_sonnet', 'sonnet'],
];

function safeParse(text) {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function readFileQuiet(file) {
  try {
    return fs.readFileSync(file, 'utf8');
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------- credentials

function parseCredentials(text) {
  const raw = safeParse(String(text || '').trim());
  const o = raw && raw.claudeAiOauth;
  if (!o || typeof o.accessToken !== 'string' || !o.accessToken) return null;
  return {
    accessToken: o.accessToken,
    subscriptionType: typeof o.subscriptionType === 'string' ? o.subscriptionType : null,
  };
}

function keychainRead() {
  return new Promise((resolve) => {
    execFile(
      'security',
      ['find-generic-password', '-s', KEYCHAIN_SERVICE, '-w'],
      // A timeout matters here beyond speed: if the keychain item's ACL doesn't cover us
      // the read blocks on a GUI prompt, and a hung poller is worse than a missing bar.
      { encoding: 'utf8', timeout: FETCH_TIMEOUT },
      (err, stdout) => resolve(err ? null : stdout)
    );
  });
}

async function readCredentials() {
  if (process.platform === 'darwin') {
    const creds = parseCredentials(await keychainRead());
    if (creds) return creds;
  }
  // Non-macOS, or a Keychain the CLI never wrote to.
  return parseCredentials(readFileQuiet(CREDENTIALS_FILE));
}

// The user can switch the footer — and with it this module's only network call — off
// entirely. Read through `config` on every check rather than captured once, so toggling it
// takes effect on the next poll instead of the next launch.
function disabledByUser() {
  try {
    return require('./config').get().usage.enabled === false;
  } catch {
    return false;
  }
}

// Plan limits don't apply to API-key or third-party-provider sessions — the CLI models
// this as its own `rate_limits_available` flag. Better no widget than a wrong one.
function usesOwnBilling() {
  const e = process.env;
  return Boolean(
    e.ANTHROPIC_API_KEY ||
      e.ANTHROPIC_AUTH_TOKEN ||
      e.CLAUDE_CODE_USE_BEDROCK ||
      e.CLAUDE_CODE_USE_VERTEX ||
      e.CLAUDE_CODE_USE_FOUNDRY
  );
}

// ---------------------------------------------------------------------- normalizing

// Top-level windows report utilization as 0–100 with an ISO `resets_at`. (The CLI's other,
// header-derived path uses a 0–1 fraction and epoch seconds for the same idea — hence
// normalizing at the edge rather than trusting the shape downstream.)
function pickWindow(v) {
  if (!v || typeof v !== 'object') return null;
  const pct = Number(v.utilization);
  if (!Number.isFinite(pct)) return null;
  return {
    percent: Math.max(0, Math.min(100, pct)),
    resetsAt: typeof v.resets_at === 'string' ? v.resets_at : null,
  };
}

function normalize(body, { plan, fetchedAt, stale }) {
  if (!body || typeof body !== 'object') return null;

  const windows = [];
  for (const [key, label] of WINDOWS) {
    const w = pickWindow(body[key]);
    if (w) windows.push({ key, label, percent: w.percent, resetsAt: w.resetsAt });
  }
  if (!windows.length) return null;

  // Per-model weekly windows only exist on some plans; they arrive in limits[] carrying
  // the display name the CLI itself labels them with.
  const perModel = [];
  if (Array.isArray(body.limits)) {
    for (const l of body.limits) {
      const name = l && l.scope && l.scope.model && l.scope.model.display_name;
      const pct = Number(l && l.percent);
      if (typeof name !== 'string' || !Number.isFinite(pct)) continue;
      perModel.push({
        label: name,
        percent: Math.max(0, Math.min(100, pct)),
        resetsAt: typeof l.resets_at === 'string' ? l.resets_at : null,
      });
    }
  }

  // `Number(null)` is 0, not NaN — an unset credit limit would read as a limit of zero.
  const num = (v) => (v == null || !Number.isFinite(Number(v)) ? null : Number(v));
  const x = body.extra_usage;
  const extraUsage =
    x && typeof x === 'object'
      ? {
          enabled: x.is_enabled === true,
          usedCredits: num(x.used_credits),
          monthlyLimit: num(x.monthly_limit),
          currency: typeof x.currency === 'string' ? x.currency : null,
        }
      : null;

  return { plan: plan || null, windows, perModel, extraUsage, fetchedAt, stale: Boolean(stale) };
}

// ---------------------------------------------------------------------- local cache

// The CLI persists the raw response body here, so it feeds the same normalizer. It only
// gets written when someone opens `/usage`, which is why this can't be the primary read.
function readCachedBody() {
  const raw = safeParse(readFileQuiet(CONFIG_FILE) || '');
  const c = raw && raw.cachedUsageUtilization;
  if (!c || typeof c.fetchedAtMs !== 'number') return null;
  if (Date.now() - c.fetchedAtMs > CACHE_MAX_AGE) return null;
  return { body: c.utilization, fetchedAt: c.fetchedAtMs };
}

// ---------------------------------------------------------------------------- fetch

async function fetchUsage(token) {
  const res = await fetch(ENDPOINT, {
    headers: {
      Authorization: `Bearer ${token}`,
      'anthropic-beta': OAUTH_BETA,
      'Content-Type': 'application/json',
    },
    signal: AbortSignal.timeout(FETCH_TIMEOUT),
  });
  if (res.status === 401) return { unauthorized: true };
  if (!res.ok) throw new Error(`usage: HTTP ${res.status}`);
  return { body: await res.json() };
}

// ----------------------------------------------------------------------- the poller

let last = null; // last normalized value we handed out
let lastFetchAt = 0;
let failures = 0;
let inflight = null;

function stale(value) {
  return value ? { ...value, stale: true } : null;
}

function fallback() {
  // Off means off, including the copy the CLI already left on disk — a footer painted from
  // cache would read as the app having fetched it.
  if (disabledByUser()) return null;
  if (last) return stale(last);
  const cached = readCachedBody();
  if (!cached) return null;
  return normalize(cached.body, { plan: null, fetchedAt: cached.fetchedAt, stale: true });
}

async function load() {
  if (disabledByUser() || usesOwnBilling()) return null;

  let creds = await readCredentials();
  if (!creds) return fallback();

  try {
    let res = await fetchUsage(creds.accessToken);
    if (res.unauthorized) {
      // The CLI may have rotated the token between our keychain read and the request.
      creds = await readCredentials();
      res = creds ? await fetchUsage(creds.accessToken) : { unauthorized: true };
    }
    if (res.unauthorized) throw new Error('usage: unauthorized');

    const value = normalize(res.body, {
      plan: creds.subscriptionType,
      fetchedAt: Date.now(),
      stale: false,
    });
    lastFetchAt = Date.now();
    failures = 0;
    if (value) last = value;
    return value || fallback();
  } catch {
    failures++;
    return fallback();
  }
}

// `force` bypasses the floor between network calls — a manual click should always go out,
// a window-focus event should not.
function refresh({ force = false } = {}) {
  if (!force && last && Date.now() - lastFetchAt < MIN_INTERVAL) return Promise.resolve(last);
  if (inflight) return inflight;
  inflight = load().finally(() => {
    inflight = null;
  });
  return inflight;
}

function snapshot() {
  return last;
}

// Poll on a slow timer, backing off on repeated failure so a revoked token or a dead
// network doesn't turn into a request every five minutes forever.
function poll(onUpdate) {
  let timer = null;
  let stopped = false;

  const tick = async () => {
    const value = await refresh({ force: true });
    if (stopped) return;
    onUpdate(value);
    const delay = failures ? Math.min(POLL_MS * 2 ** failures, MAX_BACKOFF) : POLL_MS;
    timer = setTimeout(tick, delay);
  };

  // Paint whatever the CLI already left on disk, then go ask for the real thing.
  const seeded = fallback();
  if (seeded) onUpdate(seeded);
  tick();

  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
  };
}

module.exports = { refresh, snapshot, poll, normalize };
