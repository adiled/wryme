#!/usr/bin/env bash

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="$HOME/.local/share/wryme"
BIN_DIR="$HOME/.local/bin"
APP_ENTRY="$HOME/.local/share/applications/wryme.desktop"

mkdir -p "$INSTALL_DIR" "$BIN_DIR" "$(dirname "$APP_ENTRY")"

cp "$HERE/wezterm.lua" "$HERE/wryme-launcher.sh" "$HERE/wryme.png" "$INSTALL_DIR/"
chmod +x "$INSTALL_DIR/wryme-launcher.sh"

ln -sf "$INSTALL_DIR/wryme-launcher.sh" "$BIN_DIR/wryme"

sed "s|%INSERT_ABS_PATH%|$INSTALL_DIR|" "$HERE/wryme.desktop" > "$APP_ENTRY"
chmod +x "$APP_ENTRY"

update-desktop-database "$(dirname "$APP_ENTRY")" >/dev/null 2>&1 || true

echo "wryme installed to $INSTALL_DIR"
exec "$INSTALL_DIR/wryme-launcher.sh"