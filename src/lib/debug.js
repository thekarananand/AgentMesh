// Debug logging for the main process, gated on the same flag CLAUDE.md already documents
// for seeing renderer console output: `ELECTRON_ENABLE_LOGGING=1 npm start`. That flag
// alone makes the *renderer's* console.log reach the terminal (Electron's own doing); it
// does nothing for the main process, whose console.log always reaches stdout regardless.
// This module closes that gap so one flag turns on diagnostic output for both, and a
// plain `npm start` stays exactly as quiet as it already was.
//
// Pure Node, no Electron import — same standalone-testable shape as the rest of `lib/`.

const enabled = Boolean(process.env.ELECTRON_ENABLE_LOGGING);

function debugLog(...args) {
  if (enabled) console.log('[debug:main]', ...args);
}

module.exports = { debugLog };
