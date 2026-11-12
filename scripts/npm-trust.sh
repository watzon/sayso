#!/usr/bin/env bash
# Give each npm package of Sayso the trusted publisher release.yml of
# watzon/sayso, so the job "npm publish" of the Release workflow can publish it
# with OIDC and no token (docs/releasing.md, "npm packages").
#
#   scripts/npm-trust.sh            # add the trusted publisher to every package
#   scripts/npm-trust.sh --dry-run  # show what npm would do
#   scripts/npm-trust.sh --list     # show the trusted publishers of every package
#
# It needs npm 11.15.0 or later, a session of `npm login` with two-factor
# authentication, and packages that exist on npm. Each call needs a one-time
# password. Set NPM_OTP_COMMAND to a command that prints one, and the script
# gets a new one for each package, for example:
#
#   NPM_OTP_COMMAND='op item get <npm item> --otp' scripts/npm-trust.sh
#
# Without it, npm asks for the password in the terminal.
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
npm whoami >/dev/null || { echo "Log in first: npm login" >&2; exit 1; }

failed=()
while read -r name; do
  if [[ "$mode" == list ]]; then
    echo "==> $name"
    npm trust list "$name" || failed+=("$name")
    continue
  fi
  args=(trust github "$name" --file release.yml --repository watzon/sayso --allow-publish --yes)
  [[ "$mode" == dry-run ]] && args+=(--dry-run)
  if [[ -n "${NPM_OTP_COMMAND:-}" ]]; then
    otp=$(bash -c "$NPM_OTP_COMMAND") || { echo "NPM_OTP_COMMAND failed" >&2; exit 1; }
    args+=(--otp "$otp")
  fi
  echo "==> $name"
  npm "${args[@]}" || failed+=("$name")
  # npm asks for a pause between the calls.
  sleep 2
done < <(python3 scripts/npm-packages.py names)

if [[ ${#failed[@]} -gt 0 ]]; then
  echo "These packages failed: ${failed[*]}" >&2
  echo "A package must exist on npm before it can have a trusted publisher." >&2
  exit 1
fi
