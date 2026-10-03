#!/usr/bin/env bash
# Point the things in the repository that name a release to a new release:
# the Nix package (packaging/nix/release.json) and the download buttons of
# README.md. Run it when the release is the latest release, then commit the
# two files.
#
#   scripts/after-release.sh v0.4.2
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# -eq 1 && "$1" =~ ^v[0-9][0-9A-Za-z.]*$ ]] || { echo "usage: $0 <tag>, for example v0.4.2" >&2; exit 2; }
tag=$1; version=${tag#v}

scripts/nix-release.sh "$tag"

# The link definitions of the README: .../releases/download/<tag>/Sayso-<version>-...
sed -E -i.bak "s#(releases/download/)v[^/]+(/[Ss]ayso-)[0-9][0-9A-Za-z.]*-#\1$tag\2$version-#" README.md
rm README.md.bak
count=$(grep -c "releases/download/$tag/" README.md || true)
[[ "$count" -gt 0 ]] || { echo "README.md has no download link for $tag" >&2; exit 1; }
echo "==> README.md: $count download links for $tag"
