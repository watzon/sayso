#!/usr/bin/env bash
# Build the macOS release files in dist/: a signed, notarized DMG and its checksum.
#
#   scripts/release.sh                   bundle, notarize the app, DMG, notarize the DMG
#   scripts/release.sh --skip-notarize   bundle and DMG only. Nothing goes to Apple.
#   scripts/release.sh --debug           the same with a debug app, for update tests (docs/updates.md)
#
# The app gets its own notarization ticket before it goes into the DMG. The
# updater copies the app out of the DMG and checks it with spctl, and a
# stapled app passes that check without a network.
#
# Notarization uploads the app and the DMG to Apple. The credentials come from one of these:
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
bundle_flags=()
for arg in "$@"; do
  case "$arg" in
    --skip-notarize) notarize=0 ;;
    --debug) bundle_flags+=(--debug) ;;
    *) echo "usage: $0 [--skip-notarize] [--debug]" >&2; exit 2 ;;
  esac
done
command -v create-dmg >/dev/null || { echo "error: create-dmg is missing. Run: brew install create-dmg" >&2; exit 1; }

SAYSO_BUNDLE_DIR=dist scripts/bundle.sh ${bundle_flags[@]+"${bundle_flags[@]}"}
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

if [[ $notarize -eq 1 ]]; then
  echo "==> notarize the app (uploads it to Apple)"
  zip="dist/Sayso-notarize.zip"
  rm -f "$zip"
  ditto -c -k --keepParent "$app" "$zip"
  notarize "$zip"
  rm -f "$zip"
  xcrun stapler staple "$app"
  spctl --assess --type execute -v "$app"
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
  echo "==> notarize the DMG (uploads it to Apple)"
  notarize "$dmg"
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  spctl --assess --type open --context context:primary-signature -v "$dmg"
fi

(cd dist && shasum -a 256 "$(basename "$dmg")" > "$(basename "$dmg").sha256")
echo "==> release files:"
/bin/ls -lh "$dmg" "$dmg.sha256"
