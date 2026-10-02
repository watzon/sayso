#!/usr/bin/env bash
# Build the macOS release files in dist/: a signed, notarized DMG and its checksum.
#
#   scripts/release.sh                   bundle, DMG, notarize, staple
#   scripts/release.sh --skip-notarize   bundle and DMG only. Nothing goes to Apple.
#
# Notarization uploads the DMG to Apple. The credentials come from one of these:
#   APPLE_API_KEY_PATH, APPLE_API_KEY_ID, APPLE_API_ISSUER_ID
#       An App Store Connect API key. The release workflow uses this.
#   SAYSO_NOTARY_PROFILE
#       A keychain profile from `xcrun notarytool store-credentials`.
#       The default is notarytool-password.
#
# The bundle goes to dist/Sayso.app, so a Sayso that runs from build/ keeps running.
set -euo pipefail
cd "$(dirname "$0")/.."
notarize=1
[[ "${1:-}" == "--skip-notarize" ]] && notarize=0
command -v create-dmg >/dev/null || { echo "error: create-dmg is missing. Run: brew install create-dmg" >&2; exit 1; }

SAYSO_BUNDLE_DIR=dist scripts/bundle.sh
app=dist/Sayso.app
version=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$app/Contents/Info.plist")
dmg="dist/Sayso-$version-macos-$(uname -m).dmg"
# The signature names the identity, so this script and bundle.sh cannot disagree.
identity=$(codesign -dvv "$app" 2>&1 | sed -n 's/^Authority=//p' | head -1)
if [[ "$identity" != "Developer ID Application:"* ]]; then
  [[ $notarize -eq 1 ]] && { echo "error: the app has no Developer ID signature, and Apple notarizes only Developer ID builds." >&2; exit 1; }
  echo "warning: the app has no Developer ID signature. The DMG stays unsigned." >&2
  identity=
fi

echo "==> dmg ($dmg)"
rm -f "$dmg"
create-dmg \
  --volname "Sayso" \
  --volicon "$app/Contents/Resources/AppIcon.icns" \
  --window-pos 200 120 \
  --window-size 600 400 \
  --background assets/dmg/background.tiff \
  --icon-size 100 \
  --icon "Sayso.app" 150 190 \
  --hide-extension "Sayso.app" \
  --app-drop-link 450 190 \
  --no-internet-enable \
  "$dmg" "$app" >/dev/null
[[ -n "$identity" ]] && codesign --force --timestamp -s "$identity" "$dmg"

if [[ $notarize -eq 1 ]]; then
  echo "==> notarize (uploads the DMG to Apple)"
  if [[ -n "${APPLE_API_KEY_PATH:-}" ]]; then
    auth=(--key "$APPLE_API_KEY_PATH" --key-id "${APPLE_API_KEY_ID:?}" --issuer "${APPLE_API_ISSUER_ID:?}")
  else
    auth=(--keychain-profile "${SAYSO_NOTARY_PROFILE:-notarytool-password}")
  fi
  result=$(mktemp)
  # notarytool can exit 0 for a rejected upload, so read the status from the result.
  xcrun notarytool submit "$dmg" "${auth[@]}" --wait --output-format json > "$result" || true
  status=$(plutil -extract status raw -o - "$result" 2>/dev/null || true)
  if [[ "$status" != "Accepted" ]]; then
    echo "error: notarization status is '${status:-unknown}'." >&2
    cat "$result" >&2
    echo "Read the log with: xcrun notarytool log <id> <the same credentials>" >&2
    exit 1
  fi
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  spctl --assess --type open --context context:primary-signature -v "$dmg"
fi

(cd dist && shasum -a 256 "$(basename "$dmg")" > "$(basename "$dmg").sha256")
echo "==> release files:"
/bin/ls -lh "$dmg" "$dmg.sha256"
