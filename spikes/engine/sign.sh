#!/usr/bin/env bash
# Rebuild SaysoSpike.app from source and sign it inside-out with the hardened runtime.
# Reproducible: wipes build/, uses one fixed identity, fixed identifiers, secure timestamp.
set -euo pipefail
cd "$(dirname "$0")"

IDENTITY="${IDENTITY:-Developer ID Application: Watzon Ventures LLc (MB5789APU7)}"
APP="build/SaysoSpike.app"
MACOS="$APP/Contents/MacOS"

echo "==> build sidecar (swift build -c release)"
(cd Engine && swift build -c release 2>&1 | tail -1)

echo "==> assemble bundle"
rm -rf build && mkdir -p "$MACOS"
xcrun swiftc -O -target arm64-apple-macos14.0 app/main.swift -o "$MACOS/SaysoSpike"
cp Engine/.build/release/SaysoEngine "$MACOS/SaysoEngine"
cp app/Info.plist "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

echo "==> sign inside-out (nested code first, bundle last; no --deep)"
SIGN=(codesign --force --timestamp --options runtime --sign "$IDENTITY")
"${SIGN[@]}" --identifier dev.sayso.spike.engine --entitlements app/Engine.entitlements "$MACOS/SaysoEngine"
"${SIGN[@]}" --identifier dev.sayso.spike --entitlements app/App.entitlements "$APP"

echo "==> verify"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -d --entitlements - "$APP" 2>&1 | sed 's/^/    /'
codesign -dvv "$APP" 2>&1 | grep -E "^(Identifier|Authority|TeamIdentifier|Timestamp|CodeDirectory|Signature)" | sed 's/^/    /'
echo "==> done: $APP"
