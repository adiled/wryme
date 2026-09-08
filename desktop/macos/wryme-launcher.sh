#!/bin/bash
#
# wryme desktop launcher — macOS
#
# Opens wryme in a clean, app-like WezTerm window. No terminal chrome.
#
# Usage: this script is dropped inside Wryme.app/Contents/MacOS and is the
# app's executable. It works from any location as long as `wme` and
# `wezterm.lua` live in the bundle's Resources.

set -euo pipefail

# Bundle layout:
#   Wryme.app/Contents/MacOS/wryme-launcher   <- this script
#   Wryme.app/Contents/Resources/wme          <- the wryme binary
#   Wryme.app/Contents/Resources/wezterm.lua  <- app-like WezTerm config
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESC="$SCRIPT_DIR/../Resources"
WME="$RESC/wme"
CFG="$RESC/wezterm.lua"

# --- Locate wezterm ---------------------------------------------------------
find_wezterm() {
    local bin
    if bin="$(command -v wezterm 2>/dev/null)"; then
        echo "$bin"; return 0
    fi
    for p in \
        "/Applications/WezTerm.app/Contents/MacOS/wezterm" \
        "$HOME/Applications/WezTerm.app/Contents/MacOS/wezterm"; do
        if [[ -x "$p" ]]; then echo "$p"; return 0; fi
    done
    return 1
}

WEZTERM="$(find_wezterm || true)"

install_wezterm_direct() {
    local tmp dest url zip
    tmp="$(mktemp -d)"
    trap "rm -rf \"$tmp\"" RETURN

    # Resolve latest WezTerm zip URL via GitHub API (zip is universal, no arch split).
    url="$(python3 -c '
import json, urllib.request, sys
try:
    with urllib.request.urlopen("https://api.github.com/repos/wezterm/wezterm/releases/latest", timeout=15) as r:
        data = json.load(r)
        for a in data.get("assets", []):
            n = a.get("name","")
            if n.startswith("WezTerm-macos-") and n.endswith(".zip"):
                print(a["browser_download_url"])
                sys.exit(0)
except Exception as e:
    print(f"api error: {e}", file=sys.stderr)
    sys.exit(1)
' 2>/dev/null || true)"

    if [[ -z "$url" ]]; then
        osascript -e 'display dialog "Could not find WezTerm download. Install it from https://wezterm.org/install/macos.html then open wryme again." buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
        exit 1
    fi

    if ! curl -fL --progress-bar "$url" -o "$tmp/wezterm.zip"; then
        osascript -e 'display dialog "WezTerm download failed. Check your internet and try again, or install WezTerm from https://wezterm.org/install/macos.html" buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
        exit 1
    fi

    unzip -q "$tmp/wezterm.zip" -d "$tmp" 2>/dev/null || {
        osascript -e 'display dialog "WezTerm download was corrupted. Try again or install WezTerm from https://wezterm.org/install/macos.html" buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
        exit 1
    }

    # Zip contains WezTerm.app at top level
    if [[ ! -d "$tmp/WezTerm.app" ]]; then
        # Some zips nest one level deep
        local found
        found="$(find "$tmp" -maxdepth 3 -name "WezTerm.app" -type d | head -n1 || true)"
        if [[ -n "$found" ]]; then
            mv "$found" "$tmp/WezTerm.app" 2>/dev/null || true
        fi
    fi

    if [[ ! -d "$tmp/WezTerm.app" ]]; then
        osascript -e 'display dialog "WezTerm bundle not found in download. Install WezTerm from https://wezterm.org/install/macos.html" buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
        exit 1
    fi

    # Pick install destination: /Applications if writable, else ~/Applications, else ask for admin
    if [[ -w "/Applications" ]]; then
        dest="/Applications/WezTerm.app"
        rm -rf "$dest" 2>/dev/null || true
        ditto "$tmp/WezTerm.app" "$dest" 2>/dev/null || cp -R "$tmp/WezTerm.app" "$dest"
    elif mkdir -p "$HOME/Applications" 2>/dev/null && [[ -w "$HOME/Applications" ]]; then
        dest="$HOME/Applications/WezTerm.app"
        rm -rf "$dest" 2>/dev/null || true
        ditto "$tmp/WezTerm.app" "$dest" 2>/dev/null || cp -R "$tmp/WezTerm.app" "$dest"
    else
        dest="/Applications/WezTerm.app"
        # Escalate with admin prompt
        osascript -e "do shell script \"rm -rf \\\"$dest\\\"; ditto \\\"$tmp/WezTerm.app\\\" \\\"$dest\\\"\" with administrator privileges" >/dev/null 2>&1 || {
            osascript -e 'display dialog "Could not install WezTerm (permission denied). Try installing WezTerm from https://wezterm.org/install/macos.html" buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
            exit 1
        }
    fi

    # Clear quarantine so Gatekeeper does not block first run
    xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true

    # Brand WezTerm's dock icon with Wryme's W so all wryme windows group under our W
    # (WezTerm owns the windows, so its dock icon is what groups; patch it best-effort)
    if [[ -f "$RESC/AppIcon.icns" ]]; then
        if [[ -d "$dest/Contents/Resources" ]]; then
            # Backup original once
            [[ -f "$dest/Contents/Resources/terminal.icns" && ! -f "$dest/Contents/Resources/terminal.orig.icns" ]] && cp "$dest/Contents/Resources/terminal.icns" "$dest/Contents/Resources/terminal.orig.icns" 2>/dev/null || true
            cp "$RESC/AppIcon.icns" "$dest/Contents/Resources/terminal.icns" 2>/dev/null || true
            cp "$RESC/AppIcon.icns" "$dest/Contents/Resources/AppIcon.icns" 2>/dev/null || true
            # Point WezTerm's plist at our icon and re-sign ad-hoc so macOS picks it up
            /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile AppIcon" "$dest/Contents/Info.plist" 2>/dev/null || true
            /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" "$dest/Contents/Info.plist" 2>/dev/null || true
            codesign --force --deep --sign - "$dest" >/dev/null 2>&1 || true
            touch "$dest" 2>/dev/null || true
        fi
    fi

    WEZTERM="$dest/Contents/MacOS/wezterm"
}

