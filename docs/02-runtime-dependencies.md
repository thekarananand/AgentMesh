# Runtime Dependencies

Complete list of what the app assumes at runtime, how it's satisfied, and what happens when it's missing.

| Dependency | How it's satisfied | If missing |
|---|---|---|
| Claude Code CLI (`claude`) | Resolved at startup via login shell, or `claudeBin` in settings | New-session controls disable, welcome pane and sidebar say why |
| Fonts (JetBrains Mono Nerd Font, Inter) | **Bundled** in `assets/fonts` as woff2, `@font-face` in `index.html` | n/a — cannot be missing |
| App icon | **Bundled** as committed `build/icon.png` | n/a |
| `node-pty` native binary | `npm install` postinstall runs `electron-builder install-app-deps` | Build fails loudly |
| Process list (`ps` / PowerShell CIM) | `platform.parentMap()` | Rows stop binding to tabs; a warning names it |
| Per-session control socket | Path comes from the CLI's own pid registry | Rename falls back to transcript records |
| macOS Keychain / `.credentials.json` | `usage.js`, platform-branched | Usage footer hides |

## Authoring-only dependencies

Authoring-only tools (`npm run fonts`, `npm run icon`) may depend on whatever is convenient —
their outputs are committed, so no one else ever runs them.

## Portability checklist

When adding a dependency:
1. **Verify it's bundled, checked at runtime, or documented** in this table
2. **Test on a fresh machine** if possible
3. **Add the failure case** to this table
4. **Update README.md** if the user must install something

See [00-intent.md](00-intent.md) for the portability philosophy.
