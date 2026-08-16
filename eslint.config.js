const js = require('@eslint/js');
const globals = require('globals');

module.exports = [
  js.configs.recommended,
  {
    ignores: ['dist/**', 'node_modules/**'],
  },
  {
    rules: {
      // Deliberate pattern throughout this codebase: e.g. `try { win.webContents.send(...) }
      // catch {}` around calls into a window that may already be destroyed, where the
      // failure is expected and there is nothing useful to do with it.
      'no-empty': ['error', { allowEmptyCatch: true }],
    },
  },
  {
    files: ['src/main/**/*.js', 'src/preload/**/*.js', 'src/lib/**/*.js', 'tools/**/*.js', 'eslint.config.js'],
    languageOptions: {
      sourceType: 'commonjs',
      ecmaVersion: 2022,
      globals: {
        ...globals.node,
      },
    },
  },
  {
    // Loaded via <script> in src/renderer/index.html, not a CommonJS module — Terminal and
    // CanvasAddon come from the xterm/xterm-addon-canvas <script> tags loaded just before it.
    files: ['src/renderer/renderer.js'],
    languageOptions: {
      sourceType: 'script',
      ecmaVersion: 2022,
      globals: {
        ...globals.browser,
        Terminal: 'readonly',
        CanvasAddon: 'readonly',
      },
    },
  },
];
