#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?usage: package-linux.sh <version> [arch]}"
RAW_ARCH="${2:-$(uname -m)}"
case "$RAW_ARCH" in
    x86_64*|*amd64*) DEB_ARCH="amd64"; TARGZ_ARCH="x86_64" ;;
    aarch64*|arm64*) DEB_ARCH="arm64"; TARGZ_ARCH="aarch64" ;;
    *) DEB_ARCH="$RAW_ARCH"; TARGZ_ARCH="$RAW_ARCH" ;;
esac
if [[ "$RAW_ARCH" == *"-"* ]]; then
    case "$RAW_ARCH" in
        x86_64*) DEB_ARCH="amd64"; TARGZ_ARCH="x86_64" ;;
        aarch64*) DEB_ARCH="arm64"; TARGZ_ARCH="aarch64" ;;
    esac
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STAGE="dist/wryme-linux"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp desktop/config/wezterm.lua "$STAGE/wezterm.lua"
cp desktop/linux/wryme-launcher.sh "$STAGE/wryme-launcher.sh"
cp desktop/linux/install-linux.sh "$STAGE/install.sh"
cp desktop/linux/wryme.desktop "$STAGE/wryme.desktop"
cp desktop/linux/wryme.png "$STAGE/wryme.png"
chmod +x "$STAGE/wryme-launcher.sh" "$STAGE/install.sh"
tar czf "dist/wryme-linux-$TARGZ_ARCH.tar.gz" -C dist wryme-linux 2>/dev/null || true

DEB_STAGE="dist/wryme-deb"
DEB_NAME="wryme_${VERSION}_${DEB_ARCH}.deb"
rm -rf "$DEB_STAGE"
mkdir -p "$DEB_STAGE/DEBIAN" "$DEB_STAGE/usr/share/wryme" "$DEB_STAGE/usr/share/applications" "$DEB_STAGE/usr/share/icons/hicolor/256x256/apps"

cp desktop/config/wezterm.lua "$DEB_STAGE/usr/share/wryme/wezterm.lua"
cp desktop/linux/wryme-launcher.sh "$DEB_STAGE/usr/share/wryme/wryme-launcher.sh"
cp desktop/linux/wryme.png "$DEB_STAGE/usr/share/wryme/wryme.png"
cp desktop/linux/wryme.png "$DEB_STAGE/usr/share/icons/hicolor/256x256/apps/wryme.png"
chmod 755 "$DEB_STAGE/usr/share/wryme/wryme-launcher.sh"

cat > "$DEB_STAGE/usr/share/applications/wryme.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=wryme
GenericName=AI Chat
Comment=Small calm window to chat with an AI
Exec=/usr/share/wryme/wryme-launcher.sh
Icon=wryme
Terminal=false
Categories=Network;Utility;Chat;
StartupNotify=true
StartupWMClass=wryme
X-GNOME-UsesNotifications=false
EOF

cat > "$DEB_STAGE/DEBIAN/control" <<EOF
Package: wryme
Version: $VERSION
Section: utils
Priority: optional
Architecture: $DEB_ARCH
Maintainer: wryme <hi@wryme.sh>
Description: wryme — small calm window to chat with an AI
 Wryme desktop app (WezTerm-wrapped wme TUI).
Depends: ca-certificates
EOF

chmod 755 "$DEB_STAGE/DEBIAN"
if command -v dpkg-deb >/dev/null 2>&1; then
    dpkg-deb --build "$DEB_STAGE" "dist/$DEB_NAME" >/dev/null
else
    echo "warn: dpkg-deb not found, keeping tar.gz only" >&2
fi

cp "dist/$DEB_NAME" "./$DEB_NAME" 2>/dev/null || true
cp "dist/wryme-linux-$TARGZ_ARCH.tar.gz" "./wryme-linux-$TARGZ_ARCH.tar.gz" 2>/dev/null || true

echo "built dist/$DEB_NAME"
CI_ARTIFACT_DEB="wryme-linux-x86_64.deb"
if [[ "$DEB_ARCH" == "amd64" ]]; then
    cp "dist/$DEB_NAME" "dist/$CI_ARTIFACT_DEB" 2>/dev/null || true
    cp "dist/$DEB_NAME" "./$CI_ARTIFACT_DEB" 2>/dev/null || true
fi
