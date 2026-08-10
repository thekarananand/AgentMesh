#!/usr/bin/env node
//
// Regenerates `assets/fonts/*.woff2` from TrueType originals.
//
// The .woff2 files are committed, so nobody needs to run this to build or use the app —
// it exists so the vendored fonts are reproducible rather than binaries of unknown
// provenance, and so a version bump is one command instead of a manual conversion.
//
//   node tools/build-fonts.js [source-dir]
//
// Source dir defaults to the platform's user font directory, which is where the upstream
// installers (Homebrew casks, the Nerd Fonts release zips, the Inter release zip) put
// them. Anything already present is skipped unless --force is passed.
//
// Licenses: JetBrains Mono and Inter are both SIL OFL 1.1, and the Nerd Fonts patch keeps
// the upstream license of the font it patches. That is what makes vendoring them legal;
// the license texts ship next to them in assets/fonts/ and must stay there.

const fs = require('fs');
const os = require('os');
const path = require('path');
const { compress } = require('wawoff2');

const OUT_DIR = path.join(__dirname, '..', 'assets', 'fonts');

// One entry per face the app actually asks for. Adding a weight here is not free — every
// face is bytes in the packaged app — so this list stays as short as the design needs:
// the terminal renders regular/bold/italic/bold-italic, and Inter is variable, so its two
// files cover every weight the UI uses.
const FACES = [
  'JetBrainsMonoNerdFontMono-Regular.ttf',
  'JetBrainsMonoNerdFontMono-Bold.ttf',
  'JetBrainsMonoNerdFontMono-Italic.ttf',
  'JetBrainsMonoNerdFontMono-BoldItalic.ttf',
  'InterVariable.ttf',
  'InterVariable-Italic.ttf',
];

function defaultSourceDir() {
  if (process.platform === 'darwin') return path.join(os.homedir(), 'Library', 'Fonts');
  if (process.platform === 'win32') {
    return path.join(process.env.LOCALAPPDATA || os.homedir(), 'Microsoft', 'Windows', 'Fonts');
  }
  return path.join(os.homedir(), '.local', 'share', 'fonts');
}

async function main() {
  const args = process.argv.slice(2);
  const force = args.includes('--force');
  const srcDir = args.find((a) => !a.startsWith('--')) || defaultSourceDir();

  fs.mkdirSync(OUT_DIR, { recursive: true });

  let converted = 0;
  let missing = 0;

  for (const face of FACES) {
    const src = path.join(srcDir, face);
    const out = path.join(OUT_DIR, face.replace(/\.ttf$/i, '.woff2'));

    if (!force && fs.existsSync(out)) {
      console.log(`skip    ${path.basename(out)} (exists)`);
      continue;
    }
    if (!fs.existsSync(src)) {
      console.log(`MISSING ${src}`);
      missing++;
      continue;
    }

    const ttf = fs.readFileSync(src);
    const woff2 = Buffer.from(await compress(ttf));
    fs.writeFileSync(out, woff2);
    const pct = Math.round((woff2.length / ttf.length) * 100);
    console.log(
      `write   ${path.basename(out)}  ${(ttf.length / 1024 / 1024).toFixed(2)}MB → ` +
        `${(woff2.length / 1024 / 1024).toFixed(2)}MB (${pct}%)`
    );
    converted++;
  }

  console.log(`\n${converted} converted, ${missing} missing, source: ${srcDir}`);
  if (missing) {
    console.log(
      'Install the originals first — see README.md. The app does not need them at runtime;\n' +
        'only this script does, and only when regenerating.'
    );
    process.exitCode = 1;
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
