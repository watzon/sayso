#!/usr/bin/env bash
# Enforce the dependency rules in docs/plan.md §4.
set -euo pipefail
cd "$(dirname "$0")/.."
meta=$(cargo metadata --format-version 1 --no-deps)
deps_of() {
  echo "$meta" | python3 -c "import json,sys; m=json.load(sys.stdin); p=[x for x in m['packages'] if x['name']=='$1'][0]; print('\n'.join(d['name'] for d in p['dependencies'] if d.get('kind') is None))"
}
fail=0
check() { # crate, forbidden-regex, reason
  if deps_of "$1" | grep -Eq "$2"; then
    echo "FAIL: $1 depends on $(deps_of "$1" | grep -E "$2" | tr '\n' ' ') ($3)"; fail=1
  fi
}
check sayso-i18n '^(sayso-|gpui)' "the catalogs are data: no Sayso crates, no GPUI"
check sayso-core '^(sayso-(platform|engine|store|enhance|transcribe|ui|update|app)|gpui)' "core imports no Sayso crate except sayso-i18n, and no GPUI"
check sayso-platform '^(sayso-(platform-|engine|store|enhance|ui|app)|gpui)' "traits only"
for c in sayso-store sayso-enhance sayso-engine-client sayso-cleanup-client sayso-ui; do
  check "$c" '^sayso-platform-' "only sayso-app may use a platform implementation"
done
check sayso-ui '^sayso-(store|enhance|engine-client)' "UI gets data through the app"
check sayso-ink '^(sayso-|gpui)' "sayso-ink is MIT and published: it needs no Sayso crate and no GPUI"
check sayso-ui '^sayso-([^i]|i[^n])' "sayso-ui is MIT and published: its only Sayso crate is sayso-ink"
# The MIT crates (docs/adr/0002-mit-sdk-gpl-app.md) are published. They must say
# so, and a dependency on the other one needs the workspace version, because
# cargo publish needs a version for a path dependency.
if ! echo "$meta" | python3 -c "
import json, sys
m = json.load(sys.stdin)
pk = {p['name']: p for p in m['packages']}
bad = []
for name in ('sayso-ink', 'sayso-ui'):
    p = pk[name]
    if p['license'] != 'MIT' or p['publish'] == []:
        bad.append(name + ' must have license MIT and be published')
    for d in p['dependencies']:
        if d['name'] in ('sayso-ink',) and d['req'] != '^' + p['version']:
            bad.append(name + ' needs sayso-ink ' + p['version'] + ' but has ' + d['req'] + ' (change the version in Cargo.toml [workspace.dependencies])')
print('\\n'.join(bad))
sys.exit(1 if bad else 0)
" > /tmp/check-deps-mit.$$; then
  sed 's/^/FAIL: /' /tmp/check-deps-mit.$$; fail=1
fi
rm -f /tmp/check-deps-mit.$$
check sayso-platform-common '^(sayso-(platform-(macos|linux|windows)|engine|store|enhance|ui|app)|gpui)' "portable platform code only"
check sayso-update '^(sayso-(platform|engine|store|enhance|transcribe|ui|app)|gpui)' "the updater has no UI and no platform code"
check sayso-engine '^(sayso-(platform|store|enhance|ui|app)|gpui)' "the engine is a separate process"
check sayso-engine-conformance '^(sayso-|gpui)' "MIT like the spec: no GPL crate of Sayso, and it tests an engine only through its process"
check sayso-cleanup-client '^(sayso-(platform|engine|store|enhance|transcribe|ui|update|app)|gpui)' "the cleanup client needs only sayso-core"
# workspace-hack (cargo hakari, .config/hakari.toml) must match the
# dependencies. After you add, remove, or change a dependency, run:
#   cargo hakari generate && cargo hakari manage-deps -y
if command -v cargo-hakari >/dev/null; then
  if ! cargo hakari generate --diff >/dev/null 2>&1 \
    || ! cargo hakari manage-deps --dry-run 2>&1 | grep -q 'no operations to perform'; then
    echo "FAIL: workspace-hack is out of date (run: cargo hakari generate && cargo hakari manage-deps -y)"; fail=1
  fi
else
  echo "warning: workspace-hack not checked, because cargo-hakari is not installed (cargo install cargo-hakari --locked)"
fi
[ $fail -eq 0 ] && echo "dependency rules: ok"
exit $fail
