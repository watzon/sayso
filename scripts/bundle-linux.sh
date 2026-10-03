#!/usr/bin/env bash
# Build the Linux release folder and its tarball:
#   build/sayso-<version>-linux-<arch>/        bin/, share/, lib/udev/, install.sh
#   build/sayso-<version>-linux-<arch>.tar.gz  and its .sha256
#
#   scripts/bundle-linux.sh            release build
#   scripts/bundle-linux.sh --debug    debug build (faster)
#
# Run it on Linux. The tarball works on the architecture it was built on.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Linux ]] || { echo "bundle-linux.sh runs on Linux" >&2; exit 1; }
profile=release; cargo_flag=--release
[[ "${1:-}" == "--debug" ]] && { profile=debug; cargo_flag=; }
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
arch=$(uname -m)
target_dir=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')

echo "==> app and engine (cargo, $profile)"
cargo build -p sayso-app -p sayso-engine $cargo_flag

name="sayso-$version-linux-$arch"
out="${SAYSO_BUNDLE_DIR:-build}/$name"
rm -rf "$out"
mkdir -p "$out/bin" "$out/share/applications" "$out/share/metainfo" "$out/lib/udev/rules.d"
install -m 755 "$target_dir/$profile/sayso" "$out/bin/sayso"
install -m 755 "$target_dir/$profile/sayso-engine" "$out/bin/sayso-engine"
cp packaging/linux/dev.sayso.Sayso.desktop "$out/share/applications/"
cp packaging/linux/dev.sayso.Sayso.metainfo.xml "$out/share/metainfo/"
cp packaging/linux/70-sayso-uinput.rules "$out/lib/udev/rules.d/"
install -m 755 packaging/linux/install.sh "$out/install.sh"
cp LICENSE "$out/"
# The macOS icon set has the sizes the hicolor theme uses.
for size in 16 32 128 256 512; do
  mkdir -p "$out/share/icons/hicolor/${size}x${size}/apps"
  cp "assets/app/AppIcon.iconset/icon_${size}x${size}.png" "$out/share/icons/hicolor/${size}x${size}/apps/dev.sayso.Sayso.png"
done
mkdir -p "$out/share/icons/hicolor/64x64/apps"
cp assets/app/icon-64.png "$out/share/icons/hicolor/64x64/apps/dev.sayso.Sayso.png"

tar -C "$(dirname "$out")" -czf "$out.tar.gz" "$name"
(cd "$(dirname "$out")" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "==> $out.tar.gz"
