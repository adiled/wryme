#!/usr/bin/env bash
#
# Package the wryme desktop bundle for macOS.
#
# Produces: wryme-darwin-<arch>.zip  (contains wme.app)
#
# Usage: package-macos.sh <version> [arch]
#   arch defaults to `uname -m` (arm64 on Apple Silicon runners, x86_64 on Intel)

set -euo pipefail

VERSION="${1:?usage: package-macos.sh <version> [arch]}"
ARCH="${2:-$(uname -m)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIN="target/release/wme"
STAGE="dist/wryme-desktop/wme.app"

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

# --- Bundle WezTerm inside wme.app (fully self-contained, no download) ---
WEZTERM_BUNDLE="$STAGE/Contents/Resources/WezTerm.app"
if [[ ! -d "$WEZTERM_BUNDLE" ]]; then
    echo "bundling WezTerm..."
    TMP_WEZ="$(mktemp -d)"
    # Resolve latest WezTerm zip (universal, ~30MB) via GitHub API
    WEZ_URL="$(python3 -c "
import json, urllib.request, sys
try:
    with urllib.request.urlopen('https://api.github.com/repos/wezterm/wezterm/releases/latest', timeout=15) as r:
        d=json.load(r)
        for a in d.get('assets',[]):
            n=a.get('name','')
            if n.startswith('WezTerm-macos-') and n.endswith('.zip'):
                print(a['browser_download_url']); sys.exit(0)
except Exception as e:
    print(f'wezterm api err: {e}', file=sys.stderr)
    sys.exit(1)
" 2>/dev/null || true)"
    if [[ -z "$WEZ_URL" ]]; then
        echo "warn: could not resolve WezTerm URL, wme.app will fallback to system WezTerm" >&2
    else
        if curl -fsSL --max-time 120 "$WEZ_URL" -o "$TMP_WEZ/wezterm.zip"; then
            unzip -q "$TMP_WEZ/wezterm.zip" -d "$TMP_WEZ" 2>/dev/null || echo "unzip WezTerm failed" >&2
            FOUND="$(find "$TMP_WEZ" -maxdepth 3 -name "WezTerm.app" -type d | head -1 || true)"
            if [[ -n "$FOUND" ]]; then
                ditto "$FOUND" "$WEZTERM_BUNDLE" 2>/dev/null || cp -R "$FOUND" "$WEZTERM_BUNDLE"
                # Brand the bundled WezTerm with our W so Dock groups under W
                if [[ -f desktop/macos/AppIcon.icns ]]; then
                    cp desktop/macos/AppIcon.icns "$WEZTERM_BUNDLE/Contents/Resources/terminal.icns" 2>/dev/null || true
                    cp desktop/macos/AppIcon.icns "$WEZTERM_BUNDLE/Contents/Resources/AppIcon.icns" 2>/dev/null || true
                    /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier sh.wryme.wezterm" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || /usr/libexec/PlistBuddy -c "Add :CFBundleIdentifier string sh.wryme.wezterm" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || true
                    /usr/libexec/PlistBuddy -c "Set :CFBundleName wme" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || true
                    /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName wme" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || true
                    /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile AppIcon" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || true
                    /usr/libexec/PlistBuddy -c "Set :CFBundleIconName AppIcon" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" "$WEZTERM_BUNDLE/Contents/Info.plist" 2>/dev/null || true
                fi
                xattr -dr com.apple.quarantine "$WEZTERM_BUNDLE" 2>/dev/null || true
            else
                echo "warn: WezTerm.app not found in zip" >&2
            fi
        else
            echo "warn: WezTerm download failed, wme.app will fallback to system WezTerm" >&2
        fi
    fi
    rm -rf "$TMP_WEZ" 2>/dev/null || true
fi

sed "s/1\.1\.4/$VERSION/g" desktop/macos/Info.plist > "$STAGE/Contents/Info.plist"

# Ad-hoc codesign so the app runs on other Macs without extra prompts where
# possible (Gatekeeper may still warn on first open of an unsigned download).
codesign --force --deep --sign - "$STAGE" >/dev/null 2>&1 || echo "codesign skipped"

(cd dist/wryme-desktop && zip -qry "../wryme-darwin-$ARCH.zip" wme.app)
echo "built dist/wryme-darwin-$ARCH.zip"