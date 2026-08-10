#!/usr/bin/env node
//
// Rasterizes build/icon.svg to build/icon.png (1024², the one file electron-builder needs —
// it derives the .icns, the .ico and every Linux size from it).
//
// The PNG is committed, so this is never part of a build. It exists so the icon has a
// source of truth that can be edited as vector art instead of a binary nobody can change.
//
// macOS only, because the rasterizer is Quick Look — the one SVG renderer that is already
// on the machine. Anywhere else, edit the SVG and convert it with whatever is at hand
// (`rsvg-convert -w 1024 -h 1024`, Inkscape, a browser screenshot), then commit the PNG.

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const BUILD = path.join(__dirname, '..', 'build');
const svg = path.join(BUILD, 'icon.svg');
const out = path.join(BUILD, 'icon.png');

if (process.platform !== 'darwin') {
  console.error('make-icon needs macOS (uses qlmanage). Convert build/icon.svg by hand and commit build/icon.png.');
  process.exit(1);
}
if (!fs.existsSync(svg)) {
  console.error(`missing ${svg}`);
  process.exit(1);
}

// qlmanage always names its output `<input>.png` and offers no way to change that.
execFileSync('qlmanage', ['-t', '-s', '1024', '-o', BUILD, svg], { stdio: 'ignore' });
const produced = path.join(BUILD, 'icon.svg.png');
if (!fs.existsSync(produced)) {
  console.error('qlmanage produced nothing — is the SVG valid?');
  process.exit(1);
}
fs.renameSync(produced, out);

const { size } = fs.statSync(out);
console.log(`wrote ${path.relative(process.cwd(), out)} (${Math.round(size / 1024)}KB)`);
