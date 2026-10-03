#!/usr/bin/env bash
# Make the Homebrew cask of Sayso (Casks/sayso.rb in watzon/homebrew-tap)
# from packaging/homebrew/sayso.rb.in and the DMG of a release. The cask
# downloads the DMG from the GitHub release, so the hash must be that of the
# file of the release.
#
#   scripts/homebrew-cask.sh <Sayso-<version>-macos-arm64.dmg> <output file>
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
[[ $# -eq 2 && -f "$1" ]] || { echo "usage: $0 <Sayso-<version>-macos-arm64.dmg> <output file>" >&2; exit 2; }
dmg=$1; out=$2

name=$(basename "$dmg")
if [[ ! "$name" =~ ^Sayso-([0-9]+\.[0-9]+\.[0-9]+)-macos-arm64\.dmg$ ]]; then
  echo "The file name must be Sayso-<version>-macos-arm64.dmg: $dmg" >&2; exit 2
fi
version=${BASH_REMATCH[1]}
if command -v sha256sum >/dev/null; then sha256=$(sha256sum "$dmg" | cut -d' ' -f1); else sha256=$(shasum -a 256 "$dmg" | cut -d' ' -f1); fi

mkdir -p "$(dirname "$out")"
sed -e "s/@VERSION@/$version/" -e "s/@SHA256@/$sha256/" "$root/packaging/homebrew/sayso.rb.in" > "$out"
echo "==> $out for Sayso $version"
