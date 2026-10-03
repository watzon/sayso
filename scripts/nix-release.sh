#!/usr/bin/env bash
# Write packaging/nix/release.json (the version and the hashes of the Nix
# package) from the two Linux tarballs of a release. Run it after a release
# and commit the file, so `nix run github:watzon/sayso` gets that release.
#
#   scripts/nix-release.sh <folder with the two tarballs>
#   scripts/nix-release.sh v0.4.0        download the tarballs of that release
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
[[ $# -eq 1 ]] || { echo "usage: $0 <folder with the two tarballs> | <tag>" >&2; exit 2; }
dist=$1
if [[ ! -d "$dist" ]]; then
  tag=$dist
  dist=$(mktemp -d)
  trap 'rm -rf "$dist"' EXIT
  gh release download "$tag" --repo watzon/sayso --dir "$dist" --pattern 'sayso-*-linux-*.tar.gz'
fi

shopt -s nullglob
tarballs=("$dist"/sayso-*-linux-x86_64.tar.gz)
[[ ${#tarballs[@]} -eq 1 ]] || { echo "$dist must have one sayso-<version>-linux-x86_64.tar.gz" >&2; exit 1; }
version=$(basename "${tarballs[0]}" -linux-x86_64.tar.gz); version=${version#sayso-}
# The hash as Nix wants it: sha256- and the digest in base64.
hash() {
  local file="$dist/sayso-$version-linux-$1.tar.gz"
  [[ -f "$file" ]] || { echo "The file $file is missing" >&2; exit 1; }
  echo "sha256-$(openssl dgst -sha256 -binary "$file" | openssl base64 -A)"
}
cat > "$root/packaging/nix/release.json" <<JSON
{
  "version": "$version",
  "hashes": {
    "x86_64": "$(hash x86_64)",
    "aarch64": "$(hash aarch64)"
  }
}
JSON
echo "==> packaging/nix/release.json for Sayso $version"
