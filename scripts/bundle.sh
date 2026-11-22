#!/usr/bin/env bash
# Build, assemble, and sign build/Sayso.app.
#
#   scripts/bundle.sh            release build, Developer ID signature
#   scripts/bundle.sh --debug    debug build (faster), same signature
#
# Set SAYSO_BUNDLE_DIR to write the bundle to another folder than build/.
# A stable Developer ID signature keeps macOS permission grants across rebuilds
# (spike S4). Set SAYSO_SIGN_IDENTITY to use another identity, or "-" for ad hoc.
#
# --debug on a bundle whose other parts did not change (build/bundle.stamp)
# builds only the Go app, puts it in the bundle, and signs the bundle again.
# SAYSO_BUNDLE_FULL=1 turns that off (scripts/release.sh sets it).
set -euo pipefail
cd "$(dirname "$0")/.."
profile=release
[[ "${1:-}" == "--debug" ]] && profile=debug
identity="${SAYSO_SIGN_IDENTITY:-Developer ID Application: Watzon Ventures LLc (MB5789APU7)}"
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
build=$(date +%Y%m%d%H%M)
app="${SAYSO_BUNDLE_DIR:-build}/Sayso.app"
ent=packaging/Sayso.entitlements
mkdir -p build

# hash_files prints one hash of every file under the paths, build output left out.
hash_files() {
  find "$@" \( -name .build -o -name target \) -prune -o -type f -print0 |
    LC_ALL=C sort -z | xargs -0 shasum -a 256 | shasum -a 256 | cut -d' ' -f1
}
# file_id changes when a build writes the file again.
file_id() { stat -f '%m %z' "$1" 2>/dev/null || echo none; }

engine_dir=native/macos/SaysoEngine
engine_bin=$engine_dir/.build/release/SaysoEngine
engine_hash=$(hash_files "$engine_dir/Sources" "$engine_dir/Package.swift" "$engine_dir/Package.resolved")
# Everything in the bundle except the Go app.
bundle_inputs=(scripts/bundle.sh Cargo.toml native/cleanup packaging/Info.plist "$ent" packaging/macos assets/app/AppIcon.icns)
bundle_stamp() {
  printf '%s\n' "$app" "$identity" "$engine_hash" "$(hash_files "${bundle_inputs[@]}")" \
    "$(file_id "$engine_bin")" "$(file_id native/cleanup/target/release/sayso-cleanup)" \
    "$(file_id "$app/Contents/MacOS/SaysoEngine")" "$(file_id "$app/Contents/MacOS/sayso-cleanup")" \
    "$(file_id "$app/Contents/Info.plist")"
}

if [[ "$profile" == debug && "${SAYSO_BUNDLE_FULL:-}" != 1 && -x "$app/Contents/MacOS/Sayso" && -x "$app/Contents/MacOS/SaysoEngine" &&
  "$(cat build/bundle.stamp 2>/dev/null)" == "$(bundle_stamp)" ]]; then
  echo "==> app (go, debug), the rest of $app is unchanged"
  CGO_ENABLED=0 go build -o build/sayso ./cmd/sayso
  # A new file, not new bytes in the old one: a running Sayso keeps its copy.
  rm -f "$app/Contents/MacOS/Sayso"
  cp build/sayso "$app/Contents/MacOS/Sayso"
  # Only the outer bundle: the signatures inside stay, and the identifier
  # keeps the permission grants.
  codesign --force --timestamp=none --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso -s "$identity" "$app"
  codesign --verify --deep --strict "$app"
  echo "==> signature ok: $app"
  exit 0
fi

if [[ -f "$engine_bin" && "$(cat build/engine.stamp 2>/dev/null)" == "$engine_hash $(file_id "$engine_bin")" ]]; then
  echo "==> engine (swift, release), unchanged"
else
  echo "==> engine (swift, release)"
  (cd "$engine_dir" && swift build -c release 2>&1 | tail -1)
  echo "$engine_hash $(file_id "$engine_bin")" > build/engine.stamp
fi
echo "==> cleanup runtime (cargo, release)"
# llama.cpp is slow in a debug build, so the runtime is always a release build.
cargo build --release --manifest-path native/cleanup/Cargo.toml --quiet
echo "==> app (go, $profile)"
if [[ "$profile" == release ]]; then
  CGO_ENABLED=0 go build -tags mygo_noinspector -ldflags="-s -w" -o build/sayso ./cmd/sayso
else
  CGO_ENABLED=0 go build -o build/sayso ./cmd/sayso
fi

rm -rf "$app" build/bundle.stamp
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp build/sayso "$app/Contents/MacOS/Sayso"
cp native/macos/SaysoEngine/.build/release/SaysoEngine "$app/Contents/MacOS/SaysoEngine"
# SwiftPM leaves the symbol table in the release binary: 6 MB.
strip "$app/Contents/MacOS/SaysoEngine"
# The app finds the cleanup runtime next to its executable
# (internal/localengine and the cleanup client).
cp native/cleanup/target/release/sayso-cleanup "$app/Contents/MacOS/sayso-cleanup"
strip "$app/Contents/MacOS/sayso-cleanup"
cp assets/app/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" packaging/Info.plist > "$app/Contents/Info.plist"
# The microphone prompt in each reviewed interface language
# (scripts/i18n-system-files.py writes these folders).
for lproj in packaging/macos/*.lproj; do
  [ -d "$lproj" ] && cp -R "$lproj" "$app/Contents/Resources/"
done
printf 'APPL????' > "$app/Contents/PkgInfo"

echo "==> sign ($identity)"
codesign --force --timestamp --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso.engine -s "$identity" "$app/Contents/MacOS/SaysoEngine"
# The cleanup runtime records no audio, so it gets no entitlements.
codesign --force --timestamp --options runtime --identifier dev.sayso.Sayso.cleanup -s "$identity" "$app/Contents/MacOS/sayso-cleanup"
codesign --force --timestamp --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso -s "$identity" "$app"
codesign --verify --deep --strict "$app"
echo "==> signature ok: $app"
bundle_stamp > build/bundle.stamp
