#!/usr/bin/env bash
# Install the packages that a Linux build of Sayso needs (Debian and Ubuntu).
set -euo pipefail
# No sudo for root, for example in a container.
sudo=sudo; [[ "$(id -u)" == 0 ]] && sudo=
$sudo apt-get update
$sudo apt-get install -y --no-install-recommends \
  build-essential clang pkg-config curl \
  libasound2-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-dev libx11-xcb-dev libxcb1-dev libxcb-xkb-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-render0-dev \
  libvulkan-dev libfontconfig-dev libfreetype-dev libssl-dev libdbus-1-dev libudev-dev libzstd-dev
