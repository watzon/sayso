#!/usr/bin/env bash
# DO NOT RUN during the spike: this uploads the app to Apple. It needs the owner's approval.
# Prereq (once, by the owner): xcrun notarytool store-credentials "<profile>" --apple-id ... --team-id MB5789APU7
set -euo pipefail
cd "$(dirname "$0")"

PROFILE="${1:?usage: notarize.sh <keychain-profile>}"
APP="build/SaysoSpike.app"
ZIP="build/SaysoSpike.zip"

[ -d "$APP" ] || { echo "run ./sign.sh first"; exit 1; }
codesign --verify --deep --strict "$APP"

# ditto keeps resource forks and symlinks; plain zip can break the signature.
ditto -c -k --keepParent "$APP" "$ZIP"

# Uploads to Apple and waits for the verdict. Prints the submission id.
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait

# Attach the ticket so Gatekeeper passes offline.
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute -vv "$APP"
