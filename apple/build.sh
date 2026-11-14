#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

PROFILE="${1:-release}"
export CARGO_TARGET_DIR=target/apple
export MACOSX_DEPLOYMENT_TARGET=14.0
if [ "$PROFILE" = release ]; then
    cargo build -p wryme-apple --release
else
    cargo build -p wryme-apple
fi

APP="$CARGO_TARGET_DIR/wryme.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp apple/Info.plist "$APP/Contents/Info.plist"
cp desktop/macos/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

swiftc -O -swift-version 5 -parse-as-library \
    -target arm64-apple-macos14.0 \
    -import-objc-header apple/wryme.h \
    apple/app/*.swift \
    "$CARGO_TARGET_DIR/$PROFILE/libwryme_apple.a" \
    -framework Security -framework SystemConfiguration -framework CoreFoundation \
    -o "$APP/Contents/MacOS/wryme"

codesign --force --sign - "$APP"
echo "$APP"
