# How to release Sayso

A release is a GitHub release with a tag `vX.Y.Z`. When you publish it, the Release workflow builds the app, signs it, notarizes it, and attaches the files to the release.

## Set up signing (one time)

The workflow needs five repository secrets.

| Secret | Contents |
|---|---|
| `MACOS_CERTIFICATE_P12` | The Developer ID Application certificate with its private key, as a base64 `.p12` file |
| `MACOS_CERTIFICATE_PASSWORD` | The password of the `.p12` file |
| `APPLE_API_KEY_P8` | The text of an App Store Connect API key (`AuthKey_<key id>.p8`) |
| `APPLE_API_KEY_ID` | The key ID of that API key |
| `APPLE_API_ISSUER_ID` | The issuer ID of the App Store Connect team |

### 1. Export the certificate

1. Open Keychain Access and select the **login** keychain, then **My Certificates**.
2. Select `Developer ID Application: Watzon Ventures LLc (MB5789APU7)`. Select only this one certificate.
3. Select **File › Export Items**, keep the format **Personal Information Exchange (.p12)**, and save the file outside the repository.
4. Set a password for the file. Remember it for step 3.

### 2. Create the API key

The workflow uses an API key for notarization, because an API key needs no Apple ID password and no two-factor code.

1. Open <https://appstoreconnect.apple.com/access/integrations/api>.
2. On the **Team Keys** tab, add a key with the **Developer** role.
3. Download the `.p8` file. Apple lets you download it only one time.
4. Copy the **Key ID** of the key and the **Issuer ID** at the top of the page.

### 3. Store the secrets

```sh
scripts/set-release-secrets.sh ~/Desktop/developer-id.p12 ~/Downloads/AuthKey_ABC123DEFG.p8 ABC123DEFG <issuer id>
```

The script asks for the `.p12` password and stores the five secrets with `gh secret set`. Then delete the `.p12` file. Keep the `.p8` file in your password manager.

If the certificate is not `Developer ID Application: Watzon Ventures LLc (MB5789APU7)`, also set the repository variable `SAYSO_SIGN_IDENTITY` to its name.

### Windows signing (optional)

The Windows job signs the installer and the two exes when these secrets exist. Without them it builds an unsigned installer, and Windows SmartScreen warns the user.

| Secret | Contents |
|---|---|
| `WINDOWS_CERTIFICATE_PFX` | A code signing certificate with its private key, as a base64 `.pfx` file |
| `WINDOWS_CERTIFICATE_PASSWORD` | The password of the `.pfx` file |

```sh
base64 -i codesign.pfx | gh secret set WINDOWS_CERTIFICATE_PFX
gh secret set WINDOWS_CERTIFICATE_PASSWORD
```

## Make a release

1. Set the new version in `Cargo.toml` (`[workspace.package]`, `version`), run `cargo build`, so `Cargo.lock` gets the version, and commit both files to `main`.
2. Run the checks on your Mac. CI does not run them on a push.

   ```sh
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   scripts/check-deps.sh
   ```

3. Publish the release. The tag must be `v` plus the version in `Cargo.toml`, or the workflow stops.

   ```sh
   gh release create v0.1.0 --target main --title "Sayso 0.1.0" --generate-notes
   ```

4. Wait for the workflow. It takes about 20 minutes without a build cache.

   ```sh
   gh run watch
   ```

5. Open the release and make sure that it has these four files:
   - `Sayso-<version>-macos-arm64.dmg`
   - `Sayso-<version>-macos-arm64.dmg.sha256`
   - `Sayso-<version>-windows-x64-setup.exe`
   - `Sayso-<version>-windows-x64-setup.exe.sha256`

If the workflow fails, correct the cause and run it again for the same tag. It replaces files that are already attached.

```sh
gh workflow run release.yml -f tag=v0.1.0
```

## Build the release files on your Mac

`scripts/release.sh` does the same steps as the workflow and writes the files to `dist/`. It needs `create-dmg` (`brew install create-dmg`).

```sh
scripts/release.sh --skip-notarize   # bundle and DMG only. Nothing goes to Apple.
scripts/release.sh                   # also notarize and staple
```

For notarization, the script uses the keychain profile `notarytool-password`, the same profile that the Pindrop release uses. Set `SAYSO_NOTARY_PROFILE` to use another profile.

## Build the Windows files on a PC

`scripts/bundle-windows.ps1 -Installer` does the same steps as the Windows job and writes the installer and its `.sha256` to `dist\`. It needs CMake and [Inno Setup 6](https://jrsoftware.org/isinfo.php). Set `SAYSO_SIGN_CERT` and `SAYSO_SIGN_PASSWORD` to sign.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 -Installer
```

## Add a platform

The Release workflow has one job for each platform: `macos` and `windows`. To add Linux, add a job next to them that builds the files for that platform and runs `gh release upload "$TAG" <files> --clobber`. Name the files `Sayso-<version>-<os>-<arch>.<ext>`.

## Limits

- Sayso has no automatic update. A user downloads each new version from the Releases page.
- The workflow does not run the tests. Step 2 above is the only test run before a release.
