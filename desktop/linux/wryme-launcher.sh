#!/usr/bin/env bash
#
# wryme desktop launcher - Linux
#
# Opens wryme in a clean, app-like WezTerm window. No terminal chrome.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WME="$HERE/wme"
CFG="$HERE/wezterm.lua"

# --- Locate wezterm ---------------------------------------------------------
find_wezterm() {
    local bin
    if bin="$(command -v wezterm 2>/dev/null)"; then
        echo "$bin"; return 0
    fi
    for p in /usr/bin/wezterm /usr/local/bin/wezterm "$HOME/.local/bin/wezterm"; do
        if [[ -x "$p" ]]; then echo "$p"; return 0; fi
    done
    return 1
}

WEZTERM="$(find_wezterm || true)"

if [[ -z "$WEZTERM" ]]; then
    if command -v zenity >/dev/null 2>&1; then
        zenity --question --title="wryme" \
            --text="wryme needs WezTerm (a small native terminal) to open as its window. Install it now?" \
            --ok-label="Install" --cancel-label="Cancel" || exit 0
    elif command -v kdialog >/dev/null 2>&1; then
        kdialog --yesno "wryme needs WezTerm (a small native terminal) to open as its window. Install it now?" || exit 0
    else
        exit 0
    fi

    if command -v apt-get >/dev/null 2>&1; then
        sudo apt-get install -y wezterm
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y wezterm
    elif command -v flatpak >/dev/null 2>&1; then
        flatpak install -y org.wezfurlong.wezterm
    elif command -v snap >/dev/null 2>&1; then
        sudo snap install wezterm
    else
        if command -v zenity >/dev/null 2>&1; then
            zenity --error --title="wryme" --text="Could not install WezTerm automatically. Install it from https://wezterm.org/download.html then open wryme again."
        fi
        exit 1
    fi
    WEZTERM="$(find_wezterm || echo wezterm)"
fi

if [[ ! -x "${WEZTERM}" && "$WEZTERM" != "wezterm" ]]; then
    WEZTERM="wezterm"
fi

exec "$WEZTERM" --config-file "$CFG" start \
    --always-new-process \
    --class wryme \
    -- "$WME"