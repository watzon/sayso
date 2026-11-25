#!/usr/bin/env bash
# Print the version of Sayso. Its one source is Version in
# internal/update/version.go (docs/releasing.md, step 2).
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^var Version = "\(.*\)"$/\1/p' internal/update/version.go)
[[ -n "$version" ]] || { echo "no version in internal/update/version.go" >&2; exit 1; }
echo "$version"
