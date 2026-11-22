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
# The Swift engine and the MediaRemoteAdapter framework are built again only
# when their sources (native/macos) or the tools change: a stamp in build/
# holds the SHA-256 of the inputs. The llama.cpp libraries of the cleanup
# runtime come from build/llama-libs, downloaded once and checked against the
# pins (scripts/llama-libs, internal/llamalib). Nothing here runs Cargo.
set -euo pipefail
cd "$(dirname "$0")/.."
profile=release
[[ "${1:-}" == "--debug" ]] && profile=debug
identity="${SAYSO_SIGN_IDENTITY:-Developer ID Application: Watzon Ventures LLc (MB5789APU7)}"
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
build=$(date +%Y%m%d%H%M)
export CGO_ENABLED=0
mkdir -p build

# inputs_hash <folder> <tool versions...>: the SHA-256 of every file of the
# folder (not the build folders) and of the tool versions.
inputs_hash() {
  local dir=$1; shift
  {
    printf '%s\n' "$@"
    find "$dir" \( -name .build -o -name .swiftpm -o -name build \) -prune -o -type f -print0 \
      | LC_ALL=C sort -z | xargs -0 shasum -a 256
  } | shasum -a 256 | cut -d' ' -f1
}

engine_bin=native/macos/SaysoEngine/.build/release/SaysoEngine
engine_hash=$(inputs_hash native/macos/SaysoEngine "$(swift --version 2>&1)" "$(xcrun --sdk macosx --show-sdk-path 2>&1)")
if [[ -x "$engine_bin" && "$(cat build/.engine.stamp 2>/dev/null)" == "$engine_hash" ]]; then
  echo "==> engine (swift, release): unchanged"
else
  echo "==> engine (swift, release)"
  (cd native/macos/SaysoEngine && swift build -c release 2>&1 | tail -1)
  echo "$engine_hash" > build/.engine.stamp
fi

adapter_build=build/media-remote-adapter
adapter_hash=$(inputs_hash native/macos/MediaRemoteAdapter "$(cmake --version | head -1)" "$(xcrun --sdk macosx --show-sdk-path 2>&1)")
if [[ -d "$adapter_build/MediaRemoteAdapter.framework" && "$(cat build/.adapter.stamp 2>/dev/null)" == "$adapter_hash" ]]; then
  echo "==> media remote adapter (cmake): unchanged"
else
  echo "==> media remote adapter (cmake)"
  cmake -S native/macos/MediaRemoteAdapter -B "$adapter_build" -DCMAKE_BUILD_TYPE=Release >/dev/null
  cmake --build "$adapter_build" --target MediaRemoteAdapter -j 8 >/dev/null
  echo "$adapter_hash" > build/.adapter.stamp
fi

echo "==> llama.cpp libraries (pinned)"
go build -o build/llama-libs-tool ./scripts/llama-libs
tool=build/llama-libs-tool
llama_libs=$("$tool" fetch -os darwin -arch arm64 -out build/llama-libs)

echo "==> app and cleanup runtime (go, $profile)"
# The cleanup runtime loads the libffi of the bundle, not a copy that its
# libffi binding would write to the user cache (library validation refuses
# that copy): -tags ffi_no_embed and the file name in -X.
ffi_flag=$("$tool" ldflags -os darwin)
if [[ "$profile" == release ]]; then
  go build -tags mygo_noinspector -ldflags="-s -w" -o build/sayso ./cmd/sayso &
  go build -tags ffi_no_embed -ldflags="-s -w $ffi_flag" -o build/sayso-cleanup ./cmd/sayso-cleanup
else
  go build -o build/sayso ./cmd/sayso &
  go build -tags ffi_no_embed -ldflags="$ffi_flag" -o build/sayso-cleanup ./cmd/sayso-cleanup
fi
wait $!

# A release needs secure timestamps for notarization. A debug build does not,
# and the timestamp server is a network round trip per file. The timestamp is
# not part of the designated requirement, so permission grants do not change.
sign=(codesign --force -s "$identity")
[[ "$profile" == release ]] && sign+=(--timestamp)

# The libraries of the cleanup runtime and libffi, each signed with the
# identity of the app, so the library validation of the hardened runtime
# accepts them. A debug build keeps the signed copies for the next build.
signed="build/llama-libs/signed-$(basename "$llama_libs")-$(printf '%s\n%s' "$identity" "$profile" | shasum -a 256 | cut -c1-12)"
if [[ "$profile" == release || ! -f "$signed/.done" ]]; then
  rm -rf "$signed"
  mkdir -p "$signed"
  cp -a "$llama_libs"/*.dylib "$signed/"
  "$tool" ffi -os darwin -arch arm64 "$signed"
  find "$signed" -type f -name '*.dylib' -print0 | xargs -0 "${sign[@]}"
  touch "$signed/.done"
fi

app="${SAYSO_BUNDLE_DIR:-build}/Sayso.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/licenses" "$app/Contents/Frameworks/llama"
cp build/sayso "$app/Contents/MacOS/Sayso"
cp "$engine_bin" "$app/Contents/MacOS/SaysoEngine"
# SwiftPM leaves the symbol table in the release binary: 6 MB.
strip "$app/Contents/MacOS/SaysoEngine"
# The app finds the cleanup runtime next to its executable, and the runtime
# finds llama.cpp in Contents/Frameworks/llama (internal/llamalib): code goes in
# Frameworks, not in Resources, and a folder of its own keeps this ggml away
# from any other.
cp build/sayso-cleanup "$app/Contents/MacOS/sayso-cleanup"
cp -a "$signed"/*.dylib "$app/Contents/Frameworks/llama/"
cp "$llama_libs/LICENSE" "$app/Contents/Resources/licenses/llama.cpp.txt"
cp packaging/licenses/libffi.txt "$app/Contents/Resources/licenses/"
cp assets/app/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
# Pause media while recording: /usr/bin/perl runs the script, and the script
# loads the framework (internal/platform).
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
"${sign[@]}" --options runtime "$app/Contents/Frameworks/MediaRemoteAdapter.framework"
"${sign[@]}" --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso.engine "$app/Contents/MacOS/SaysoEngine"
# The cleanup runtime records no audio, so it gets no entitlements. It keeps
# library validation: it loads only the libraries signed above.
"${sign[@]}" --options runtime --identifier dev.sayso.Sayso.cleanup "$app/Contents/MacOS/sayso-cleanup"
"${sign[@]}" --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso "$app"
codesign --verify --deep --strict "$app" && echo "==> signature ok: $app"
