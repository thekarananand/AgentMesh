#!/bin/bash
# Build AgentMesh.app from the release binary. Usage: scripts/bundle.sh [--install]
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release
APP=target/release/bundle/AgentMesh.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/agentmesh "$APP/Contents/MacOS/AgentMesh"

# assets/icon.png is rendered from assets/icon.svg (brew install resvg)
if command -v resvg >/dev/null; then resvg -w 1024 -h 1024 assets/icon.svg assets/icon.png; fi
ICONSET=$(mktemp -d)/icon.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>AgentMesh</string>
  <key>CFBundleDisplayName</key><string>AgentMesh</string>
  <key>CFBundleExecutable</key><string>AgentMesh</string>
  <key>CFBundleIdentifier</key><string>dev.agentmesh.app</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0.1</string>
  <key>CFBundleVersion</key><string>1.0.1</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST

codesign --force --deep --sign - "$APP"
echo "built $APP"

if [ "${1:-}" = "--install" ]; then
  rm -rf /Applications/AgentMesh.app
  cp -R "$APP" /Applications/AgentMesh.app
  echo "installed /Applications/AgentMesh.app"
fi
