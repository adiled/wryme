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
    # 1) Bundled inside wme.app (preferred, no download, no hack)
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

# --- Use a private branded WezTerm copy for wryme (so wme has its own Dock icon) ----
# We leave the user's main WezTerm.app untouched. We copy it to
# ~/Library/Application Support/wryme/WezTerm-Wryme.app, brand its icon with
# our W, give it a distinct bundle ID, and point WEZTERM there. This makes
# wme windows group under the W icon, while WezTerm keeps its own icon.
maybe_brand_wezterm() {
    [[ -f "$RESC/AppIcon.icns" ]] || return 0
    local src_app
    # Resolve src .app from WEZTERM path (e.g. /Applications/WezTerm.app/Contents/MacOS/wezterm)
    src_app="$(cd "$(dirname "$WEZTERM")/.." && pwd)"
    [[ -d "$src_app" ]] || return 0
    # Only brand if src is a real .app (not a bare binary)
    [[ "$src_app" == *.app ]] || return 0
    local support="$HOME/Library/Application Support/wryme"
    local branded="$support/WezTerm-Wryme.app"
    mkdir -p "$support" 2>/dev/null || return 0
    # Rebuild branded copy if missing or src newer, or our icon newer
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
    # Use branded binary if present
    if [[ -x "$branded/Contents/MacOS/wezterm" ]]; then
        WEZTERM="$branded/Contents/MacOS/wezterm"
    fi
}
maybe_brand_wezterm 2>/dev/null || true

# --- Silent auto-update ---------------------------------------------
# Checks GitHub for a newer wryme release once per 24h, in background.
# Fully silent: no prompts, no notifications. Replaces the bundled wme
# inside this .app and re-signs, so next launch is new.
maybe_auto_update() {
    local cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/wryme"
    local stamp="$cache_dir/last_update_check"
    local lock="$cache_dir/update.lock"

    mkdir -p "$cache_dir" 2>/dev/null || return 0
    # Throttle: 24h
    if [[ -f "$stamp" ]]; then
        local age
        if age=$(($(date +%s) - $(stat -f %m "$stamp" 2>/dev/null || stat -c %Y "$stamp" 2>/dev/null || echo 0) )) 2>/dev/null; then
            [[ $age -lt 86400 ]] && return 0
        fi
    fi
    # Lock (best-effort, no block)
    if ! mkdir "$lock" 2>/dev/null; then return 0; fi
    trap 'rmdir "$lock" 2>/dev/null || true' RETURN

    # Local version from wme or bundle plist
    local cur="0.0.0"
    if [[ -x "$WME" ]]; then
        cur="$("$WME" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || echo "0.0.0")"
    fi
    if [[ "$cur" == "0.0.0" && -f "$RESC/../Info.plist" ]]; then
        cur="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$RESC/../Info.plist" 2>/dev/null || echo "0.0.0")"
    fi
    # Also check $SCRIPT_DIR/../Info.plist (when RESC is Resources)
    if [[ "$cur" == "0.0.0" ]]; then
        cur="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$SCRIPT_DIR/../Info.plist" 2>/dev/null || echo "0.0.0")"
    fi

    # Fetch latest release atomically with timeout
    local api="https://api.github.com/repos/adiled/wryme/releases/latest"
    local json
    json="$(curl -fsSL --max-time 8 -H 'Accept: application/vnd.github+json' "$api" 2>/dev/null || true)"
    [[ -z "$json" ]] && { rmdir "$lock" 2>/dev/null || true; return 0; }

    local latest tag url arch asset
    latest="$(python3 -c "
import json,sys
try:
    d=json.loads(sys.stdin.read())
    print(d.get('tag_name',''))
except: pass
" <<< "$json" 2>/dev/null || true)"
    tag="${latest#v}"
    [[ -z "$tag" || "$tag" == "null" ]] && { rmdir "$lock" 2>/dev/null || true; return 0; }

    # Semver compare: is tag > cur?
    local need_update
    need_update="$(python3 -c "
import sys
def parse(v): 
    try: return tuple(int(x) for x in v.split('.'))
    except: return (0,0,0)
cur=tuple(parse('$cur'))
tag=tuple(parse('$tag'))
print('1' if tag>cur else '0')
" 2>/dev/null || echo 0)"
    [[ "$need_update" != "1" ]] && { date +%s > "$stamp" 2>/dev/null || true; rmdir "$lock" 2>/dev/null || true; return 0; }

    # Pick asset for this arch. We publish wryme-darwin-arm64.zip and wryme-darwin-x86_64.zip
    # (and aarch64 alias). Try arm64 first on Apple Silicon.
    arch="$(uname -m)"
    if [[ "$arch" == "arm64" ]]; then asset="wryme-darwin-arm64.zip"; else asset="wryme-darwin-x86_64.zip"; fi
    # Support old naming wryme-darwin-aarch64.zip as fallback
    url="$(python3 -c "
import json,sys
d=json.loads(sys.stdin.read())
for a in d.get('assets',[]):
    if a.get('name')=='$asset':
        print(a.get('browser_download_url',''))
        sys.exit(0)
# fallback alias
alias='wryme-darwin-aarch64.zip' if '$asset'=='wryme-darwin-arm64.zip' else ''
if alias:
    for a in d.get('assets',[]):
        if a.get('name')==alias:
            print(a.get('browser_download_url',''))
            sys.exit(0)
" <<< "$json" 2>/dev/null || true)"
    [[ -z "$url" ]] && { date +%s > "$stamp" 2>/dev/null || true; rmdir "$lock" 2>/dev/null || true; return 0; }

    local tmp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"; rmdir "'$lock'" 2>/dev/null || true' RETURN

    if ! curl -fsSL --max-time 90 "$url" -o "$tmp/bundle.zip" 2>/dev/null; then
        date +%s > "$stamp" 2>/dev/null || true
        return 0
    fi
    if ! unzip -q "$tmp/bundle.zip" -d "$tmp" 2>/dev/null; then
        date +%s > "$stamp" 2>/dev/null || true
        return 0
    fi
    local new_wme
    new_wme="$(find "$tmp" -type f -name "wme" | head -1 || true)"
    [[ -z "$new_wme" || ! -f "$new_wme" ]] && { date +%s > "$stamp" 2>/dev/null || true; return 0; }
    chmod +x "$new_wme" 2>/dev/null || true
    # Atomic replace
    cp "$new_wme" "$WME.new" 2>/dev/null && chmod +x "$WME.new" 2>/dev/null && mv "$WME.new" "$WME" 2>/dev/null || true
    xattr -c "$WME" 2>/dev/null || true
    # Re-sign the outer .app if we are inside one (so Gatekeeper stays happy)
    local app_root
    app_root="$(cd "$SCRIPT_DIR/../.." && pwd)"
    if [[ "$app_root" == *.app ]]; then
        codesign --force --deep --sign - "$app_root" >/dev/null 2>&1 || true
    elif [[ -d "$SCRIPT_DIR/../../Wryme.app" ]]; then
        codesign --force --deep --sign - "$SCRIPT_DIR/../../Wryme.app" >/dev/null 2>&1 || true
    fi
    date +%s > "$stamp" 2>/dev/null || true
}

# Kick off background check (detached, fully silent). Do not block launch.
( maybe_auto_update >/dev/null 2>&1 & ) 2>/dev/null
disown 2>/dev/null || true

exec "$WEZTERM" --config-file "$CFG" start \
    --class wryme \
    -- "$WME"
