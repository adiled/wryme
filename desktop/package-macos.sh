#!/usr/bin/env bash
#
# Package the wryme desktop bundle for macOS.
#
# Produces: wryme-darwin-<arch>.zip  (contains Wryme.app)
#
# Usage: package-macos.sh <version> [arch]
#   arch defaults to `uname -m` (arm64 on Apple Silicon runners, x86_64 on Intel)

set -euo pipefail

VERSION="${1:?usage: package-macos.sh <version> [arch]}"
ARCH="${2:-$(uname -m)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIN="target/release/wme"
STAGE="dist/wryme-desktop/Wryme.app"

[[ -x "$BIN" ]] || { echo "missing built binary: $BIN (run cargo build --release first)" >&2; exit 1; }

rm -rf dist/wryme-desktop
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources"

cp "$BIN" "$STAGE/Contents/Resources/wme"
cp desktop/config/wezterm.lua "$STAGE/Contents/Resources/wezterm.lua"
cp desktop/macos/wryme-launcher.sh "$STAGE/Contents/MacOS/wryme-launcher"
chmod +x "$STAGE/Contents/MacOS/wryme-launcher"
if [[ -f desktop/macos/AppIcon.icns ]]; then
    cp desktop/macos/AppIcon.icns "$STAGE/Contents/Resources/AppIcon.icns"
fi

sed "s/1\.1\.4/$VERSION/g" desktop/macos/Info.plist > "$STAGE/Contents/Info.plist"

# Ad-hoc codesign so the app runs on other Macs without extra prompts where
# possible (Gatekeeper may still warn on first open of an unsigned download).
codesign --force --deep --sign - "$STAGE" >/dev/null 2>&1 || echo "codesign skipped"

(cd dist/wryme-desktop && zip -qry "../wryme-darwin-$ARCH.zip" Wryme.app)
echo "built dist/wryme-darwin-$ARCH.zip"