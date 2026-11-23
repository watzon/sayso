#!/usr/bin/env bash
# Builds the core as a host C archive, links the C smoke test against it,
# and runs it: the ABI end to end, plus on_touch timing through cgo.
set -euo pipefail
cd "$(dirname "$0")/.."
out=build/host
mkdir -p "$out"
go build -buildmode=c-archive -o "$out/libsayso_core.a" ./core/capi
frameworks=()
if [[ "$(go env GOOS)" == darwin ]]; then
  frameworks=(-framework CoreFoundation -framework Security)
fi
cc -O2 -Wall -Wextra -Werror -I core/capi core/capi/testdata/smoke.c "$out/libsayso_core.a" \
  ${frameworks[@]+"${frameworks[@]}"} -lpthread -o "$out/smoke"
journal="$(mktemp -d)/journal"
"$out/smoke" models/en-US.sslm "$journal"
