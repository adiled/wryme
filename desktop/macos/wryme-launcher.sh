#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESC="$SCRIPT_DIR/../Resources"
CFG="$RESC/wezterm.lua"

if [[ -n "${CARGO_HOME:-}" ]]; then
    CARGO_BIN="$CARGO_HOME/bin"
else
    CARGO_BIN="$HOME/.cargo/bin"
fi
WME="$CARGO_BIN/wme"

find_wezterm() {
    for p in \
        "$RESC/WezTerm.app/Contents/MacOS/wezterm" \
        "$SCRIPT_DIR/../Resources/WezTerm.app/Contents/MacOS/wezterm" \
        "$SCRIPT_DIR/../../Resources/WezTerm.app/Contents/MacOS/wezterm"; do
        if [[ -x "$p" ]]; then echo "$p"; return 0; fi
    done
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

    if [[ ! -d "$tmp/WezTerm.app" ]]; then
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
        osascript -e "do shell script \"rm -rf \\\"$dest\\\"; ditto \\\"$tmp/WezTerm.app\\\" \\\"$dest\\\"\" with administrator privileges" >/dev/null 2>&1 || {
            osascript -e 'display dialog "Could not install WezTerm (permission denied). Try installing WezTerm from https://wezterm.org/install/macos.html" buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
            exit 1
        }
    fi

    xattr -dr com.apple.quarantine "$dest" 2>/dev/null || true

    WEZTERM="$dest/Contents/MacOS/wezterm"
}

if [[ -z "$WEZTERM" ]]; then
    if command -v brew >/dev/null 2>&1; then
        if ! brew install --cask wezterm 2>&1; then
            install_wezterm_direct
        fi
        WEZTERM="$(find_wezterm || true)"
        if [[ -z "$WEZTERM" ]]; then
            for p in "/Applications/WezTerm.app/Contents/MacOS/wezterm" "$HOME/Applications/WezTerm.app/Contents/MacOS/wezterm"; do
                [[ -x "$p" ]] && WEZTERM="$p" && break
            done
        fi
        if [[ -z "$WEZTERM" ]]; then
            install_wezterm_direct
        fi
    else
        install_wezterm_direct
    fi
fi

if [[ -z "${WEZTERM:-}" ]] || [[ ! -x "$WEZTERM" ]]; then
    WEZTERM="$(find_wezterm || true)"
fi
if [[ -z "${WEZTERM:-}" ]]; then
    osascript -e 'display dialog "WezTerm still not found after install. Install it from https://wezterm.org/install/macos.html then open wryme again." buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
    exit 1
fi

maybe_brand_wezterm() {
    [[ -f "$RESC/AppIcon.icns" ]] || return 0
    local src_app
    src_app="$(cd "$(dirname "$WEZTERM")/.." && pwd)"
    [[ -d "$src_app" ]] || return 0
    [[ "$src_app" == *.app ]] || return 0
    local support="$HOME/Library/Application Support/wryme"
    local branded="$support/WezTerm-Wryme.app"
    mkdir -p "$support" 2>/dev/null || return 0
    local need=0
    if [[ ! -d "$branded" ]]; then need=1
    elif [[ "$src_app/Contents/Info.plist" -nt "$branded/Contents/Info.plist" ]]; then need=1
    elif [[ "$RESC/AppIcon.icns" -nt "$branded/Contents/Resources/terminal.icns" ]]; then need=1
    fi
    if [[ $need -eq 1 ]]; then
        rm -rf "$branded" 2>/dev/null || true
        ditto "$src_app" "$branded" 2>/dev/null || cp -R "$src_app" "$branded" 2>/dev/null || return 0
        cp "$RESC/AppIcon.icns" "$branded/Contents/Resources/terminal.icns" 2>/dev/null || true
        cp "$RESC/AppIcon.icns" "$branded/Contents/Resources/AppIcon.icns" 2>/dev/null || true
        /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier sh.wryme.wezterm" "$branded/Contents/Info.plist" 2>/dev/null || /usr/libexec/PlistBuddy -c "Add :CFBundleIdentifier string sh.wryme.wezterm" "$branded/Contents/Info.plist" 2>/dev/null || true
        /usr/libexec/PlistBuddy -c "Set :CFBundleName wme" "$branded/Contents/Info.plist" 2>/dev/null || true
        /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName wme" "$branded/Contents/Info.plist" 2>/dev/null || true
        /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile AppIcon" "$branded/Contents/Info.plist" 2>/dev/null || /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon" "$branded/Contents/Info.plist" 2>/dev/null || true
        /usr/libexec/PlistBuddy -c "Set :CFBundleIconName AppIcon" "$branded/Contents/Info.plist" 2>/dev/null || /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" "$branded/Contents/Info.plist" 2>/dev/null || true
        xattr -dr com.apple.quarantine "$branded" 2>/dev/null || true
        codesign --force --deep --sign - "$branded" >/dev/null 2>&1 || true
        touch "$branded" 2>/dev/null || true
    fi
    if [[ -x "$branded/Contents/MacOS/wezterm" ]]; then
        WEZTERM="$branded/Contents/MacOS/wezterm"
    fi
}
maybe_brand_wezterm 2>/dev/null || true

ensure_wme() {
    if [[ -x "$WME" ]]; then return 0; fi
    if ! command -v cargo >/dev/null 2>&1; then
        osascript -e 'display dialog "wryme needs the cargo tools to install itself. Install Rust from https://rustup.rs then open wryme again." buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
        exit 1
    fi
    osascript -e 'display notification "Installing wryme from crates.io…" with title "wryme"' >/dev/null 2>&1 || true
    if ! cargo install wryme --locked; then
        osascript -e 'display dialog "Installing wryme from cargo failed. Run `cargo install wryme` manually, then open wryme again." buttons {"OK"} default button "OK" with icon stop' >/dev/null 2>&1 || true
        exit 1
    fi
}
ensure_wme

maybe_auto_update() {
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/wryme"
    local one_day=86400
    local stamp="$cache_dir/last_update_check"
    local lock="$cache_dir/update.lock"

    mkdir -p "$cache_dir" 2>/dev/null || return 0
    if [[ -f "$stamp" ]]; then
        local age
        if age=$(($(date +%s) - $(stat -f %m "$stamp" 2>/dev/null || stat -c %Y "$stamp" 2>/dev/null || echo 0) )) 2>/dev/null; then
            [[ $age -lt $one_day ]] && return 0
        fi
    fi
    if ! mkdir "$lock" 2>/dev/null; then return 0; fi
    trap 'rmdir "$lock" 2>/dev/null || true' RETURN

    if command -v cargo >/dev/null 2>&1; then
        cargo install wryme --locked >/dev/null 2>&1 || true
    fi
    date +%s > "$stamp" 2>/dev/null || true
}

( maybe_auto_update >/dev/null 2>&1 & ) 2>/dev/null
disown 2>/dev/null || true

exec "$WEZTERM" --config-file "$CFG" start \
    --class wryme \
    -- "$WME"
