#!/usr/bin/env bash
#
# Package the wryme desktop bundle for Linux.
#
# Produces: wryme-linux-<arch>.tar.gz  (a folder with wme, launcher, config)
#
# Usage: package-linux.sh <version> [arch]
#   arch defaults to `uname -m` (x86_64 or aarch64)

set -euo pipefail

VERSION="${1:?usage: package-linux.sh <version> [arch]}"
ARCH="${2:-$(uname -m)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STAGE="dist/wryme-linux"

# Locate binary: prefer $BIN from CI, then fallbacks
BIN="${BIN:-}"
if [[ -z "$BIN" ]]; then
    for cand in "target/${TARGET:-}/release/wme" "target/release/wme" target/*/release/wme target/x86_64-unknown-linux-gnu/release/wme; do
        if [[ -x "$cand" ]]; then BIN="$cand"; break; fi
    done
fi
[[ -x "${BIN:-}" ]] || { echo "missing built binary: $BIN (run cargo build --release first)" >&2; exit 1; }

rm -rf "$STAGE"
mkdir -p "$STAGE"

cp "$BIN" "$STAGE/wme"
cp desktop/config/wezterm.lua "$STAGE/wezterm.lua"
cp desktop/linux/wryme-launcher.sh "$STAGE/wryme-launcher.sh"
cp desktop/linux/install-linux.sh "$STAGE/install.sh"
cp desktop/linux/wryme.desktop "$STAGE/wryme.desktop"
cp desktop/linux/wryme.png "$STAGE/wryme.png"
chmod +x "$STAGE/wryme-launcher.sh" "$STAGE/install.sh"

tar czf "wryme-linux-$ARCH.tar.gz" -C dist wryme-linux
echo "built wryme-linux-$ARCH.tar.gz"