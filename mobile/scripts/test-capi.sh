#!/usr/bin/env bash
# Builds the core as a host C archive, links the C tests against it, and
# runs them: the ABI end to end, on_touch timing through cgo, and calls
# racing engine destroy.
set -euo pipefail
cd "$(dirname "$0")/.."
out=build/host
mkdir -p "$out"
go build -buildmode=c-archive -o "$out/libsayso_core.a" ./core/capi
frameworks=()
if [[ "$(go env GOOS)" == darwin ]]; then
  frameworks=(-framework CoreFoundation -framework Security)
fi
for t in smoke race; do
  cc -O2 -Wall -Wextra -Werror -I core/capi "core/capi/testdata/$t.c" "$out/libsayso_core.a" \
    ${frameworks[@]+"${frameworks[@]}"} -lpthread -o "$out/$t"
done
journal="$(mktemp -d)/journal"
"$out/smoke" models/en-US.sslm "$journal"
"$out/race" models/en-US.sslm
