#!/bin/sh
set -eu

HERE="$HOME/.config/wryme"
CLOUD="$HOME/Library/Mobile Documents/com~apple~CloudDocs/wryme"

mkdir -p "$CLOUD"
for name in shops.toml stations.toml prefs.json; do
    if [ -L "$HERE/$name" ]; then
        echo "$name already shared"
        continue
    fi
    if [ -e "$CLOUD/$name" ]; then
        echo "$CLOUD/$name exists, leaving $name alone" >&2
        continue
    fi
    if [ -e "$HERE/$name" ]; then
        mv "$HERE/$name" "$CLOUD/$name"
    fi
    ln -s "$CLOUD/$name" "$HERE/$name"
    echo "$name shared"
done
echo "iPhone: choose iCloud Drive > wryme"
