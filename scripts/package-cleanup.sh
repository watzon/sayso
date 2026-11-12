#!/usr/bin/env bash
# Make the cleanup runtime release file for macOS or Linux: a tarball of the
# runtime sayso-cleanup with its README and licenses, and its checksum.
#
#   scripts/package-cleanup.sh [--out <folder>] [--require-signed] [--no-smoke] <folder with the builds>
#
#   dist/sayso-cleanup-<version>-<macos-arm64|macos-x86_64|linux-x86_64|linux-arm64>.tar.gz
#   dist/sayso-cleanup-<version>-<...>.tar.gz.sha256
#
# The file holds the folder sayso-cleanup-<version>/ with the builds of the
# runtime, README.md, LICENSE, and licenses/. The folder that you name holds
# the builds that scripts/build-cleanup.sh makes: sayso-cleanup, and on Linux
# x86_64 also sayso-cleanup-avx and sayso-cleanup-baseline (llama.cpp is
# compiled for one instruction set, issue #37, and the SDK starts the best
# build that the processor runs: sayso_cleanup_models::cpu::best_in). The
# script copies these files. It never builds a runtime, and it never signs one.
# So on macOS the release workflow gives it the binary that was signed in the
# app bundle (macOS arm64: dist/Sayso.app/Contents/MacOS, after
# scripts/release.sh), or the Intel binary that it signs and notarizes itself.
# On Linux it gives the folder bin/ of the app tarball
# (<bundle folder>/sayso-<version>-linux-<arch>/bin, after
# scripts/bundle-linux.sh). Other files in the folder are ignored. A rename
# keeps a macOS signature valid.
#
# The builds of a system:
#   macOS arm64, macOS x86_64, Linux arm64: sayso-cleanup (one build)
#   Linux x86_64: sayso-cleanup, sayso-cleanup-avx, sayso-cleanup-baseline
# macOS x86_64 has one build because every Mac that runs macOS 14 has AVX2.
# The Windows file is made by scripts/package-cleanup.ps1.
#
# The architecture in the file name is the architecture of the runtime, not of
# this machine. On macOS the script reads it with lipo, so a Mac with Apple
# silicon packs the Intel runtime too. On Linux it reads the ELF header. The
# script stops when the builds have different architectures, or more than one.
#
# --require-signed (macOS) stops the script when a build has no valid
# Developer ID signature. The release workflow uses it. Without it, the
# script packs a runtime that is not signed (CI tests, a local run).
# --no-smoke skips the start test. By default, when the runtime has the
# architecture of this machine, the script starts each build in the file and
# sends it a hello request. The script runs in the repository root, so a
# relative path is relative to it.
set -euo pipefail
cd "$(dirname "$0")/.."
usage() { echo "usage: $0 [--out <folder>] [--require-signed] [--no-smoke] <folder with the builds>" >&2; exit 2; }
out=dist; require_signed=0; smoke=1; source_dir=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) [[ $# -ge 2 ]] || usage; out=$2; shift 2 ;;
    --require-signed) require_signed=1; shift ;;
    --no-smoke) smoke=0; shift ;;
    -*) usage ;;
    *) [[ -z "$source_dir" ]] || usage; source_dir=$1; shift ;;
  esac
done
[[ -n "$source_dir" ]] || usage
[[ -d "$source_dir" ]] || { echo "error: $source_dir is not a folder" >&2; exit 1; }

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
case "$(uname -s)" in
  Darwin) system=macos ;;
  Linux) system=linux ;;
  *) echo "error: this script runs on macOS and Linux. Use scripts/package-cleanup.ps1 on Windows." >&2; exit 1 ;;
esac
[[ $require_signed -eq 1 && $system != macos ]] && { echo "error: --require-signed is for macOS" >&2; exit 2; }

# The builds that exist in the folder.
builds=()
for build in sayso-cleanup sayso-cleanup-avx sayso-cleanup-baseline; do
  [[ -f "$source_dir/$build" ]] && builds+=("$build")
done
[[ " ${builds[*]:-} " == *" sayso-cleanup "* ]] || { echo "error: $source_dir has no sayso-cleanup. Run scripts/build-cleanup.sh first." >&2; exit 1; }

# The architecture of the builds, not of this machine, goes into the file name.
arch=
for build in "${builds[@]}"; do
  file="$source_dir/$build"
  case "$system" in
    macos)
      # lipo names two or more architectures for a universal binary.
      case "$(lipo -archs "$file")" in
        arm64) this=arm64 ;;
        x86_64) this=x86_64 ;;
        *) echo "error: $build must have exactly one architecture, arm64 or x86_64. It has: $(lipo -archs "$file")" >&2; exit 1 ;;
      esac ;;
    linux)
      # The ELF header has the machine in 2 bytes at offset 18: 3e 00 is
      # x86-64, b7 00 is AArch64. This needs no tool that a container can lack.
      case "$(od -An -tx1 -j18 -N2 "$file" | tr -d ' \n')" in
        3e00) this=x86_64 ;;
        b700) this=arm64 ;;
        *) echo "error: $build is not an x86_64 or arm64 ELF file." >&2; exit 1 ;;
      esac ;;
  esac
  [[ -z "$arch" || "$arch" == "$this" ]] || { echo "error: the builds have different architectures ($arch and $this)." >&2; exit 1; }
  arch=$this
done
platform="$system-$arch"

