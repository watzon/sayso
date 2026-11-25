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
# SAYSO_BUNDLE_FULL=1 turns that off and signs with timestamps (scripts/release.sh sets it).
#
# The Swift engine is built again only when its sources or Package files change
# (build/engine.stamp). The llama.cpp libraries of the cleanup runtime come from
# build/llama-libs, downloaded once and checked against the pins
# (scripts/llama-libs, internal/llamalib).
set -euo pipefail
cd "$(dirname "$0")/.."
export CGO_ENABLED=0
profile=release
[[ "${1:-}" == "--debug" ]] && profile=debug
identity="${SAYSO_SIGN_IDENTITY:-Developer ID Application: Watzon Ventures LLc (MB5789APU7)}"
version=$(scripts/version.sh)
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
bundle_inputs=(
  scripts/bundle.sh
  internal/update/version.go
  go.mod
  go.sum
  cmd/sayso-cleanup
  internal/cleanupd
  internal/llamalib
  scripts/llama-libs
  packaging/Info.plist
  "$ent"
  packaging/macos
  packaging/licenses
  assets/app/AppIcon.icns
)
bundle_stamp() {
  printf '%s\n' "$app" "$identity" "$engine_hash" "$(hash_files "${bundle_inputs[@]}")" \
    "$(file_id "$engine_bin")" "$(file_id build/sayso-cleanup)" \
    "$(file_id "$app/Contents/MacOS/SaysoEngine")" "$(file_id "$app/Contents/MacOS/sayso-cleanup")" \
    "$(file_id "$app/Contents/Info.plist")"
}

if [[ "$profile" == debug && "${SAYSO_BUNDLE_FULL:-}" != 1 && -x "$app/Contents/MacOS/Sayso" &&
  -x "$app/Contents/MacOS/SaysoEngine" && -x "$app/Contents/MacOS/sayso-cleanup" &&
  -d "$app/Contents/Frameworks/llama" &&
  "$(cat build/bundle.stamp 2>/dev/null)" == "$(bundle_stamp)" ]]; then
  echo "==> app (go, debug), the rest of $app is unchanged"
  go build -ldflags="-X github.com/watzon/sayso/internal/update.Version=$version" -o build/sayso ./cmd/sayso
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

echo "==> llama.cpp libraries (pinned)"
go build -o build/llama-libs-tool ./scripts/llama-libs
tool=build/llama-libs-tool
llama_libs=$("$tool" fetch -os darwin -arch arm64 -out build/llama-libs)

echo "==> app and cleanup runtime (go, $profile)"
# The cleanup runtime loads the libffi of the bundle, not a copy that its
# libffi binding would write to the user cache (library validation refuses
# that copy): -tags ffi_no_embed and the file name in -X.
ffi_flag=$("$tool" ldflags -os darwin)
# The app reports the version of the bundle (Info.plist), which the updater
# compares with the newest release.
version_flag="-X github.com/watzon/sayso/internal/update.Version=$version"
# A release (SAYSO_OFFICIAL_BUILD=1, set by the Release workflow) updates
# itself and sends telemetry as a release, not in test mode.
if [[ "${SAYSO_OFFICIAL_BUILD:-}" == 1 ]]; then
  version_flag+=" -X github.com/watzon/sayso/internal/update.OfficialBuild=1"
fi
# The TelemetryDeck App ID of the official app (internal/telemetry). Without
# it the build sends no signal.
if [[ -n "${SAYSO_TELEMETRY_APP_ID:-}" ]]; then
  version_flag+=" -X github.com/watzon/sayso/internal/telemetry.DefaultAppID=$SAYSO_TELEMETRY_APP_ID"
fi
if [[ "$profile" == release ]]; then
  go build -tags mygo_noinspector -ldflags="-s -w $version_flag" -o build/sayso ./cmd/sayso &
  go build -tags ffi_no_embed -ldflags="-s -w $ffi_flag" -o build/sayso-cleanup ./cmd/sayso-cleanup
else
  go build -ldflags="$version_flag" -o build/sayso ./cmd/sayso &
  go build -tags ffi_no_embed -ldflags="$ffi_flag" -o build/sayso-cleanup ./cmd/sayso-cleanup
fi
wait $!

# A release (or SAYSO_BUNDLE_FULL=1 from release.sh) needs secure timestamps
# for notarization. A local debug build does not, and the timestamp server is
# a network round trip per file. The timestamp is not part of the designated
# requirement, so permission grants do not change.
sign=(codesign --force -s "$identity")
sign_mode=debug
if [[ "$profile" == release || "${SAYSO_BUNDLE_FULL:-}" == 1 ]]; then
  sign+=(--timestamp)
  sign_mode=timestamp
else
  sign+=(--timestamp=none)
fi

# The libraries of the cleanup runtime and libffi, each signed with the
# identity of the app, so the library validation of the hardened runtime
# accepts them. A local debug build keeps the signed copies for the next build.
signed="build/llama-libs/signed-$(basename "$llama_libs")-$(printf '%s\n%s' "$identity" "$sign_mode" | shasum -a 256 | cut -c1-12)"
if [[ "$sign_mode" == timestamp || ! -f "$signed/.done" ]]; then
  rm -rf "$signed"
  mkdir -p "$signed"
  cp -a "$llama_libs"/*.dylib "$signed/"
  "$tool" ffi -os darwin -arch arm64 "$signed"
  find "$signed" -type f -name '*.dylib' -print0 | xargs -0 "${sign[@]}"
  touch "$signed/.done"
fi

rm -rf "$app" build/bundle.stamp
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
# llama.cpp's license is ggml's MIT text (ggml.txt, byte for byte); not every
# llama.cpp archive has its LICENSE.
cp packaging/licenses/ggml.txt "$app/Contents/Resources/licenses/llama.cpp.txt"
cp packaging/licenses/libffi.txt "$app/Contents/Resources/licenses/"
cp assets/app/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" packaging/Info.plist > "$app/Contents/Info.plist"
# The microphone prompt in each reviewed interface language
# (scripts/i18n-system-files.py writes these folders).
for lproj in packaging/macos/*.lproj; do
  [ -d "$lproj" ] && cp -R "$lproj" "$app/Contents/Resources/"
done
printf 'APPL????' > "$app/Contents/PkgInfo"

echo "==> sign ($identity)"
"${sign[@]}" --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso.engine "$app/Contents/MacOS/SaysoEngine"
# The cleanup runtime records no audio, so it gets no entitlements. It keeps
# library validation: it loads only the libraries signed above.
"${sign[@]}" --options runtime --identifier dev.sayso.Sayso.cleanup "$app/Contents/MacOS/sayso-cleanup"
"${sign[@]}" --options runtime --entitlements "$ent" --identifier dev.sayso.Sayso "$app"
codesign --verify --deep --strict "$app"
echo "==> signature ok: $app"
bundle_stamp > build/bundle.stamp
