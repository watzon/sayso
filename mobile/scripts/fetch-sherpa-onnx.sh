#!/usr/bin/env bash
# Downloads the official sherpa-onnx Android prebuilt (Apache-2.0) that the
# keyboard's :asr process uses for local speech recognition, and checks its
# sha256 against the pin below. The AAR holds the Kotlin API and one
# libsherpa-onnx-jni.so per ABI with ONNX Runtime linked in statically; the
# app build packages only arm64-v8a and x86_64 from it.
#
# Writes mobile/build/sherpa-onnx/sherpa-onnx-<version>.aar and prints its path.
# A file that is already there is checked again, not downloaded again.
set -euo pipefail
cd "$(dirname "$0")/.."

# https://github.com/k2-fsa/sherpa-onnx/releases/tag/v2.0.3, checksum.txt of that release.
version=2.0.3
name="sherpa-onnx-static-link-onnxruntime-$version.aar"
sha256=7744a3a8f08f017d59c2d8ed8d9b700345561f5abe9f4e06dc6eaef3a0b977b4
url="https://github.com/k2-fsa/sherpa-onnx/releases/download/v$version/$name"

dir=build/sherpa-onnx
out="$dir/sherpa-onnx-$version.aar"
mkdir -p "$dir"

sum() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi
}

if [[ -f "$out" && "$(sum "$out")" == "$sha256" ]]; then
  echo "$PWD/$out"
  exit 0
fi

tmp="$out.download"
rm -f "$tmp"
curl -fsSL --retry 3 -o "$tmp" "$url"
got="$(sum "$tmp")"
if [[ "$got" != "$sha256" ]]; then
  rm -f "$tmp"
  echo "fetch-sherpa-onnx: $name sha256 $got, expected $sha256" >&2
  exit 1
fi
mv "$tmp" "$out"
echo "$PWD/$out"
