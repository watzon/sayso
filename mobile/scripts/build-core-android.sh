#!/usr/bin/env bash
# Builds the typing core as libsayso_core.so for Android arm64-v8a and
# x86_64 into mobile/build/jniLibs/<abi>/, and copies the C header to
# mobile/build/include/. Pages are 16 KB aligned (Android 15+ devices).
#
#   ANDROID_NDK_HOME  NDK to use (default: the newest under ~/Library/Android/sdk/ndk)
#   ANDROID_API       minimum API level (default 24)
set -euo pipefail
cd "$(dirname "$0")/.."

ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [[ -z "$ndk" ]]; then
  ndk="$(ls -d "$HOME"/Library/Android/sdk/ndk/* "$HOME"/Android/Sdk/ndk/* 2>/dev/null | sort -V | tail -1 || true)"
fi
[[ -d "$ndk" ]] || { echo "no Android NDK found; set ANDROID_NDK_HOME" >&2; exit 1; }
api="${ANDROID_API:-24}"
host="$(ls "$ndk/toolchains/llvm/prebuilt" | head -1)"
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"

out=build/jniLibs
mkdir -p build/include
cp core/capi/sayso_core.h build/include/

build() { # abi goarch clang-triple
  local abi=$1 goarch=$2 triple=$3
  mkdir -p "$out/$abi"
  CGO_ENABLED=1 GOOS=android GOARCH="$goarch" CC="$bin/${triple}${api}-clang" \
    go build -buildmode=c-shared -trimpath \
    -ldflags="-s -w -extldflags=-Wl,-z,max-page-size=16384,-soname,libsayso_core.so" \
    -o "$out/$abi/libsayso_core.so" ./core/capi
  rm -f "$out/$abi/libsayso_core.h" # the committed header is build/include/sayso_core.h
  local align
  align="$("$bin/llvm-readelf" -lW "$out/$abi/libsayso_core.so" | awk '$1=="LOAD"{print $NF}' | sort -u | tr '\n' ' ')"
  local syms
  syms="$("$bin/llvm-nm" -D --defined-only "$out/$abi/libsayso_core.so" | grep -c ' T sayso_')"
  printf '%-12s %8s bytes  LOAD align %s exported sayso_* %s\n' "$abi" "$(wc -c <"$out/$abi/libsayso_core.so" | tr -d ' ')" "$align" "$syms"
}

build arm64-v8a arm64 aarch64-linux-android
build x86_64 amd64 x86_64-linux-android
