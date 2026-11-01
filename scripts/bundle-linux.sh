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
# The bridge runs outside the Flatpak sandbox, on any Linux of the user, so it
# has no library of the system. It is linked statically, and always a release
# build. Only this crate gets the flag, because of --target.
echo "==> bridge (cargo, release, static)"
bridge_target="$arch-unknown-linux-gnu"
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p sayso-bridge --target "$bridge_target"
bridge_bin="$target_dir/$bridge_target/release/sayso-bridge"

name="sayso-$version-linux-$arch"
out="${SAYSO_BUNDLE_DIR:-build}/$name"
rm -rf "$out"
mkdir -p "$out/bin" "$out/share/applications" "$out/share/metainfo" "$out/lib/udev/rules.d"
install -m 755 "$target_dir/$profile/sayso" "$out/bin/sayso"
install -m 755 "$target_dir/$profile/sayso-engine" "$out/bin/sayso-engine"
# The cleanup runtime is its own Cargo workspace (it builds llama.cpp, which is
# slow in a debug build), so it is always a release build. The app finds it
# next to itself, like the engine. On x86_64 there are three builds, one for
# each instruction set (scripts/build-cleanup.sh).
scripts/build-cleanup.sh "$out/bin"
install -m 755 "$bridge_bin" "$out/bin/sayso-bridge"
cp packaging/linux/dev.sayso.Sayso.desktop "$out/share/applications/"
cp packaging/linux/dev.sayso.Sayso.metainfo.xml "$out/share/metainfo/"
cp packaging/linux/70-sayso-uinput.rules "$out/lib/udev/rules.d/"
install -m 755 packaging/linux/install.sh "$out/install.sh"
cp LICENSE "$out/"
# The licenses of the libraries in the engine: parakeet.cpp and ggml (MIT), which
# it links statically, and sherpa-onnx (Apache-2.0) and ONNX Runtime (MIT).
# The cleanup runtime links llama.cpp statically, and its LICENSE has the same
# text as ggml.txt.
install -D -m 644 -t "$out/share/licenses/sayso" crates/sayso-engine/licenses/*.txt
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
