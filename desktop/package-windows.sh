#!/usr/bin/env bash
#
# Package the wryme desktop bundle for Windows.
#
# Produces: wryme-windows-x86_64.zip  (launchers, config, icons — no binary;
#           wme.exe always comes from cargo install)
#
# Usage: package-windows.sh <version>

set -euo pipefail

VERSION="${1:?usage: package-windows.sh <version>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STAGE="dist/wryme-windows"

rm -rf "$STAGE"
mkdir -p "$STAGE"

cp desktop/config/wezterm.lua "$STAGE/wezterm.lua"
cp desktop/windows/wryme-run.cmd "$STAGE/wryme-run.cmd"
cp desktop/windows/wryme-launcher.vbs "$STAGE/wryme-launcher.vbs"
cp desktop/windows/wryme.png "$STAGE/wryme.png"
cp desktop/windows/wryme.ico "$STAGE/wryme.ico"

powershell -NoProfile -Command \
  "Compress-Archive -Path '$STAGE/*' -DestinationPath 'wryme-windows-x86_64.zip' -Force"
echo "built wryme-windows-x86_64.zip"
