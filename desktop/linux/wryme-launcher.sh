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

# --- Silent auto-update (Linux) -------------------------------------------
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
    local cur="0.0.0"
    if [[ -x "$WME" ]]; then cur="$("$WME" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo "0.0.0")"; fi
    local json
    json="$(curl -fsSL --max-time 8 -H 'Accept: application/vnd.github+json' https://api.github.com/repos/adiled/wryme/releases/latest 2>/dev/null || true)"
    [[ -z "$json" ]] && { rmdir "$lock" 2>/dev/null || true; return 0; }
    local tag
    tag="$(python3 -c "import json,sys; d=json.loads(sys.stdin.read()); print(d.get('tag_name','').lstrip('v'))" <<< "$json" 2>/dev/null || echo "")"
    [[ -z "$tag" ]] && { rmdir "$lock" 2>/dev/null || true; return 0; }
    local need
    need="$(python3 -c "
import sys
def parse(v):
    try: return tuple(int(x) for x in v.split('.'))
    except: return (0,0,0)
print('1' if parse('$tag')>parse('$cur') else '0')
" 2>/dev/null || echo 0)"
    [[ "$need" != "1" ]] && { date +%s > "$stamp" 2>/dev/null || true; rmdir "$lock" 2>/dev/null || true; return 0; }
    local arch url tmp new_wme
    arch="$(uname -m)"
    local asset="wryme-linux-${arch}.tar.gz"
    # we only publish x86_64 for linux presently; fall back
    if [[ "$arch" != "x86_64" ]]; then asset="wryme-linux-x86_64.tar.gz"; fi
    url="$(python3 -c "import json,sys; d=json.loads(sys.stdin.read()); [print(a['browser_download_url']) for a in d.get('assets',[]) if a.get('name')=='$asset']" <<< "$json" 2>/dev/null | head -1 || true)"
    [[ -z "$url" ]] && url="$(python3 -c "import json,sys; d=json.loads(sys.stdin.read()); [print(a['browser_download_url']) for a in d.get('assets',[]) if 'linux' in a.get('name','')]" <<< "$json" 2>/dev/null | head -1 || true)"
    [[ -z "$url" ]] && { date +%s > "$stamp" 2>/dev/null || true; rmdir "$lock" 2>/dev/null || true; return 0; }
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"; rmdir "'$lock'" 2>/dev/null || true' RETURN
    if ! curl -fsSL --max-time 90 "$url" -o "$tmp/bundle.tar.gz" 2>/dev/null; then date +%s > "$stamp" 2>/dev/null || true; return 0; fi
    if ! tar -xzf "$tmp/bundle.tar.gz" -C "$tmp" 2>/dev/null; then date +%s > "$stamp" 2>/dev/null || true; return 0; fi
    new_wme="$(find "$tmp" -type f -name "wme" | head -1 || true)"
    [[ -z "$new_wme" ]] && { date +%s > "$stamp" 2>/dev/null || true; return 0; }
    chmod +x "$new_wme" 2>/dev/null || true
    cp "$new_wme" "$WME.new" 2>/dev/null && chmod +x "$WME.new" 2>/dev/null && mv "$WME.new" "$WME" 2>/dev/null || true
    # also update installed copy if this is the dist copy (install.sh will copy on next install)
    if [[ "$HERE" != "$HOME/.local/share/wryme" && -x "$HOME/.local/share/wryme/wme" ]]; then
        cp "$WME" "$HOME/.local/share/wryme/wme" 2>/dev/null || true
    fi
    date +%s > "$stamp" 2>/dev/null || true
}
( maybe_auto_update >/dev/null 2>&1 & ) 2>/dev/null
disown 2>/dev/null || true

exec "$WEZTERM" --config-file "$CFG" start \
    --class wryme \
    -- "$WME"