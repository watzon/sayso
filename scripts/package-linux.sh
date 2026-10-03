#!/usr/bin/env bash
# Make the Linux packages from the release tarball of scripts/bundle-linux.sh.
# Nothing is compiled here: each format gets the same two binaries.
#
#   scripts/package-linux.sh <tarball> [deb] [rpm] [appimage]    default: all
#
# The packages go next to the tarball:
#   sayso-<version>-linux-<arch>.deb, .rpm, .AppImage
#
# Run it on Linux, on the architecture of the tarball. It needs curl, file,
# and binutils. The tools (nfpm, appimagetool, the AppImage runtime) are
# downloaded to build/tools, or to $SAYSO_TOOLS_DIR, and checked against the
# hashes below.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
[[ "$(uname -s)" == Linux ]] || { echo "package-linux.sh runs on Linux" >&2; exit 1; }
[[ $# -ge 1 && -f "$1" ]] || { echo "usage: $0 <tarball> [deb] [rpm] [appimage]" >&2; exit 2; }
tarball=$(realpath "$1"); shift
formats=("$@"); [[ ${#formats[@]} -gt 0 ]] || formats=(deb rpm appimage)

name=$(basename "$tarball" .tar.gz)
if [[ ! "$name" =~ ^sayso-(.+)-linux-(x86_64|aarch64)$ ]]; then
  echo "The tarball name must be sayso-<version>-linux-<arch>.tar.gz: $tarball" >&2; exit 2
fi
version=${BASH_REMATCH[1]}; arch=${BASH_REMATCH[2]}
[[ "$arch" == "$(uname -m)" ]] || { echo "The tarball is for $arch, and this system is $(uname -m)" >&2; exit 1; }
out=$(dirname "$tarball")

# name, URL, and SHA-256 of each tool, for this architecture.
case "$arch" in
  x86_64)
    pkg_arch=amd64
    nfpm=(nfpm_2.47.0_Linux_x86_64.tar.gz https://github.com/goreleaser/nfpm/releases/download/v2.47.0/nfpm_2.47.0_Linux_x86_64.tar.gz 0660ca602b2d2d2ae4781a06c692b3eeb9d437ffea05b831d76e41f4a3188783)
    appimagetool=(appimagetool-x86_64.AppImage https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0)
    runtime=(runtime-x86_64 https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d)
    ;;
  aarch64)
    pkg_arch=arm64
    nfpm=(nfpm_2.47.0_Linux_arm64.tar.gz https://github.com/goreleaser/nfpm/releases/download/v2.47.0/nfpm_2.47.0_Linux_arm64.tar.gz 1c0f5f2999b9a974bfb04fdb0cc3306096de530ac5dbb25d739cc5f5219c919c)
    appimagetool=(appimagetool-aarch64.AppImage https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-aarch64.AppImage f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158)
    runtime=(runtime-aarch64 https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-aarch64 00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444)
    ;;
esac

tools=${SAYSO_TOOLS_DIR:-$root/build/tools}
mkdir -p "$tools"
# fetch <name> <url> <sha256>: download the tool unless the file with that hash is there.
fetch() {
  local file="$tools/$1"
  if ! echo "$3  $file" | sha256sum --check --status 2>/dev/null; then
    echo "==> download $1"
    curl --fail --silent --show-error --location --retry 3 -o "$file" "$2"
    echo "$3  $file" | sha256sum --check --status || { echo "The SHA-256 of $1 is wrong" >&2; rm -f "$file"; exit 1; }
  fi
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -C "$work" -xzf "$tarball"
stage="$work/$name"

# The packages declare the glibc and libstdc++ versions in nfpm.yaml. Stop
# when a binary needs more, for example after a change of the build system.
newest() { objdump -T "$stage/bin/sayso" "$stage/bin/sayso-engine" | grep -oE "$1_[0-9.]+" | cut -d_ -f2 | sort -Vu | tail -1; }
declared_glibc=$(sed -n 's/.*libc6 (>= \([0-9.]*\)).*/\1/p' "$root/packaging/linux/nfpm.yaml")
declared_glibcxx=$(sed -n 's/.*(GLIBCXX_\([0-9.]*\)).*/\1/p' "$root/packaging/linux/nfpm.yaml")
check_floor() {
  [[ -n "$3" && "$(printf '%s\n%s\n' "$2" "$3" | sort -V | tail -1)" == "$3" ]] || {
    echo "The binaries need $1 $2, but packaging/linux/nfpm.yaml declares ${3:-nothing}." >&2; exit 1
  }
}
check_floor glibc "$(newest GLIBC)" "$declared_glibc"
check_floor GLIBCXX "$(newest GLIBCXX)" "$declared_glibcxx"

package_nfpm() {
  fetch "${nfpm[@]}"
  tar -C "$tools" -xzf "$tools/${nfpm[0]}" nfpm
  cp "$root/packaging/linux/postinstall.sh" "$stage/"
  (cd "$stage" && SAYSO_VERSION=$version SAYSO_PKG_ARCH=$pkg_arch \
    "$tools/nfpm" package --config "$root/packaging/linux/nfpm.yaml" --packager "$1" --target "$out/$name.$1")
}

# The AppImage holds only the two binaries, like the other packages, and takes
# the libraries from the system. A copy of libxkbcommon from the build system
# cannot read the keyboard data of a newer system.
package_appimage() {
  fetch "${appimagetool[@]}"
  fetch "${runtime[@]}"
  chmod +x "$tools/${appimagetool[0]}"
  local appdir="$work/AppDir"
  mkdir -p "$appdir/usr"
  cp -R "$stage/bin" "$stage/share" "$appdir/usr/"
  # AppRun is a link, so the app finds the engine next to itself.
  ln -s usr/bin/sayso "$appdir/AppRun"
  ln -s usr/share/applications/dev.sayso.Sayso.desktop "$appdir/dev.sayso.Sayso.desktop"
  ln -s usr/share/icons/hicolor/256x256/apps/dev.sayso.Sayso.png "$appdir/dev.sayso.Sayso.png"
  ln -s dev.sayso.Sayso.png "$appdir/.DirIcon"
  # No FUSE in a container, so the tool unpacks itself.
  APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$arch VERSION=$version \
    "$tools/${appimagetool[0]}" --no-appstream --runtime-file "$tools/${runtime[0]}" "$appdir" "$out/$name.AppImage"
  chmod +x "$out/$name.AppImage"
}

for format in "${formats[@]}"; do
  echo "==> $format"
  case "$format" in
    deb|rpm) package_nfpm "$format" ;;
    appimage) package_appimage ;;
    *) echo "unknown format: $format (deb, rpm, appimage)" >&2; exit 2 ;;
  esac
done
ls -l "$out"/"$name".*
