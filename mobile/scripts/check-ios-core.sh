#!/usr/bin/env bash
# Checks that the typing core builds as a static C archive for iOS arm64
# with Xcode's clang and SDK, and reports its size. The archive goes to
# mobile/build/ios/libsayso_core.a.
#
#   IOS_MIN  minimum iOS version (default 15.0)
set -euo pipefail
cd "$(dirname "$0")/.."
command -v xcrun >/dev/null || { echo "Xcode is needed" >&2; exit 1; }
sdk="$(xcrun --sdk iphoneos --show-sdk-path)"
cc="$(xcrun --sdk iphoneos -f clang)"
min="${IOS_MIN:-15.0}"
flags="-isysroot $sdk -arch arm64 -miphoneos-version-min=$min"
out=build/ios
mkdir -p "$out"
CGO_ENABLED=1 GOOS=ios GOARCH=arm64 CC="$cc" CGO_CFLAGS="$flags" CGO_LDFLAGS="$flags" \
  go build -buildmode=c-archive -trimpath -ldflags="-s -w" -o "$out/libsayso_core.a" ./core/capi
rm -f "$out/libsayso_core.h"
cp core/capi/sayso_core.h "$out/"
size="$(wc -c <"$out/libsayso_core.a" | tr -d ' ')"
syms="$(nm -gU "$out/libsayso_core.a" 2>/dev/null | grep -c ' T _sayso_' || true)"
arch="$(lipo -archs "$out/libsayso_core.a" 2>/dev/null || echo '?')"
echo "ios arm64 c-archive: $size bytes ($(awk "BEGIN{printf \"%.1f\", $size/1048576}") MiB), archs $arch, exported sayso_* $syms (min iOS $min, SDK $(basename "$sdk"))"
[[ "$syms" -gt 0 ]] || { echo "no sayso_ symbols exported" >&2; exit 1; }
