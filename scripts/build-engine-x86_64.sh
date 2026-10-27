#!/usr/bin/env bash
# Build the speech engine for Intel Macs: dist/engine-x86_64/SaysoEngine.
#
#   scripts/build-engine-x86_64.sh                    build, strip, sign, notarize
#   scripts/build-engine-x86_64.sh --skip-notarize    build, strip, sign. Nothing goes to Apple.
#   SAYSO_SIGN_IDENTITY=- scripts/build-engine-x86_64.sh
#                                                     ad hoc signature, no notarization (CI, local tests)
#   scripts/build-engine-x86_64.sh --out <folder>     write to another folder
#
# Sayso itself runs on Apple silicon only. Only the engine release file
# (sayso-engine-<version>-macos-x86_64.tar.gz) has an Intel build. This script
# cross-compiles on a Mac with Apple silicon. Then scripts/package-engine.sh
# packs the result.
#
# The signature is the same as the one of the engine in the app bundle
# (scripts/bundle.sh): the same identity, the same identifier, the same
# entitlements, the hardened runtime, and a secure timestamp. Set
# SAYSO_SIGN_IDENTITY to use another identity.
#
# A bare executable cannot have a stapled ticket, so there is no staple step.
# macOS looks the notarization ticket up online. The credentials for
# notarization are the ones of scripts/notarize-lib.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
usage() { echo "usage: $0 [--skip-notarize] [--out <folder>]" >&2; exit 2; }
notarize=1
out=dist/engine-x86_64
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-notarize) notarize=0; shift ;;
    --out) [[ $# -ge 2 ]] || usage; out=$2; shift 2 ;;
    *) usage ;;
  esac
done
# The same default as scripts/bundle.sh.
identity="${SAYSO_SIGN_IDENTITY:-Developer ID Application: Watzon Ventures LLc (MB5789APU7)}"
[[ "$identity" == "-" ]] && notarize=0

echo "==> engine (swift, release, x86_64)"
# The build uses the .build folder of the native build. Some versions of
# SwiftPM write the Intel product to the path of the native one, so the next
# native build (scripts/bundle.sh) builds again. Ask SwiftPM for the path.
(cd native/macos/SaysoEngine && swift build -c release --arch x86_64 2>&1 | tail -1)
built=$(cd native/macos/SaysoEngine && swift build -c release --arch x86_64 --show-bin-path)/SaysoEngine
[[ -f "$built" ]] || { echo "error: the build did not make $built" >&2; exit 1; }

mkdir -p "$out"
engine="$out/SaysoEngine"
rm -f "$engine"
cp "$built" "$engine"
# SwiftPM leaves the symbol table in the release binary: 6 MB.
strip "$engine"
[[ "$(lipo -archs "$engine")" == "x86_64" ]] || { echo "error: the engine is not an x86_64 binary: $(lipo -archs "$engine")" >&2; exit 1; }

echo "==> sign ($identity)"
# An ad hoc signature gets no timestamp and no hardened runtime.
sign_flags=(--force --entitlements packaging/Sayso.entitlements --identifier dev.sayso.Sayso.engine)
[[ "$identity" == "-" ]] || sign_flags+=(--timestamp --options runtime)
codesign "${sign_flags[@]}" -s "$identity" "$engine"
codesign --verify --strict "$engine" && echo "==> signature ok: $engine"

if [[ $notarize -eq 1 ]]; then
  # shellcheck source=scripts/notarize-lib.sh
  source scripts/notarize-lib.sh
  echo "==> notarize the engine (uploads it to Apple)"
  zip="$out/SaysoEngine-notarize.zip"
  rm -f "$zip"
  ditto -c -k --keepParent "$engine" "$zip"
  notarize "$zip"
  rm -f "$zip"
fi
/bin/ls -l "$engine"
