// Finding the Claude Code CLI, and knowing what we found.
//
// Every tab is a `claude` process, and until now the app just wrote the bare word `claude`
// into a login shell and hoped. When that hope was wrong the shell exited 127, the pty
// closed, and the user saw a tab flash and vanish with nothing to read — the same picture a
// crash makes. On this machine the binary is a Homebrew cask; on someone else's it is an npm
// global, a curl installer, a Nix profile or absent. So: resolve it once, keep what was
// found, and let the UI say so when there is nothing.
//
// Deliberately *not* here: a version→feature table gating `--fork-session`, `agents --cwd`
// and `--name`. Those flags were read out of one shipped binary (see CLAUDE.md); no version
// range has been verified for any of them, and a made-up threshold would disable working
// features on some machines and fail confusingly on others. The version is recorded and
// surfaced instead, so a bug report says which CLI produced it.

const { execFile } = require('child_process');
const platform = require('./platform');

const TIMEOUT = 5000;

let cached = null; // { found, path, version, source }
let inflight = null;

function run(cmd) {
  const { bin, args } = platform.shellCapture(cmd);
  return new Promise((resolve) => {
    execFile(bin, args, { encoding: 'utf8', timeout: TIMEOUT, windowsHide: true }, (err, stdout) =>
      // `claude --version` prints the version and still exits nonzero, so the exit code is
      // not evidence of anything here. Read stdout and judge by what came back.
      resolve(String(stdout || '').trim())
    );
  });
}

function locateCommand() {
  return platform.isWindows
    ? 'Get-Command claude -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source'
    : 'command -v claude 2>/dev/null';
}

async function probeVersion(binPath) {
  const out = await run(`${platform.quoteArg(binPath)} --version`);
  const m = out.match(/\d+\.\d+\.\d+/);
  return m ? m[0] : null;
}

// `override` is the user's configured path, which wins over PATH entirely — that is the
// escape hatch for a CLI installed somewhere a login shell doesn't look.
async function load(override) {
  if (override) {
    const version = await probeVersion(override);
    if (version) return { found: true, path: override, version, source: 'config' };
    // A configured path that doesn't answer is worth naming rather than silently falling
    // back to PATH and behaving as though the setting did nothing.
    return { found: false, path: override, version: null, source: 'config-broken' };
  }

  const located = (await run(locateCommand())).split('\n')[0].trim();
  if (!located) return { found: false, path: null, version: null, source: 'path' };
  return { found: true, path: located, version: await probeVersion(located), source: 'path' };
}

function resolve({ override = null, force = false } = {}) {
  if (cached && !force) return Promise.resolve(cached);
  if (inflight) return inflight;
  inflight = load(override)
    .then((info) => {
      cached = info;
      return info;
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

// What `spawnTab` puts at the front of the pty command. The absolute path when we have one:
// the login shell would usually find the same binary, but resolving once means every tab
// runs the CLI we probed rather than whatever a later PATH edit happens to shadow it with.
// Bare `claude` is the fallback, which keeps the old behavior when resolution failed but
// the binary is somehow there anyway.
function command() {
  return cached && cached.found && cached.path ? platform.quoteArg(cached.path) : 'claude';
}

function info() {
  return cached || { found: false, path: null, version: null, source: 'unresolved' };
}

module.exports = { resolve, command, info };
