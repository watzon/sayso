# shellcheck shell=bash
# Source this file: it defines notarize <file>, which uploads a file to Apple
# and waits for the answer. scripts/release.sh and
# scripts/build-engine-x86_64.sh use it.
#
# The credentials come from one of these:
#   APPLE_API_KEY_PATH, APPLE_API_KEY_ID, APPLE_API_ISSUER_ID
#       An App Store Connect API key. The release workflow uses this.
#   SAYSO_NOTARY_PROFILE
#       A keychain profile from `xcrun notarytool store-credentials`.
#       The default is notarytool-password.

# Upload a file to Apple and wait for the answer.
notarize() {
  local file=$1 auth result status
  if [[ -n "${APPLE_API_KEY_PATH:-}" ]]; then
    auth=(--key "$APPLE_API_KEY_PATH" --key-id "${APPLE_API_KEY_ID:?}" --issuer "${APPLE_API_ISSUER_ID:?}")
  else
    auth=(--keychain-profile "${SAYSO_NOTARY_PROFILE:-notarytool-password}")
  fi
  result=$(mktemp)
  # notarytool can exit 0 for a rejected upload, so read the status from the result.
  xcrun notarytool submit "$file" "${auth[@]}" --wait --output-format json > "$result" || true
  status=$(plutil -extract status raw -o - "$result" 2>/dev/null || true)
  if [[ "$status" != "Accepted" ]]; then
    echo "error: notarization status of $file is '${status:-unknown}'." >&2
    cat "$result" >&2
    echo "Read the log with: xcrun notarytool log <id> <the same credentials>" >&2
    exit 1
  fi
}
