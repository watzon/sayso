#!/usr/bin/env bash
# Enforce the rules of the Rust workspace, which holds only the MIT SDK crates.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 - "$(cargo metadata --format-version 1 --no-deps)" <<'PY'
import json, sys
m = json.loads(sys.argv[1])
pk = {p['name']: p for p in m['packages']}
sdk = ['sayso-protocol', 'sayso-engine-sdk', 'sayso-speech', 'sayso-capture',
       'sayso-cleanup-models', 'sayso-cleanup-client', 'sayso-sdk', 'sayso-node']
allowed = {
    'sayso-protocol': set(),
    'sayso-engine-sdk': {'sayso-protocol'},
    'sayso-speech': {'sayso-protocol'},
    'sayso-capture': {'sayso-protocol', 'sayso-speech'},
    'sayso-cleanup-models': set(),
    'sayso-cleanup-client': {'sayso-cleanup-models'},
    'sayso-sdk': {'sayso-protocol', 'sayso-engine-sdk', 'sayso-speech', 'sayso-capture', 'sayso-cleanup-models', 'sayso-cleanup-client'},
    'sayso-node': {'sayso-sdk', 'sayso-protocol', 'sayso-speech', 'sayso-capture'},
}
bad = []
if sorted(pk) != sorted(sdk):
    bad.append('the workspace must hold exactly: ' + ', '.join(sdk) + ' (has ' + ', '.join(sorted(pk)) + ')')
for name in sdk:
    p = pk.get(name)
    if not p:
        continue
    # sayso-node is published to npm, not to crates.io.
    if p['license'] != 'MIT' or (p['publish'] == [] and name != 'sayso-node'):
        bad.append(name + ' must have license MIT and be published')
    for d in p['dependencies']:
        if d['name'].startswith('sayso-') or d['name'] in ('workspace-hack',) or d['name'].startswith('gpui'):
            if d['name'] not in allowed[name]:
                bad.append(name + ' must not depend on ' + d['name'])
            elif d['name'] in pk and d['req'] != '^' + p['version'] and d.get('kind') is None:
                bad.append(name + ' needs ' + d['name'] + ' ' + p['version'] + ' but has ' + d['req'])
print('\n'.join('FAIL: ' + b for b in bad) if bad else 'dependency rules: ok')
sys.exit(1 if bad else 0)
PY
