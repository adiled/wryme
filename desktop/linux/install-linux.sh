#!/usr/bin/env bash
#
# One-time install for the Linux wryme desktop bundle.
#
# Copies the bundle into ~/.local/share/wryme (so it survives cleanup of the
# downloaded folder) and registers a launcher in the desktop menu, then
# opens wryme.
#
# Lovingly borrowed from the package layout: this script shares a folder with
# `wme`, `wezterm.lua` and `wryme-launcher.sh`.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="$HOME/.local/share/wryme"
BIN_DIR="$HOME/.local/bin"
APP_ENTRY="$HOME/.local/share/applications/wryme.desktop"

mkdir -p "$INSTALL_DIR" "$BIN_DIR" "$(dirname "$APP_ENTRY")"

cp "$HERE/wme" "$HERE/wezterm.lua" "$HERE/wryme-launcher.sh" "$INSTALL_DIR/"
chmod +x "$INSTALL_DIR/wryme-launcher.sh" "$INSTALL_DIR/wme"

ln -sf "$INSTALL_DIR/wryme-launcher.sh" "$BIN_DIR/wryme"

# Menu entry with the real install path baked in.
sed "s|%INSERT_ABS_PATH%|$INSTALL_DIR|" "$HERE/wryme.desktop" > "$APP_ENTRY"
chmod +x "$APP_ENTRY"

# Refresh desktop databases when present (best-effort).
update-desktop-database "$(dirname "$APP_ENTRY")" >/dev/null 2>&1 || true

echo "wryme installed to $INSTALL_DIR"
exec "$INSTALL_DIR/wryme-launcher.sh"