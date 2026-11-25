#!/usr/bin/env bash
# Build libparakeet.so, the parakeet.cpp library that the Linux engine loads for
# Fluid Speech Mini (internal/localengine/parakeet_linux.go), into a folder:
#   scripts/build-parakeet.sh <folder>
#
# parakeet.cpp has no prebuilt library. The sources are two pinned GitHub
# archives, checked by SHA-256: parakeet.cpp, and the ggml commit that is its
# submodule (an archive leaves submodules out), with parakeet.cpp's ggml
# patches. ggml is linked in statically with its Vulkan backend, so the library
# needs only libvulkan.so.1 and libstdc++ at run time. It needs the packages of
# scripts/linux-deps.sh, and on Ubuntu 22.04 VULKAN_SDK from
# scripts/linux-vulkan-deps.sh. A new pin also updates
# packaging/licenses/parakeet.cpp.txt and ggml.txt.
set -euo pipefail
[[ $# -eq 1 ]] || { echo "usage: $0 <folder>" >&2; exit 2; }
[[ "$(uname -s)" == Linux ]] || { echo "build-parakeet.sh runs on Linux" >&2; exit 1; }
parakeet_url=https://github.com/mudler/parakeet.cpp/archive/5c9b0fbbd8e611d88e47d7e1ebae9bca9b447e8f.tar.gz
parakeet_sha256=a6544898a6721a9ab54a8af37362cb9a79a8b50feaf16a597091790a8874debe
ggml_url=https://github.com/ggml-org/ggml/archive/e705c5fed490514458bdd2eaddc43bd098fcce9b.tar.gz
ggml_sha256=77eacb7cc20c2e47ffbaa5a69fe6f9fd9da0bea2a6431b7fae1a8283cb2c4e43
out=$(realpath -m "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() { # <url> <sha256> <folder>
  curl -fsSL --retry 3 -o "$work/archive.tar.gz" "$1"
  echo "$2  $work/archive.tar.gz" | sha256sum -c --quiet - || { echo "$1 does not have the pinned SHA-256" >&2; exit 1; }
  mkdir -p "$3"
  tar -xzf "$work/archive.tar.gz" -C "$3" --strip-components=1
  rm "$work/archive.tar.gz"
}
src="$work/parakeet.cpp"
fetch "$parakeet_url" "$parakeet_sha256" "$src"
fetch "$ggml_url" "$ggml_sha256" "$src/third_party/ggml"
# parakeet.cpp's CMake applies the patches only to a git checkout.
for p in "$src"/third_party/ggml-patches/*.patch; do
  patch -p1 --forward --silent -d "$src/third_party/ggml" -i "$p"
done

# GGML_NATIVE off: the library runs on other CPUs. With it off, ggml still turns
# on SSE 4.2, AVX, AVX2, BMI2, FMA, and F16C, and a processor without them
# stops with SIGILL when the CPU backend starts (issue #37). The model runs on
# the GPU, and the CPU backend takes only the few operations that Vulkan lacks,
# so it is built for baseline x86-64. The options do nothing on aarch64.
cmake -S "$src" -B "$work/build" -DCMAKE_BUILD_TYPE=Release \
  -DPARAKEET_SHARED=ON -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DPARAKEET_GGML_VULKAN=ON -DPARAKEET_WITH_CED=OFF -DPARAKEET_WITH_VOICEDETECT=OFF \
  -DPARAKEET_BUILD_CLI=OFF -DPARAKEET_BUILD_SERVER=OFF -DPARAKEET_BUILD_TESTS=OFF \
  -DGGML_NATIVE=OFF -DGGML_SSE42=OFF -DGGML_AVX=OFF -DGGML_AVX2=OFF -DGGML_BMI2=OFF \
  -DGGML_FMA=OFF -DGGML_F16C=OFF -DGGML_OPENMP=OFF
cmake --build "$work/build" --target parakeet -j "$(nproc)"

lib=$(find "$work/build" -name libparakeet.so -type f | head -1)
[[ -n "$lib" ]] || { echo "the build made no libparakeet.so" >&2; exit 1; }
# The engine reads the C API and ggml's device list from this library.
exports=$(nm -D --defined-only "$lib")
for sym in parakeet_capi_load parakeet_capi_transcribe_pcm ggml_backend_dev_count ggml_backend_dev_type; do
  grep -qw "$sym" <<< "$exports" || { echo "libparakeet.so does not export $sym" >&2; exit 1; }
done
mkdir -p "$out"
install -m 755 -s "$lib" "$out/libparakeet.so"
echo "==> $out/libparakeet.so"
