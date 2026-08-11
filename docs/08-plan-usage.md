# Plan Usage — The Sidebar Footer (usage.js)

The one **account-wide** fact in an app otherwise built out of per-session facts, which is 
why it lives in its own footer strip rather than on any row.

Running six agents at once burns the 5-hour window fast and the CLI can only answer for it 
from *inside* a session (`/usage`) — the wrong shape for a question about the account.

## API and credentials

**Source: `GET https://api.anthropic.com/api/oauth/usage`** — Exactly what the CLI's own 
`fetchUtilization` calls (5s timeout, `Content-Type: application/json`, 
`anthropic-beta: oauth-2025-04-20`).

### Response shape

```json
{
  "five_hour": {
    "utilization": 8.0,
    "resets_at": "2026-08-10T00:00:00.292938+00:00"
  },
  "seven_day": {
    "utilization": 21.0,
    "resets_at": "2026-08-15T15:59:59.292960+00:00"
  },
  "seven_day_opus": null,
  "seven_day_sonnet": null,
  "extra_usage": {
    "is_enabled": false,
    "monthly_limit": null,
    "used_credits": null
  },
  "limits": [...],
  "spend": {...}
}
```

## Unit conventions

**Two unit conventions for one idea:**
- Top-level windows: `utilization` 0–100 with ISO `resets_at`
- Response headers (`anthropic-ratelimit-unified-*`): 0–1 fraction and epoch seconds

`normalize()` is the only place either shape is touched; nothing downstream may assume one.

## Allowlist approach

Read the response by allowlist. The live endpoint already returns keys the CLI's own list 
doesn't cover (`tangelo`, `nimbus_quill`, `amber_ladder`, `seven_day_cowork`, 
`seven_day_omelette` — unreleased, codenamed). Iterating whatever comes back would put 
mystery bars in someone else's sidebar.

`WINDOWS` in `usage.js` is that allowlist, and its order is the render order.

## Credential handling

- **Read, never written, never refreshed.** macOS Keychain (service `Claude Code-credentials`, 
  one JSON blob: `{"claudeAiOauth":{accessToken, refreshToken, expiresAt, scopes, 
  subscriptionType:"pro", rateLimitTier}}`)
- **Fallback**: `~/.claude/.credentials.json` (the path the binary itself references)
- **No rotation** — the CLI does that, and AgentMesh always has live sessions making it happen
- `subscriptionType` is the plan chip
- The token stays inside `usage.js` and the main process; only normalized numbers cross 
  the contextBridge
- Security read carries a timeout on purpose: if the Keychain item's ACL doesn't cover us 
  the read blocks on a **GUI prompt**, and a hung poller is worse than a missing bar

## Local caching

`~/.claude.json → cachedUsageUtilization` holds the same response body, but it's written 
from `loadPlanRateLimits` — i.e. **only when a human opens `/usage`**, throttled to one 
write per 5 min (`yey=300000`) and discarded on read past 1 h (`_ey=3600000`).

**Why the local-only path can't be the primary:**
- Absent entirely on fresh machines
- Good as a cold-start seed and an offline fallback, nothing more
- Header-derived utilization never touches disk at all — it lives in CLI process memory

## Polling strategy

- **Base interval**: 5 min (the CLI's own persist throttle)
- **Window focus trigger**: re-check on app focus
- **Tab spawn trigger**: re-check on new tab
- **Minimum interval**: 60s floor between actual network calls (alt-tabbing can't turn into traffic)
- **Failure backoff**: exponentially to 30 min
- **Stale fallback**: keep serving the last good value flagged `stale` — footer dims rather 
  than vanishing
- **401 recovery**: re-read credentials once and retry once, since the CLI may have rotated 
  the token underneath us
- **No-op on API keys**: `ANTHROPIC_API_KEY` / Bedrock / Vertex / Foundry in the env means 
  plan limits don't apply, and the footer renders nothing at all

## UI rendering

- **Widest window first** — `week` on top, `session` under it, sitting directly above the 
  countdown that belongs to it
- **Status colors** — green / amber ≥75% / red ≥90%, the same three-state vocabulary as row 
  status glyphs
- **Other windows' reset dates** live in each row's hover title so they don't fight for the 
  one line underneath

### Rendering implementation

`renderUsage()` is **not** part of `render()`:
- Full row rebuild happens on 4s session poll
- Usage arrives on its own 5-minute cadence
- The countdown is the **one timer in the sidebar** — 60s, all the precision `3h 12m` has, 
  writing a single text node
- Costs one assignment instead of a full repaint
