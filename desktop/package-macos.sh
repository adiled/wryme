#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?usage: package-macos.sh <version> [arch]}"
RAW_ARCH="${2:-$(uname -m)}"
case "$RAW_ARCH" in
    aarch64*|arm64*) ARCH="arm64" ;;
    x86_64*) ARCH="x86_64" ;;
    *) ARCH="$RAW_ARCH" ;;
esac
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STAGE="dist/wryme-desktop/wme.app"

rm -rf dist/wryme-desktop
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources"

cp desktop/config/wezterm.lua "$STAGE/Contents/Resources/wezterm.lua"
cp desktop/macos/wryme-launcher.sh "$STAGE/Contents/MacOS/wryme-launcher"
chmod +x "$STAGE/Contents/MacOS/wryme-launcher"
if [[ -f desktop/macos/AppIcon.icns ]]; then
    cp desktop/macos/AppIcon.icns "$STAGE/Contents/Resources/AppIcon.icns"
fi

WEZTERM_BUNDLE="$STAGE/Contents/Resources/WezTerm.app"
if [[ ! -d "$WEZTERM_BUNDLE" ]]; then
    echo "bundling WezTerm..."
    TMP_WEZ="$(mktemp -d)"
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
                original_entitlements="/tmp/wryme-ent-$$.plist"
                codesign -d --entitlements "$original_entitlements" "$FOUND" 2>/dev/null || codesign -d --entitlements "$original_entitlements" "$FOUND/Contents/MacOS/wezterm-gui" 2>/dev/null || rm -f "$original_entitlements" 2>/dev/null || true
                if [[ ! -s "$original_entitlements" && -d "/Applications/WezTerm.app" ]]; then
                    codesign -d --entitlements "$original_entitlements" "/Applications/WezTerm.app" 2>/dev/null || true
                fi
                ditto "$FOUND" "$WEZTERM_BUNDLE" 2>/dev/null || cp -R "$FOUND" "$WEZTERM_BUNDLE"
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
                if [[ -s "$original_entitlements" ]]; then
                    codesign --force --deep --sign - --entitlements "$original_entitlements" "$WEZTERM_BUNDLE" >/dev/null 2>&1 || codesign --force --deep --sign - "$WEZTERM_BUNDLE" >/dev/null 2>&1 || true
                else
                    codesign --force --options runtime --deep --sign - "$WEZTERM_BUNDLE" >/dev/null 2>&1 || codesign --force --deep --sign - "$WEZTERM_BUNDLE" >/dev/null 2>&1 || true
                fi
                rm -f "$original_entitlements" 2>/dev/null || true
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

codesign --force --deep --sign - "$STAGE" >/dev/null 2>&1 || echo "codesign skipped"

(cd dist/wryme-desktop && rm -f "../wryme-darwin-$ARCH.zip" && zip -qry "../wryme-darwin-$ARCH.zip" wme.app)
echo "built dist/wryme-darwin-$ARCH.zip"
