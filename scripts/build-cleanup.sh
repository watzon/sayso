#!/usr/bin/env bash
# Build the cleanup runtime (native/cleanup) and copy it to a folder:
#
#   scripts/build-cleanup.sh <folder>
#
# On x86_64 the folder gets three builds: sayso-cleanup (AVX2, FMA, F16C,
# BMI2), sayso-cleanup-avx (AVX, SSE 4.2), and sayso-cleanup-baseline (x86-64
# only). llama.cpp compiles its kernels for one instruction set, and a
# processor without it stops the runtime with SIGILL (issue #37). The app
# starts the best build that the processor runs
# (crates/sayso-cleanup-client/src/cpu.rs). Elsewhere the folder gets one
# build, sayso-cleanup.
#
# Each build has its own target folder: the ggml options are CMake cache
# entries, and one folder for all would build llama.cpp again each time.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# -eq 1 ]] || { echo "usage: $0 <folder>" >&2; exit 2; }
dest=$1
mkdir -p "$dest"
manifest=native/cleanup/Cargo.toml
target=$(cargo metadata --format-version 1 --no-deps --manifest-path "$manifest" | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')

# llama-cpp-sys-2 passes GGML_* variables to CMake. With GGML_NATIVE off, ggml
# turns on SSE 4.2, AVX, AVX2, BMI2, FMA, and F16C by default.
build() { # <file name> <target folder> [VAR=value ...]
  local name=$1 dir=$2
  shift 2
  echo "==> cleanup runtime $name (cargo, release)"
  env "$@" cargo build --release --manifest-path "$manifest" --target-dir "$dir"
  install -m 755 "$dir/release/sayso-cleanup" "$dest/$name"
}

build sayso-cleanup "$target"
if [[ "$(uname -m)" == x86_64 ]]; then
  build sayso-cleanup-avx "$target/avx" GGML_AVX2=OFF GGML_FMA=OFF GGML_F16C=OFF GGML_BMI2=OFF
  build sayso-cleanup-baseline "$target/baseline" GGML_SSE42=OFF GGML_AVX=OFF GGML_AVX2=OFF GGML_FMA=OFF GGML_F16C=OFF GGML_BMI2=OFF
fi
