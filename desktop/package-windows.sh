#!/usr/bin/env bash
#
# Package the wryme desktop bundle for Windows.
#
# Produces: wryme-windows-x86_64.zip  (a folder with wme.exe, launchers, config)
#
# Usage: package-windows.sh <version>

set -euo pipefail

VERSION="${1:?usage: package-windows.sh <version>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STAGE="dist/wryme-windows"

BIN="${BIN:-}"
if [[ -z "$BIN" ]]; then
    for cand in "target/${TARGET:-}/release/wme.exe" "target/release/wme.exe" target/*/release/wme.exe target/x86_64-pc-windows-msvc/release/wme.exe; do
        if [[ -f "$cand" ]]; then BIN="$cand"; break; fi
    done
fi
[[ -f "${BIN:-}" ]] || { echo "missing built binary: $BIN (run cargo build --release first)" >&2; exit 1; }

rm -rf "$STAGE"
mkdir -p "$STAGE"

cp "$BIN" "$STAGE/wme.exe"
cp desktop/config/wezterm.lua "$STAGE/wezterm.lua"
cp desktop/windows/wryme-run.cmd "$STAGE/wryme-run.cmd"
cp desktop/windows/wryme-launcher.vbs "$STAGE/wryme-launcher.vbs"
cp desktop/windows/wryme.png "$STAGE/wryme.png"
cp desktop/windows/wryme.ico "$STAGE/wryme.ico"

powershell -NoProfile -Command \
  "Compress-Archive -Path '$STAGE/*' -DestinationPath 'wryme-windows-x86_64.zip' -Force"
echo "built wryme-windows-x86_64.zip"