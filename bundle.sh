#!/bin/bash
# Builds minitype.app, signs it with a local code-signing identity and installs it
# to ~/Applications. Usage: ./bundle.sh ["Signing Identity Name"]
set -euo pipefail
cd "$(dirname "$0")"

IDENTITY="${1:-${MINITYPE_SIGNING_IDENTITY:-SpeakType Local Signing}}"
APP="target/release/minitype.app"
DEST="$HOME/Applications/minitype.app"

cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/minitype "$APP/Contents/MacOS/minitype"
cp macos/Info.plist "$APP/Contents/Info.plist"

codesign --force --options runtime --timestamp=none \
  --entitlements macos/Entitlements.plist \
  --sign "$IDENTITY" "$APP"
codesign --verify --strict "$APP"

pkill -f "$DEST/Contents/MacOS/minitype" 2>/dev/null || true
mkdir -p "$HOME/Applications"
rm -rf "$DEST"
ditto "$APP" "$DEST"
echo "Installed $DEST (signed by: $IDENTITY)"
