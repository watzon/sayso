#!/usr/bin/env bash
# Give each npm package of Sayso the trusted publisher release.yml of
# watzon/sayso, so the job "npm publish" of the Release workflow can publish it
# with OIDC and no token (docs/releasing.md, "npm packages").
#
#   scripts/npm-trust.sh            # add the trusted publisher to every package
#   scripts/npm-trust.sh --dry-run  # show what npm would do
#   scripts/npm-trust.sh --list     # show the trusted publishers of every package
#
# It needs npm 11.15.0 or later, a session of `npm login`, and packages that
# exist on npm. Each npm trust command, also `npm trust list`, needs two-factor
# authentication:
#
# - In a terminal, npm opens an approval page in the browser and waits. At the
#   first package, approve and select the option to skip two-factor
#   authentication for 5 minutes. The next packages then need no approval.
#   npm waits only when its input and output are a terminal, so the script
#   does not redirect or capture them.
# - Or set NPM_OTP_COMMAND to a command that prints a one-time password. The
#   script gets a new one for each package, for example:
#
#     NPM_OTP_COMMAND='op item get <npm item> --otp' scripts/npm-trust.sh
set -euo pipefail
cd "$(dirname "$0")/.."

mode=add
case "${1:-}" in
  "") ;;
  --dry-run) mode=dry-run ;;
  --list) mode=list ;;
  *) echo "usage: $0 [--dry-run | --list]" >&2; exit 2 ;;
esac

npm_version=$(npm --version)
if [[ "$(printf '%s\n%s\n' 11.15.0 "$npm_version" | sort -V | head -1)" != 11.15.0 ]]; then
  echo "npm $npm_version is too old: npm trust needs 11.15.0 or later (npm install -g npm@11)." >&2
  exit 1
fi
npm whoami >/dev/null || { echo "Log in first: npm login --auth-type=web" >&2; exit 1; }

names=()
while read -r name; do names+=("$name"); done < <(python3 scripts/npm-packages.py names)

failed=()
for name in "${names[@]}"; do
  if [[ "$mode" == list ]]; then
    args=(trust list "$name")
  else
    args=(trust github "$name" --file release.yml --repository watzon/sayso --allow-publish --yes)
    [[ "$mode" == dry-run ]] && args+=(--dry-run)
  fi
  if [[ -n "${NPM_OTP_COMMAND:-}" ]]; then
    otp=$(bash -c "$NPM_OTP_COMMAND") || { echo "NPM_OTP_COMMAND failed" >&2; exit 1; }
    args+=(--otp "$otp")
  fi
  echo "==> $name"
  npm "${args[@]}" || failed+=("$name")
  # npm asks for a pause between the calls.
  sleep 2
done

if [[ ${#failed[@]} -gt 0 ]]; then
  echo "These packages failed: ${failed[*]}" >&2
  [[ "$mode" == list ]] || echo "A package must exist on npm before it can have a trusted publisher." >&2
  exit 1
fi