# Which builds a platform must have.
if [[ "$system" == linux && "$arch" == x86_64 ]]; then
  expected="sayso-cleanup sayso-cleanup-avx sayso-cleanup-baseline"
else
  expected="sayso-cleanup"
fi
[[ "${builds[*]}" == "$expected" ]] || { echo "error: $platform must have these builds: $expected. The folder has: ${builds[*]}" >&2; exit 1; }

name="sayso-cleanup-$version"
file="$name-$platform.tar.gz"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name/licenses" "$out"
dir="$stage/$name"

for build in "${builds[@]}"; do
  install -m 755 "$source_dir/$build" "$dir/$build"
done
cp LICENSE "$dir/LICENSE"
# llama.cpp is linked statically, and it has the same MIT license text as ggml
# (the app bundles ship this one file for it: scripts/bundle-linux.sh,
# scripts/bundle-windows.ps1).
install -m 644 crates/sayso-engine/licenses/ggml.txt "$dir/licenses/ggml.txt"

# The libraries of the runtime must be the libraries of the system. A library
# that is not part of the system would have to be in the file.
for build in "${builds[@]}"; do
  case "$system" in
    macos)
      deps=$(otool -L "$dir/$build" | tail -n +2)
      echo "$deps"
      if grep -vE '^[[:space:]]*(/System/Library/|/usr/lib/)' <<< "$deps"; then
        echo "error: $build links a library that is not part of macOS." >&2
        exit 1
      fi
      if [[ $require_signed -eq 1 ]]; then
        codesign --verify --strict "$dir/$build"
        authority=$(codesign -dvv "$dir/$build" 2>&1 | sed -n 's/^Authority=//p' | head -1)
        [[ "$authority" == "Developer ID Application:"* ]] || { echo "error: $build has no Developer ID signature (authority: ${authority:-none})." >&2; exit 1; }
        echo "==> signature ok: $authority"
      else
        echo "warning: $build is not checked for a Developer ID signature." >&2
      fi ;;
    linux)
      deps=$(ldd "$dir/$build")
      echo "$deps"
      # OpenMP is off (native/cleanup/Cargo.toml), so no libgomp.
      if grep -qE 'libgomp|not found' <<< "$deps"; then
        echo "error: $build has a library that it must not have, or one that is missing." >&2
        exit 1
      fi ;;
  esac
done

# Start each build and send it a hello request. The runtime needs no model to
# answer. This runs only when the machine can run the build.
host=$(uname -m); [[ "$host" == aarch64 ]] && host=arm64
if [[ $smoke -eq 1 && "$host" == "$arch" ]]; then
  for build in "${builds[@]}"; do
    # Not every processor runs every x86_64 build. SIGILL (132) on a build for
    # a newer instruction set than the processor has is not an error here: the
    # SDK picks the best build that runs (cpu::best_in).
    status=0
    reply=$(printf '{"v":1,"id":"1","type":"hello"}\n' | SAYSO_CLEANUP_LOG=off "$dir/$build" 2>/dev/null) || status=$?
    if [[ $status -eq 132 && $system == linux ]]; then
      echo "==> $build: this processor does not run it (illegal instruction), start test skipped"
      continue
    fi
    echo "==> $build: $reply"
    [[ "$reply" == *'"type":"hello"'* ]] || { echo "error: $build gave no hello reply (status $status)." >&2; exit 1; }
  done
else
  echo "==> start test skipped"
fi

# The README: what it is, how the SDK uses it, and what it needs.
if [[ "$system" == macos ]]; then
  system_name=macOS
  case "$arch" in
    arm64) requirements='macOS 14 or later, on a Mac with Apple silicon. The runtime uses the GPU through Metal.' ;;
    x86_64) requirements='macOS 14 or later, on a Mac with an Intel processor. The runtime uses the CPU only, and it is slower than on Apple silicon. It needs a processor with AVX2, which every Mac that runs macOS 14 has.' ;;
  esac
  builds_text='This file has one build, `sayso-cleanup`.'
  hello='(echo '"'"'{"v":1,"id":"1","type":"hello"}'"'"'; sleep 1) | ./sayso-cleanup'
else
  system_name=Linux
  requirements='glibc 2.35 or newer, with `libstdc++` and `libgcc_s` (every Linux system that runs C++ programs has them). The runtime uses the CPU only.'
  if [[ "$arch" == x86_64 ]]; then
    builds_text='This file has three builds, for three levels of the processor: `sayso-cleanup` (AVX2, FMA, F16C, and BMI2), `sayso-cleanup-avx` (AVX and SSE 4.2), and `sayso-cleanup-baseline` (x86-64 only). A processor that lacks the instructions of a build stops it with an illegal instruction. The SDK starts the best build that the processor runs.'
  else
    builds_text='This file has one build, `sayso-cleanup`.'
  fi
  hello='(echo '"'"'{"v":1,"id":"1","type":"hello"}'"'"'; sleep 1) | ./sayso-cleanup'
fi
readme=$(cat packaging/cleanup/README.md.in)
readme=${readme//@VERSION@/$version}
readme=${readme//@SYSTEM@/$system_name}
readme=${readme//@REQUIREMENTS@/$requirements}
readme=${readme//@BUILDS@/$builds_text}
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
echo "==> cleanup runtime file:"
/bin/ls -lh "$out/$file" "$out/$file.sha256"
