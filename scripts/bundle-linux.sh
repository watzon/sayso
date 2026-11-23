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
profile=release
[[ "${1:-}" == "--debug" ]] && profile=debug
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
arch=$(uname -m)
case "$arch" in
  x86_64) goarch=amd64 ;;
  aarch64) goarch=arm64 ;;
  *) echo "no llama.cpp build for $arch" >&2; exit 1 ;;
esac
export CGO_ENABLED=0
# The app reports the version of the tarball, which the updater compares
# with the newest release.
version_flag="-X github.com/watzon/sayso/internal/update.Version=$version"
if [[ "$profile" == release ]]; then ldflags=(-ldflags="-s -w $version_flag" -tags mygo_noinspector); else ldflags=(-ldflags="$version_flag"); fi

echo "==> app, engine, and cleanup runtime (go, $profile)"
mkdir -p build/go
go build -buildvcs=false ${ldflags[@]+"${ldflags[@]}"} -o build/go/sayso ./cmd/sayso
go build -buildvcs=false ${ldflags[@]+"${ldflags[@]}"} -o build/go/sayso-engine ./cmd/sayso-engine
# The cleanup runtime loads llama.cpp with yzma, through the system's
# libffi.so.8 (ffi_no_embed: Linux has no embedded copy anyway).
go build -buildvcs=false -tags ffi_no_embed -ldflags="-s -w" -o build/go/sayso-cleanup ./cmd/sayso-cleanup

echo "==> llama.cpp libraries (pinned)"
llama_libs=$(go run ./scripts/llama-libs fetch -os linux -arch "$goarch" -out build/llama-libs)

name="sayso-$version-linux-$arch"
out="${SAYSO_BUNDLE_DIR:-build}/$name"
rm -rf "$out"
mkdir -p "$out/bin" "$out/share/applications" "$out/share/metainfo" "$out/lib/udev/rules.d"
install -m 755 build/go/sayso "$out/bin/sayso"
install -m 755 build/go/sayso-engine "$out/bin/sayso-engine"
# The engine loads sherpa-onnx, whisper.cpp, ggml, and parakeet at run time
# (purego): SAYSO_NATIVE_LIBS_DIR holds the .so files that go beside it.
if [[ -n "${SAYSO_NATIVE_LIBS_DIR:-}" ]]; then
  install -m 755 "$SAYSO_NATIVE_LIBS_DIR"/*.so* "$out/bin/"
fi
# The cleanup runtime and its llama.cpp libraries live in bin/llama, apart from
# the ggml of the engine in bin/ (internal/llamalib). The libraries find each
# other and libgomp through RUNPATH $ORIGIN, and need their soname links. On
# x86_64 ggml loads the CPU library that the processor runs (issue #37).
mkdir -p "$out/bin/llama"
install -m 755 build/go/sayso-cleanup "$out/bin/llama/sayso-cleanup"
cp -a "$llama_libs"/lib* "$out/bin/llama/"
# libgomp (OpenMP of GCC), which ggml needs, from this build system: so a
# system without it runs the runtime too.
gomp=$(PATH="$PATH:/sbin:/usr/sbin" ldconfig -p | awk '$1 == "libgomp.so.1" && $NF ~ /^\// { print $NF; exit }')
[[ -n "$gomp" ]] || { echo "This system has no libgomp.so.1 (apt install libgomp1)" >&2; exit 1; }
install -m 755 "$(realpath "$gomp")" "$out/bin/llama/libgomp.so.1"
cp packaging/linux/dev.sayso.Sayso.desktop "$out/share/applications/"
cp packaging/linux/dev.sayso.Sayso.metainfo.xml "$out/share/metainfo/"
cp packaging/linux/70-sayso-uinput.rules "$out/lib/udev/rules.d/"
install -m 755 packaging/linux/install.sh "$out/install.sh"
cp LICENSE "$out/"
# The licenses of the libraries that the engine loads: parakeet.cpp and ggml (MIT) and sherpa-onnx (Apache-2.0) and ONNX Runtime (MIT).
# llama.cpp has the same license text as ggml.txt; libgomp is GCC's runtime
# (libgomp.txt).
install -D -m 644 -t "$out/share/licenses/sayso" packaging/licenses/*.txt
# llama.cpp's license is ggml's MIT text (ggml.txt, byte for byte); not every
# llama.cpp archive has its LICENSE (linux-arm64 has none).
install -m 644 packaging/licenses/ggml.txt "$out/share/licenses/sayso/llama.cpp.txt"
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
