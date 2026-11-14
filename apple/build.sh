#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

TARGET="${1:-mac}"
PROFILE="${2:-release}"
export CARGO_TARGET_DIR=target/apple
FLAG=""
[ "$PROFILE" = release ] && FLAG="--release"

case "$TARGET" in
mac)
    export MACOSX_DEPLOYMENT_TARGET=15.0
    TRIPLE=aarch64-apple-darwin SDK=macosx SWIFT_TARGET=arm64-apple-macos15.0
    APP="$CARGO_TARGET_DIR/wryme.app" BIN="$CARGO_TARGET_DIR/wryme.app/Contents/MacOS"
    RES="$CARGO_TARGET_DIR/wryme.app/Contents/Resources" PLIST="$CARGO_TARGET_DIR/wryme.app/Contents/Info.plist"
    ;;
ios-sim)
    export IPHONEOS_DEPLOYMENT_TARGET=18.0
    TRIPLE=aarch64-apple-ios-sim SDK=iphonesimulator SWIFT_TARGET=arm64-apple-ios18.0-simulator
    APP="$CARGO_TARGET_DIR/wryme-sim.app" BIN="$APP" RES="$APP" PLIST="$APP/Info.plist"
    ;;
*)
    echo "usage: apple/build.sh [mac|ios-sim] [release|debug]" >&2
    exit 2
    ;;
esac

cargo build -p wryme-apple --target "$TRIPLE" $FLAG

rm -rf "$APP"
mkdir -p "$BIN" "$RES"

if [ "$TARGET" = mac ]; then
    cp apple/mac.plist "$PLIST"
    cp desktop/macos/AppIcon.icns "$RES/AppIcon.icns"
else
    cp apple/ios.plist "$PLIST"
    ASSETS="$CARGO_TARGET_DIR/Assets.xcassets"
    rm -rf "$ASSETS"
    mkdir -p "$ASSETS/AppIcon.appiconset"
    cp desktop/macos/wryme-1024.png "$ASSETS/AppIcon.appiconset/icon.png"
    printf '{"info":{"author":"xcode","version":1}}' > "$ASSETS/Contents.json"
    printf '{"images":[{"filename":"icon.png","idiom":"universal","platform":"ios","size":"1024x1024"}],"info":{"author":"xcode","version":1}}' \
        > "$ASSETS/AppIcon.appiconset/Contents.json"
    xcrun actool "$ASSETS" --compile "$APP" --platform "$SDK" --minimum-deployment-target 18.0 \
        --app-icon AppIcon --target-device iphone --target-device ipad \
        --output-partial-info-plist "$CARGO_TARGET_DIR/icon.plist" > /dev/null
    /usr/libexec/PlistBuddy -c "Merge $CARGO_TARGET_DIR/icon.plist" "$PLIST" > /dev/null
fi

xcrun --sdk "$SDK" swiftc -O -swift-version 5 -parse-as-library \
    -target "$SWIFT_TARGET" \
    -import-objc-header apple/wryme.h \
    apple/app/*.swift \
    "$CARGO_TARGET_DIR/$TRIPLE/$PROFILE/libwryme_apple.a" \
    -framework Security -framework SystemConfiguration -framework CoreFoundation \
    -o "$BIN/wryme"

codesign --force --sign - "$APP"
echo "$APP"
