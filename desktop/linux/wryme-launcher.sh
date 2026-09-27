#!/usr/bin/env bash
#
# wryme desktop launcher - Linux
#
# Opens wryme in a clean, app-like WezTerm window. No terminal chrome.
#
# The wryme binary is NEVER bundled. It always comes from cargo: this
# launcher resolves the cargo-installed `wme`, installs it from crates.io
# if missing, and keeps it current. No fallbacks.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CFG="$HERE/wezterm.lua"

# --- Resolve cargo's bin dir regardless of PATH ----------------------------
if [[ -n "${CARGO_HOME:-}" ]]; then
    CARGO_BIN="$CARGO_HOME/bin"
else
    CARGO_BIN="$HOME/.cargo/bin"
fi
WME="$CARGO_BIN/wme"
CARGO_INSTALL="cargo install wryme --locked"

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

# --- Cargo-first wryme ----------------------------------------------------
# The desktop bundle never carries `wme`. If cargo hasn't installed it yet,
# install the latest from crates.io now. No bundled fallback.
ensure_wme() {
    if [[ -x "$WME" ]]; then return 0; fi
    if ! command -v cargo >/dev/null 2>&1; then
        if command -v zenity >/dev/null 2>&1; then
            zenity --error --title="wryme" --text="wryme needs the cargo tools to install itself. Install Rust from https://rustup.rs then open wryme again."
        fi
        exit 1
    fi
    if ! $CARGO_INSTALL; then
        if command -v zenity >/dev/null 2>&1; then
            zenity --error --title="wryme" --text="Installing wryme from cargo failed. Run 'cargo install wryme' manually, then open wryme again."
        fi
        exit 1
    fi
}
ensure_wme

# --- Silent auto-update (Linux) -------------------------------------------
# Keeps the cargo-installed wme current: plain `cargo install` upgrades when
# a newer version exists and is a no-op when already current.
maybe_auto_update() {
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/wryme"
    local stamp="$cache_dir/last_update_check"
    local lock="$cache_dir/update.lock"
    mkdir -p "$cache_dir" 2>/dev/null || return 0
    if [[ -f "$stamp" ]]; then
        local age
        age=$(($(date +%s) - $(stat -c %Y "$stamp" 2>/dev/null || echo 0)))
        [[ $age -lt 86400 ]] && return 0
    fi
    if ! mkdir "$lock" 2>/dev/null; then return 0; fi
    trap 'rmdir "$lock" 2>/dev/null || true' RETURN
    if command -v cargo >/dev/null 2>&1; then
        $CARGO_INSTALL >/dev/null 2>&1 || true
    fi
    date +%s > "$stamp" 2>/dev/null || true
}
( maybe_auto_update >/dev/null 2>&1 & ) 2>/dev/null
disown 2>/dev/null || true

exec "$WEZTERM" --config-file "$CFG" start \
    --class wryme \
    -- "$WME"
