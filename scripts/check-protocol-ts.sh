#!/usr/bin/env bash
# Check that the TypeScript types of sayso-protocol in the repository are what
# the Rust code writes (docs/sdk-typescript.md, "Types").
#
# It writes the files again and compares them with the files that were there.
# A changed, missing, or new file fails the check. After a change to the types
# of sayso-protocol, run this script and commit the files it writes.
set -euo pipefail
cd "$(dirname "$0")/.."

protocol=packages/sdk/src/protocol
fixture=packages/sdk/tests/types/wire.ts

before=$(mktemp -d)
trap 'rm -rf "$before"' EXIT
[ -d "$protocol" ] && cp -R "$protocol" "$before/protocol"
[ -f "$fixture" ] && cp "$fixture" "$before/wire.ts"

# Put the old files back when the writing fails, so a build error does not
# leave the folder empty.
restore() {
  rm -rf "$protocol" "$fixture"
  [ -d "$before/protocol" ] && cp -R "$before/protocol" "$protocol"
  [ -f "$before/wire.ts" ] && cp "$before/wire.ts" "$fixture"
  echo "FAIL: could not write the TypeScript types (see the error above). The old files are back."
}

# Remove the old files, so a type that left the crate leaves the folder too.
rm -rf "$protocol" "$fixture"
if ! { cargo test -q -p sayso-protocol --features ts && python3 scripts/gen-protocol-ts.py; }; then
  restore
  exit 1
fi

status=0
diff -ru "$before/protocol" "$protocol" || status=1
diff -u "$before/wire.ts" "$fixture" || status=1
if [ $status -ne 0 ]; then
  echo
  echo "FAIL: the TypeScript types in $protocol or $fixture do not match sayso-protocol."
  echo "The files are now the new ones. Commit them, or undo your change to the types."
  echo "To write them again: cargo test -p sayso-protocol --features ts && python3 scripts/gen-protocol-ts.py"
  exit 1
fi
echo "protocol types: ok"
