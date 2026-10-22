#!/usr/bin/env bash
# Run a compiled sayso-platform-linux test binary against real Xvfb/XInput2,
# with private mode-000 event nodes. No host devices or permissions change.
# Usage: scripts/test-unreadable-input.sh target/debug/deps/sayso_platform_linux-<hash>
set -euo pipefail
if [[ ${1:-} != --namespace ]]; then
    binary=$(realpath "${1:?pass the compiled Linux platform test binary}")
    original=$(mktemp -d "${TMPDIR:?}/sayso-input-ns.XXXXXX")
    trap 'rmdir "$original"' EXIT
    xvfb-run -a -s '-screen 0 1280x720x24 -noreset' unshare --user --map-root-user --mount --propagation private \
        bash "$0" --namespace "$original" "$binary"
    exit $?
fi
shift
# The outer process removes this mountpoint after the private namespace exits.
original=$1
shift
mount --rbind /dev "$original"
mount -t tmpfs tmpfs /dev
for name in null zero full random urandom tty; do
    touch "/dev/$name"
    mount --bind "$original/$name" "/dev/$name"
done
mkdir /dev/pts /dev/shm /dev/input
mount --rbind "$original/pts" /dev/pts
mount --rbind "$original/shm" /dev/shm
ln -s pts/ptmx /dev/ptmx
for i in 0 1 2 3; do
    touch "/dev/input/event$i"
    chmod 000 "/dev/input/event$i"
done
exec setpriv --bounding-set=-all --inh-caps=-all \
    --securebits=+noroot,+noroot_locked,+no_setuid_fixup,+no_setuid_fixup_locked \
    bash -s -- "$@" <<'TEST'
set -euo pipefail
if test -r /dev/input/event0; then
    printf 'PREMISE FAILED: event0 is readable\n' >&2
    exit 1
fi
printf 'premise: mode-000 event nodes unreadable with capabilities dropped\n'
exec "$@" unreadable_input --ignored --test-threads=1 --nocapture
TEST
