#!/usr/bin/env bash
# Builds "Claude Usage Widget.app" (universal: Apple silicon + Intel) and a
# .dmg into dist/. Run on macOS:  installer/macos/build-app.sh 1.4.0
#
# The app is ad-hoc signed, not notarized (that needs a paid Apple Developer
# account), so macOS asks for confirmation the first time it opens.
set -euo pipefail
VERSION="$1"

cargo build --release --locked --target aarch64-apple-darwin
cargo build --release --locked --target x86_64-apple-darwin

ROOT=dist/mac
APP="$ROOT/Claude Usage Widget.app"
rm -rf "$ROOT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/claude-usage-widget" \
  target/aarch64-apple-darwin/release/claude-usage-widget \
  target/x86_64-apple-darwin/release/claude-usage-widget
sed "s/__VERSION__/$VERSION/g" installer/macos/Info.plist > "$APP/Contents/Info.plist"

ICONSET="$(mktemp -d)/icon.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256; do
  sips -z "$s" "$s" assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

codesign --force --deep --sign - "$APP"

cp LICENSE "$APP/Contents/Resources/LICENSE"
ln -s /Applications "$ROOT/Applications"
mkdir -p dist
hdiutil create -volname "Claude Usage Widget" -srcfolder "$ROOT" -ov -format UDZO \
  "dist/ClaudeUsageWidget-$VERSION-macos.dmg"
