#!/usr/bin/env bash
# Build the Vulkan headers and glslc that ggml's Vulkan backend needs, into one folder:
#   scripts/linux-vulkan-deps.sh <folder>
#   export VULKAN_SDK=<folder>   # CMake's FindVulkan and ggml look there first
#
# For Ubuntu 22.04, which has no glslc package and Vulkan headers (1.3.204) too old for ggml. Newer
# releases have both in apt (scripts/linux-deps.sh). It needs the packages of scripts/linux-deps.sh, git
# and python3. glslc runs on the architecture and glibc it was built on.
set -euo pipefail
[[ $# -eq 1 ]] || { echo "usage: $0 <folder>" >&2; exit 2; }
vulkan_headers=v1.4.363
shaderc=v2026.4 # its DEPS file pins the glslang, SPIRV-Tools and SPIRV-Headers commits
prefix=$(realpath -m "$1")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git clone --quiet --depth 1 --branch "$vulkan_headers" https://github.com/KhronosGroup/Vulkan-Headers "$work/vulkan-headers"
cmake -S "$work/vulkan-headers" -B "$work/vulkan-headers/build" -DCMAKE_INSTALL_PREFIX="$prefix"
cmake --install "$work/vulkan-headers/build"

git clone --quiet --depth 1 --branch "$shaderc" https://github.com/google/shaderc "$work/shaderc"
GIT_SYNC_DEPS_QUIET=1 python3 "$work/shaderc/utils/git-sync-deps"
cmake -S "$work/shaderc" -B "$work/shaderc/build" -DCMAKE_BUILD_TYPE=Release \
  -DSHADERC_SKIP_TESTS=ON -DSHADERC_SKIP_EXAMPLES=ON -DSHADERC_SKIP_COPYRIGHT_CHECK=ON
cmake --build "$work/shaderc/build" --target glslc_exe -j "$(nproc)"
install -D -s -m 755 "$work/shaderc/build/glslc/glslc" "$prefix/bin/glslc"
