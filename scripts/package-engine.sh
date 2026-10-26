#!/usr/bin/env bash
# Make the engine release file for macOS or Linux: a tarball of the speech
# engine with its README and licenses, and its checksum.
#
#   scripts/package-engine.sh [--out <folder>] [--require-signed] <engine binary>
#
#   dist/sayso-engine-<version>-<macos-arm64|linux-x86_64|linux-arm64>.tar.gz
#   dist/sayso-engine-<version>-<...>.tar.gz.sha256
#
# The file holds the folder sayso-engine-<version>/ with sayso-engine, README.md,
# LICENSE, and licenses/. The script copies the engine that you name. It never
# builds an engine, so the release file has the binary that was signed in the
# app bundle (macOS: dist/Sayso.app/Contents/MacOS/SaysoEngine, after
# scripts/release.sh; Linux: <bundle folder>/sayso-<version>-linux-<arch>/bin/sayso-engine,
# after scripts/bundle-linux.sh). The file name of the copy is always
# sayso-engine, and a rename keeps a macOS signature valid.
#
# --require-signed (macOS) stops the script when the engine has no valid
# Developer ID signature. The release workflow uses it. Without it, the
# script packs an engine that is not signed (CI tests, a local run).
# The script runs in the repository root, so a relative path is relative to it.
# The Windows file is made by scripts/package-engine.ps1.
set -euo pipefail
cd "$(dirname "$0")/.."
usage() { echo "usage: $0 [--out <folder>] [--require-signed] <engine binary>" >&2; exit 2; }
out=dist; require_signed=0; engine=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) [[ $# -ge 2 ]] || usage; out=$2; shift 2 ;;
    --require-signed) require_signed=1; shift ;;
    -*) usage ;;
    *) [[ -z "$engine" ]] || usage; engine=$1; shift ;;
  esac
done
[[ -n "$engine" ]] || usage
[[ -f "$engine" ]] || { echo "error: the engine $engine does not exist" >&2; exit 1; }

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
case "$(uname -m)" in
  arm64|aarch64) arch=arm64 ;;
  x86_64) arch=x86_64 ;;
  *) echo "error: no engine file for the architecture $(uname -m)" >&2; exit 1 ;;
esac
case "$(uname -s)" in
  Darwin) system=macos ;;
  Linux) system=linux ;;
  *) echo "error: this script runs on macOS and Linux. Use scripts/package-engine.ps1 on Windows." >&2; exit 1 ;;
esac
platform="$system-$arch"
[[ $system == macos && $arch != arm64 ]] && { echo "error: the macOS engine is for arm64 only" >&2; exit 1; }
[[ $require_signed -eq 1 && $system != macos ]] && { echo "error: --require-signed is for macOS" >&2; exit 2; }

name="sayso-engine-$version"
file="$name-$platform.tar.gz"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name/licenses" "$out"
dir="$stage/$name"

install -m 755 "$engine" "$dir/sayso-engine"
cp LICENSE "$dir/LICENSE"

# The libraries of the engine must be the libraries of the system. A library
# that is not part of the system would have to be in the file.
case "$system" in
  macos)
    deps=$(otool -L "$dir/sayso-engine" | tail -n +2)
    echo "$deps"
    if grep -vE '^[[:space:]]*(/System/Library/|/usr/lib/)' <<< "$deps"; then
      echo "error: the engine links a library that is not part of macOS." >&2
      exit 1
    fi
    if [[ $require_signed -eq 1 ]]; then
      codesign --verify --strict "$dir/sayso-engine"
      authority=$(codesign -dvv "$dir/sayso-engine" 2>&1 | sed -n 's/^Authority=//p' | head -1)
      [[ "$authority" == "Developer ID Application:"* ]] || { echo "error: the engine has no Developer ID signature (authority: ${authority:-none})." >&2; exit 1; }
      echo "==> signature ok: $authority"
    else
      echo "warning: the engine is not checked for a Developer ID signature." >&2
    fi
    # The app bundle ships no license files. The engine links these two Swift
    # packages of Package.swift statically, and Apache-2.0 needs the license
    # text with the binary. FluidAudio also has the licenses of its own code.
    shopt -s nullglob
    for pkgname in FluidAudio argmax-oss-swift; do
      pkg=native/macos/SaysoEngine/.build/checkouts/$pkgname
      [[ -f "$pkg/LICENSE" ]] || { echo "error: $pkg/LICENSE is missing. Build the engine first (swift build)." >&2; exit 1; }
      for license in "$pkg"/LICENSE* "$pkg"/NOTICE* "$pkg"/ThirdPartyLicenses/*; do
        [[ -f "$license" ]] && install -m 644 "$license" "$dir/licenses/$pkgname-$(basename "$license")"
      done
    done
    shopt -u nullglob
    ;;
  linux)
    deps=$(ldd "$dir/sayso-engine")
    echo "$deps"
    if grep -q 'not found' <<< "$deps"; then
      echo "error: the engine has a library that is missing." >&2
      exit 1
    fi
    # The same files as bin/ and share/licenses/sayso in the app tarball
    # (scripts/bundle-linux.sh): the MIT licenses of parakeet.cpp and ggml.
    install -m 644 -t "$dir/licenses" crates/sayso-engine/licenses/*.txt
    ;;
esac

# The README: the same text for each system, with the command for this one.
case "$system" in
  macos)
    system_name=macOS
    requirements='macOS 14 or later, on a Mac with Apple silicon.' ;;
  linux)
    system_name=Linux
    # build.rs links the Vulkan loader (crates/sayso-engine/build.rs), and
    # the glibc is the oldest one that the release build supports (linux.yml).
    requirements='glibc 2.35 or newer, and the Vulkan loader (`libvulkan.so.1`). On Debian and Ubuntu, the package is `libvulkan1`. The engine does not start without the loader, also when a model runs on the CPU.' ;;
esac
run='SAYSO_MODELS_DIR="$HOME/sayso-models" ./sayso-engine'
hello='export SAYSO_MODELS_DIR="$HOME/sayso-models"
(echo '"'"'{"v":1,"id":"1","type":"hello"}'"'"'; sleep 2) | ./sayso-engine'
readme=$(cat packaging/engine/README.md.in)
readme=${readme//@VERSION@/$version}
readme=${readme//@SYSTEM@/$system_name}
readme=${readme//@REQUIREMENTS@/$requirements}
readme=${readme//@RUN@/$run}
readme=${readme//@HELLO@/$hello}
printf '%s\n' "$readme" > "$dir/README.md"
chmod 644 "$dir/README.md"

# bsdtar (macOS) and GNU tar (Linux) name the owner with different options.
# COPYFILE_DISABLE keeps the ._ files of macOS out of the file.
tar_owner=(--owner=0 --group=0 --numeric-owner)
[[ $system == macos ]] && tar_owner=(--uid 0 --gid 0 --numeric-owner)
rm -f "$out/$file" "$out/$file.sha256"
COPYFILE_DISABLE=1 tar -C "$stage" "${tar_owner[@]}" -czf "$out/$file" "$name"
if command -v sha256sum >/dev/null; then
  (cd "$out" && sha256sum "$file" > "$file.sha256")
else
  (cd "$out" && shasum -a 256 "$file" > "$file.sha256")
fi
echo "==> engine file:"
/bin/ls -lh "$out/$file" "$out/$file.sha256"
