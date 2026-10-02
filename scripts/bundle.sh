#!/usr/bin/env bash
# Build, assemble, and sign build/Sayso.app.
#
#   scripts/bundle.sh            release build, Developer ID signature
#   scripts/bundle.sh --debug    debug build (faster), same signature
#
# A stable Developer ID signature keeps macOS permission grants across rebuilds
# (spike S4). Set SAYSO_SIGN_IDENTITY to use another identity, or "-" for ad hoc.
set -euo pipefail
cd "$(dirname "$0")/.."
profile=release; cargo_flag=--release
[[ "${1:-}" == "--debug" ]] && { profile=debug; cargo_flag=; }
identity="${SAYSO_SIGN_IDENTITY:-Developer ID Application: Watzon Ventures LLc (MB5789APU7)}"
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
build=$(date +%Y%m%d%H%M)

echo "==> engine (swift, release)"
(cd native/macos/SaysoEngine && swift build -c release 2>&1 | tail -1)
echo "==> app (cargo, $profile)"
cargo build -p sayso-app $cargo_flag 2>&1 | grep -E "^(error|warning: unused)" || true
cargo build -p sayso-app $cargo_flag --quiet

app=build/Sayso.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/sayso" "$app/Contents/MacOS/Sayso"
cp native/macos/SaysoEngine/.build/release/SaysoEngine "$app/Contents/MacOS/SaysoEngine"
cp assets/app/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" packaging/Info.plist > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

echo "==> sign ($identity)"
ent=packaging/Sayso.entitlements
codesign --force --timestamp --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso.engine -s "$identity" "$app/Contents/MacOS/SaysoEngine"
codesign --force --timestamp --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso -s "$identity" "$app"
codesign --verify --deep --strict "$app" && echo "==> signature ok: $app"
