#!/usr/bin/env bash
# Build, assemble, and sign build/Sayso.app.
#
#   scripts/bundle.sh            release build, Developer ID signature
#   scripts/bundle.sh --debug    debug build (faster), same signature
#
# Set SAYSO_BUNDLE_DIR to write the bundle to another folder than build/.
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
echo "==> cleanup runtime (cargo, release)"
# llama.cpp is slow in a debug build, so the runtime is always a release build.
cargo build --release --manifest-path native/cleanup/Cargo.toml --quiet
echo "==> media remote adapter (cmake)"
adapter_build=target/media-remote-adapter
cmake -S native/macos/MediaRemoteAdapter -B "$adapter_build" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$adapter_build" --target MediaRemoteAdapter -j 8 >/dev/null
echo "==> app (cargo, $profile)"
cargo build -p sayso-app $cargo_flag 2>&1 | grep -E "^(error|warning: unused)" || true
cargo build -p sayso-app $cargo_flag --quiet

app="${SAYSO_BUNDLE_DIR:-build}/Sayso.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp "target/$profile/sayso" "$app/Contents/MacOS/Sayso"
cp native/macos/SaysoEngine/.build/release/SaysoEngine "$app/Contents/MacOS/SaysoEngine"
# SwiftPM leaves the symbol table in the release binary: 6 MB.
strip "$app/Contents/MacOS/SaysoEngine"
# The app finds the cleanup runtime next to its executable
# (crates/sayso-cleanup-client/src/runtime.rs).
cp native/cleanup/target/release/sayso-cleanup "$app/Contents/MacOS/sayso-cleanup"
strip "$app/Contents/MacOS/sayso-cleanup"
cp assets/app/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
# Pause media while recording: /usr/bin/perl runs the script, and the script
# loads the framework (crates/sayso-platform-macos/src/media.rs).
cp -R "$adapter_build/MediaRemoteAdapter.framework" "$app/Contents/Frameworks/"
cp native/macos/MediaRemoteAdapter/bin/mediaremote-adapter.pl "$app/Contents/Resources/"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" packaging/Info.plist > "$app/Contents/Info.plist"
# The microphone prompt in each reviewed interface language
# (scripts/i18n-system-files.py writes these folders).
for lproj in packaging/macos/*.lproj; do
  [ -d "$lproj" ] && cp -R "$lproj" "$app/Contents/Resources/"
done
printf 'APPL????' > "$app/Contents/PkgInfo"

echo "==> sign ($identity)"
ent=packaging/Sayso.entitlements
codesign --force --timestamp --options runtime -s "$identity" "$app/Contents/Frameworks/MediaRemoteAdapter.framework"
codesign --force --timestamp --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso.engine -s "$identity" "$app/Contents/MacOS/SaysoEngine"
# The cleanup runtime records no audio, so it gets no entitlements.
codesign --force --timestamp --options runtime --identifier dev.sayso.Sayso.cleanup -s "$identity" "$app/Contents/MacOS/sayso-cleanup"
codesign --force --timestamp --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso -s "$identity" "$app"
codesign --verify --deep --strict "$app" && echo "==> signature ok: $app"
