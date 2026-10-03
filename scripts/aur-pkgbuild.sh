#!/usr/bin/env bash
# Make the AUR files of sayso-bin (PKGBUILD and .SRCINFO) from the two Linux
# release tarballs. The AUR package downloads the tarballs from the GitHub
# release, so the hashes must be those of the files of the release.
#
#   scripts/aur-pkgbuild.sh <folder with the two tarballs> <output folder>
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
[[ $# -eq 2 && -d "$1" ]] || { echo "usage: $0 <folder with the two tarballs> <output folder>" >&2; exit 2; }
dist=$1; out=$2

shopt -s nullglob
tarballs=("$dist"/sayso-*-linux-x86_64.tar.gz)
[[ ${#tarballs[@]} -eq 1 ]] || { echo "$dist must have one sayso-<version>-linux-x86_64.tar.gz" >&2; exit 1; }
version=$(basename "${tarballs[0]}" -linux-x86_64.tar.gz); version=${version#sayso-}
# pkgver cannot have a hyphen.
[[ "$version" =~ ^[0-9][0-9A-Za-z.+_]*$ ]] || { echo "The AUR cannot take the version $version" >&2; exit 1; }
hash() {
  local file="$dist/sayso-$version-linux-$1.tar.gz"
  [[ -f "$file" ]] || { echo "The file $file is missing" >&2; exit 1; }
  if command -v sha256sum >/dev/null; then sha256sum "$file" | cut -d' ' -f1; else shasum -a 256 "$file" | cut -d' ' -f1; fi
}
x86_64=$(hash x86_64); aarch64=$(hash aarch64)

mkdir -p "$out"
sed -e "s/@VERSION@/$version/" -e "s/@SHA256_X86_64@/$x86_64/" -e "s/@SHA256_AARCH64@/$aarch64/" \
  "$root/packaging/linux/aur/PKGBUILD.in" | grep -v -e '^# scripts/aur-pkgbuild.sh' -e '^# not the PKGBUILD' -e '^#$' > "$out/PKGBUILD"

# .SRCINFO, as `makepkg --printsrcinfo` writes it. The values come from the
# PKGBUILD, so the two files cannot differ.
value() { sed -n "s/^$1=\(.*\)$/\1/p" "$out/PKGBUILD" | head -1 | tr -d "\"'()"; }
list() { awk -v key="$1" '$0 ~ "^"key"=\\(" {on=1} on {print} on && /\)$/ {exit}' "$out/PKGBUILD" | sed -e "s/^$1=(//" -e 's/)$//' | grep -oE "'[^']*'" | tr -d "'"; }
base=https://github.com/watzon/sayso/releases/download/v$version
{
  echo "pkgbase = $(value pkgname)"
  echo "	pkgdesc = $(value pkgdesc)"
  echo "	pkgver = $version"
  echo "	pkgrel = $(value pkgrel)"
  echo "	url = $(value url)"
  list arch | sed 's/^/	arch = /'
  list license | sed 's/^/	license = /'
  list depends | sed 's/^/	depends = /'
  list optdepends | sed 's/^/	optdepends = /'
  list provides | sed 's/^/	provides = /'
  list conflicts | sed 's/^/	conflicts = /'
  list options | sed 's/^/	options = /'
  echo "	source_x86_64 = $base/sayso-$version-linux-x86_64.tar.gz"
  echo "	sha256sums_x86_64 = $x86_64"
  echo "	source_aarch64 = $base/sayso-$version-linux-aarch64.tar.gz"
  echo "	sha256sums_aarch64 = $aarch64"
  echo
  echo "pkgname = $(value pkgname)"
} > "$out/.SRCINFO"
echo "==> $out/PKGBUILD and $out/.SRCINFO for sayso-bin $version"
