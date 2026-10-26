#!/usr/bin/env bash
# Assembles Prizrak-Box.app (Rust + Slint shell) at the repo root — the layout
# src-wails/build/darwin/create-macos-dmg.mjs packages into the DMG.
#   src-rs/packaging/macos/build-app.sh <path-to-prizrak-box-binary> [version]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
BIN="${1:?binary path}"
VER="${2:-}"
APP="$ROOT/Prizrak-Box.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Prizrak-Box"
cp "$ROOT/src-go/px" "$ROOT/src-service/px-service" "$APP/Contents/MacOS/"
chmod +x "$APP/Contents/MacOS/"*
cp "$ROOT/build/appicon.icns" "$APP/Contents/Resources/appicon.icns"
cp "$ROOT/src-rs/packaging/macos/Info.plist" "$APP/Contents/Info.plist"
if [ -n "$VER" ]; then
  VER="${VER#v}"
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VER" -c "Set :CFBundleShortVersionString $VER" "$APP/Contents/Info.plist" || true
fi
# Ad-hoc signature so the bundle launches locally; CI replaces it with the
# Developer ID signature when the secrets are present.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
echo "Built $APP"