if [[ -z "$WEZTERM" ]]; then
    if command -v brew >/dev/null 2>&1; then
        # Prefer brew when available (handles updates + cask quarantine)
        if ! brew install --cask wezterm 2>&1; then
            # brew failed (e.g. no cask tap) — fall back to direct download
            install_wezterm_direct
        fi
        # Re-resolve after brew
        WEZTERM="$(find_wezterm || true)"
        if [[ -z "$WEZTERM" ]]; then
            # brew claimed success but binary not in expected spot — try direct paths
            for p in "/Applications/WezTerm.app/Contents/MacOS/wezterm" "$HOME/Applications/WezTerm.app/Contents/MacOS/wezterm"; do
                [[ -x "$p" ]] && WEZTERM="$p" && break
            done
        fi
        # Last resort: direct download if still missing
        if [[ -z "$WEZTERM" ]]; then
            install_wezterm_direct
        fi
    else
        install_wezterm_direct
    fi
fi

# Final sanity check — if still not found, give a helpful exit
if [[ -z "${WEZTERM:-}" ]] || [[ ! -x "$WEZTERM" ]]; then
    WEZTERM="$(find_wezterm || true)"
fi
if [[ -z "${WEZTERM:-}" ]]; then
    osascript -e 'display dialog "WezTerm still not found after install. Install it from https://wezterm.org/install/macos.html then open wryme again." buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
    exit 1
fi

exec "$WEZTERM" --config-file "$CFG" start \
    --class wryme \
    -- "$WME"
