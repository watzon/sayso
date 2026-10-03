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
check sayso-core '^(sayso-|gpui)' "core must not import other Sayso crates or GPUI"
check sayso-platform '^(sayso-(platform-|engine|store|enhance|ui|app)|gpui)' "traits only"
for c in sayso-store sayso-enhance sayso-engine-client sayso-ui; do
  check "$c" '^sayso-platform-' "only sayso-app may use a platform implementation"
done
check sayso-ui '^sayso-(store|enhance|engine-client)' "UI gets data through the app"
check sayso-platform-common '^(sayso-(platform-(macos|linux|windows)|engine|store|enhance|ui|app)|gpui)' "portable platform code only"
check sayso-engine '^(sayso-(platform|store|enhance|ui|app)|gpui)' "the engine is a separate process"
[ $fail -eq 0 ] && echo "dependency rules: ok"
exit $fail
