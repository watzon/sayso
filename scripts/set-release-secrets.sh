#!/usr/bin/env bash
# Store the signing secrets of the Release workflow in the GitHub repository.
#
#   scripts/set-release-secrets.sh <certificate.p12> <AuthKey.p8> <key id> <issuer id>
#
# docs/releasing.md tells you how to get the two files and the two ids.
# The script sends the values only to GitHub, through `gh secret set`.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $# -eq 4 ]] || { sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 1; }
p12=$1 p8=$2 key_id=$3 issuer_id=$4
[[ -f "$p12" ]] || { echo "error: no file at $p12" >&2; exit 1; }
grep -q "BEGIN PRIVATE KEY" "$p8" 2>/dev/null || { echo "error: $p8 is not an App Store Connect API key (.p8)" >&2; exit 1; }

read -r -s -p "Password of the .p12 file: " password; echo
# Fail now, not in the workflow, if the password is wrong or the file holds another certificate.
subject=$(openssl pkcs12 -in "$p12" -passin "pass:$password" -nokeys -legacy 2>/dev/null | openssl x509 -noout -subject 2>/dev/null) \
  || subject=$(openssl pkcs12 -in "$p12" -passin "pass:$password" -nokeys 2>/dev/null | openssl x509 -noout -subject 2>/dev/null) \
  || { echo "error: cannot read $p12 with that password" >&2; exit 1; }
[[ "$subject" == *"Developer ID Application"* ]] || { echo "error: $p12 holds no Developer ID Application certificate ($subject)" >&2; exit 1; }
echo "certificate: $subject"

base64 < "$p12" | gh secret set MACOS_CERTIFICATE_P12
printf '%s' "$password" | gh secret set MACOS_CERTIFICATE_PASSWORD
gh secret set APPLE_API_KEY_P8 < "$p8"
printf '%s' "$key_id" | gh secret set APPLE_API_KEY_ID
printf '%s' "$issuer_id" | gh secret set APPLE_API_ISSUER_ID
echo "The five secrets are set:"
gh secret list
