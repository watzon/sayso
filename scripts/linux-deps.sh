#!/usr/bin/env bash
# Install the packages that a Linux build of Sayso needs (Debian and Ubuntu).
set -euo pipefail
# No sudo for root, for example in a container.
sudo=sudo; [[ "$(id -u)" == 0 ]] && sudo=
$sudo apt-get update
$sudo apt-get install -y --no-install-recommends \
  build-essential clang pkg-config curl cmake patch \
  libasound2-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-dev libx11-xcb-dev libxcb1-dev libxcb-xkb-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev \
  libvulkan-dev spirv-headers libfontconfig-dev libfreetype-dev libssl-dev libdbus-1-dev libudev-dev libzstd-dev
# ggml's Vulkan backend compiles its shaders with glslc. Ubuntu 22.04 has no glslc package, and its Vulkan
# headers are too old for ggml: scripts/linux-vulkan-deps.sh builds both there.
if apt-cache show glslc >/dev/null 2>&1; then
  $sudo apt-get install -y --no-install-recommends glslc
else
  echo "No glslc package: also run scripts/linux-vulkan-deps.sh" >&2
fi
