#!/usr/bin/env node
//
// Rasterizes build/icon.svg to build/icon.png (1024², the one file electron-builder needs —
// it derives the .icns, the .ico and every Linux size from it).
//
// The PNG is committed, so this is never part of a build. It exists so the icon has a
// source of truth that can be edited as vector art instead of a binary nobody can change.
//
// Rendered with headless Chrome, not qlmanage: qlmanage flattens the SVG's transparent
// background to opaque white instead of preserving alpha. That white baked straight into
// build/icon.png and every size electron-builder derived from it, showing up as a white
// halo behind the icon's rounded corners in the compiled app. Chrome renders the same SVG
// with real transparent corners. Anywhere without a Chromium browser, convert build/icon.svg
// by hand (`rsvg-convert -w 1024 -h 1024`, Inkscape, a browser screenshot), then commit the PNG.

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const BUILD = path.join(__dirname, '..', 'build');
const svg = path.join(BUILD, 'icon.svg');
const out = path.join(BUILD, 'icon.png');

const BROWSER_PATHS = [
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
  '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
];

if (!fs.existsSync(svg)) {
  console.error(`missing ${svg}`);
  process.exit(1);
}

const browser = BROWSER_PATHS.find((p) => fs.existsSync(p));
if (!browser) {
  console.error('make-icon needs a Chromium-based browser (Chrome, Chromium, or Edge) for headless rendering. Convert build/icon.svg by hand and commit build/icon.png.');
  process.exit(1);
}

execFileSync(browser, [
  '--headless=new',
  '--disable-gpu',
  '--hide-scrollbars',
  '--default-background-color=00000000',
  '--window-size=1024,1024',
  `--screenshot=${out}`,
  `file://${svg}`,
], { stdio: 'ignore' });

if (!fs.existsSync(out)) {
  console.error('headless browser produced nothing — is the SVG valid?');
  process.exit(1);
}

const { size } = fs.statSync(out);
console.log(`wrote ${path.relative(process.cwd(), out)} (${Math.round(size / 1024)}KB)`);
