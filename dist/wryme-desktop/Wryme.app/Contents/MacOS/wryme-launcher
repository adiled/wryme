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

if [[ -z "$WEZTERM" ]]; then
    if command -v brew >/dev/null 2>&1; then
        osascript -e 'display dialog "One more thing: wryme needs WezTerm (a small native terminal) as its window. Install it now?" buttons {"Cancel", "Install"} default button "Install" with icon note' \
            >/dev/null 2>&1 || exit 0
        brew install --cask wezterm
        MEANINGFUL_DELAY_MSG=1
        WEZTERM="/Applications/WezTerm.app/Contents/MacOS/wezterm"
    else
        osascript -e 'display dialog "wryme needs WezTerm to open as an app window. Install WezTerm first, then open wryme again." buttons {"OK"} default button "OK" with icon stop' \
            >/dev/null 2>&1 || true
        exit 1
    fi
fi

exec "$WEZTERM" start \
    --always-new-process \
    --class wryme \
    --config-file "$CFG" \
    -- "$WME"
