# How to release Sayso

A release is a GitHub release with a tag `vX.Y.Z`. You publish it as a prerelease. The Release workflow then builds the app, signs it, notarizes it, attaches the files, signs the update manifest (`latest.json`), and makes the release the latest release. From that last step on, each installed Sayso sees the new version. [updates.md](updates.md) has the design of the updater.

## Set up signing (one time)

The workflow needs six repository secrets.

| Secret | Contents |
|---|---|
| `SAYSO_UPDATE_SIGNING_KEY` | The private key that signs the update manifest: 32 bytes in base64 |
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

### The update signing key

The key pair exists. The private key is the secret `SAYSO_UPDATE_SIGNING_KEY` and the 1Password item "Sayso update signing key (Ed25519, key id 2026-10)". The public key is in `crates/sayso-update/src/keys.rs`, and its id is `SAYSO_UPDATE_KEY_ID` in `.github/workflows/release.yml`.

To set the secret again from 1Password:

```sh
op read "op://Personal/<item id>/private key" | gh secret set SAYSO_UPDATE_SIGNING_KEY
```

Make sure that `op read` printed the key before you trust the secret. `gh secret set` also accepts empty input.

**Change the key.** Do this when you want a new key and the old key is still safe.

1. Make a key pair: `cargo run -p sayso-update --example update_tool -- keygen <new id>`. The entry for `keys.rs` goes to stdout, and the private key goes to stderr.
2. Add the entry to `keys.rs`. Keep the old entry.
3. Ship releases that still sign with the old key. Each app that updates in this period learns the new key.
4. Set the secret to the new key and `SAYSO_UPDATE_KEY_ID` to the new id. An app that never updated in the period shows "A newer Sayso is available" with a link to the download page.

**The key is lost.** Do the steps above, without the period: the next release signs with the new key. All apps with only the old key show the link to the download page. Tell users on the website.

**The key is stolen.** Do the same, and also remove the old entry from `keys.rs` in that release. An app that still trusts the old key accepts a manifest from the thief, but only for files on the Releases page of this repository. Tell users on the website and in the release notes to install the new version by hand at once.

### Homebrew tap

The last job of the workflow, **Homebrew cask**, writes `Casks/sayso.rb` in [watzon/homebrew-tap](https://github.com/watzon/homebrew-tap). It makes the cask from `packaging/homebrew/sayso.rb.in` and the DMG of the run (`scripts/homebrew-cask.sh`). The job runs after the release is the latest release, so a failure there does not change the release. Run the job again to publish again.

The job needs one more secret:

| Secret | Contents |
|---|---|
| `HOMEBREW_TAP_DEPLOY_KEY` | The private key of a deploy key of `watzon/homebrew-tap` that has write access |

To make a new key:

```sh
ssh-keygen -t ed25519 -N "" -C "sayso release" -f tap-key
gh repo deploy-key add tap-key.pub --repo watzon/homebrew-tap --allow-write --title "Sayso release publisher"
gh secret set HOMEBREW_TAP_DEPLOY_KEY --repo watzon/sayso < tap-key
rm tap-key tap-key.pub
```

### AUR package

The job **AUR package** pushes the `PKGBUILD` and the `.SRCINFO` of `sayso-bin` to the [AUR](https://aur.archlinux.org/packages/sayso-bin). The `manifest` job makes the two files from `packaging/linux/aur/PKGBUILD.in` and the tarballs of the run (`scripts/aur-pkgbuild.sh`). The `package()` function installs the three binaries from the tarball into `/usr/lib/sayso`. Like the Homebrew job, it runs after the release is the latest release.

The job needs one secret:

| Secret | Contents |
|---|---|
| `AUR_SSH_PRIVATE_KEY` | The private key of an SSH key of the AUR account that maintains `sayso-bin` |

To make a new key, add the public key to the AUR account (**My Account › SSH Public Key**, one key for each line), then set the secret:

```sh
ssh-keygen -t ed25519 -N "" -C "sayso release (AUR)" -f aur-key
cat aur-key.pub                       # add this line to the AUR account
gh secret set AUR_SSH_PRIVATE_KEY --repo watzon/sayso < aur-key
```

The AUR commit uses the name and the email address of the repository variables `AUR_COMMIT_NAME` and `AUR_COMMIT_EMAIL`, when they are set.

To change the package without a new Sayso version (for example a new dependency), change `PKGBUILD.in`, increase `pkgrel` there, and push the files by hand. Set `pkgrel` back to 1 for the next version.

### Windows signing (optional)

The Windows job signs the installer and the three exes (`Sayso.exe`, `sayso-engine.exe`, and `sayso-cleanup.exe`) when these secrets exist. Without them it builds an unsigned installer, and Windows SmartScreen warns the user.

| Secret | Contents |
|---|---|
| `WINDOWS_CERTIFICATE_PFX` | A code signing certificate with its private key, as a base64 `.pfx` file |
| `WINDOWS_CERTIFICATE_PASSWORD` | The password of the `.pfx` file |

```sh
base64 -i codesign.pfx | gh secret set WINDOWS_CERTIFICATE_PFX
gh secret set WINDOWS_CERTIFICATE_PASSWORD
```

## Make a release

1. Write the notes of the new version in `release-notes.toml`, as the first `[[release]]` entry. Sayso shows them one time in the "What is new" window, at the first start of the new version. The comment at the top of the file gives the fields.
   - Write for a user, not for a developer: name what the user can do now. Keep to five items or fewer.
   - Give an item `systems` when only some systems have the feature.
   - Use `[release.note]` for one announcement or one request, for example a request for support.
   - A version without an entry shows no window. Leave the entry out for a release that has only corrections.

   To see the window, run `scripts/dev.sh --whats-new`. It shows the first entry of the file.

2. Set the new version in `Cargo.toml` (`[workspace.package]`, `version`), run `cargo build`, so `Cargo.lock` gets the version, and set the same version in `engineVersion` in `native/macos/SaysoEngine/Sources/SaysoEngine/Protocol.swift`. The Swift engine cannot read `Cargo.toml`, and its `hello` reply must have the version of the release. The test `the_swift_engine_reports_the_sayso_version` in `sayso-core` (step 3) fails when the two differ. Commit the three files and `release-notes.toml` to `main`.
3. Run the checks on your Mac. CI does not run them on a push.

   ```sh
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   scripts/check-deps.sh
   ```

4. Publish the release as a prerelease. The tag must be `v` plus the version in `Cargo.toml`, or the workflow stops. `--notes-start-tag` names the release before this one, because GitHub does not use a prerelease as the start of the notes by itself.

   ```sh
   gh release create v0.3.0 --target main --title "Sayso 0.3.0" --prerelease --generate-notes --notes-start-tag v0.2.0
   ```

5. Wait for the workflow. It takes about 25 minutes without a build cache. The last job, **Update manifest**, removes the prerelease mark.

   ```sh
   gh run watch
   ```

6. Open the release and make sure that it is the latest release and has these files:
   - `Sayso-<version>-macos-arm64.dmg`
   - `Sayso-<version>-windows-x64-setup.exe`
   - `sayso-<version>-linux-x86_64.tar.gz`, `.deb`, `.rpm`, and `.flatpak`
   - `sayso-<version>-linux-aarch64.tar.gz`, `.deb`, `.rpm`, and `.flatpak`
   - `sayso-<version>-x86_64.AppImage` and `sayso-<version>-aarch64.AppImage`, each with its `.AppImage.zsync` file
   - the engine release files: `sayso-engine-<version>-macos-arm64.tar.gz`, `sayso-engine-<version>-linux-x86_64.tar.gz`, `sayso-engine-<version>-linux-arm64.tar.gz`, and `sayso-engine-<version>-windows-x64.zip`
   - a `.sha256` file for each of the eighteen
   - `latest.json`

   The tap `watzon/homebrew-tap` must also have a new commit, `sayso <version>`.

   The AUR package `sayso-bin` must also have the new version.

7. Point the Nix package and the download buttons of the README to the new release, and commit the two files to `main`. Until this step, `nix run github:watzon/sayso` and the buttons give the release before. The Nix package installs `sayso-cleanup` only when the tarball has it, so it also works with a release before the cleanup runtime. After the first release that has the runtime, remove that test from `packaging/nix/package.nix`.

   ```sh
   scripts/after-release.sh v0.3.0
   git commit -m "Point the Nix package and the README to 0.3.0" packaging/nix/release.json README.md
   ```

If a job fails, the release stays a prerelease, and no installed Sayso sees it. Correct the cause and run the workflow again for the same tag. It replaces files that are already attached.

```sh
gh workflow run release.yml -f tag=v0.3.0
```

If only the last step of **Update manifest** fails, run that job again.

The workflow does not change a release that users already get: each job stops when the release is not a prerelease. To correct such a release, make a new version.

## What ships where

Each release file holds three programs: the app, the speech engine, and the cleanup runtime `sayso-cleanup` (`native/cleanup`, its own Cargo workspace, which builds llama.cpp with CMake). The app finds the runtime in the folder of its own executable (`Runtime::default_path` in `crates/sayso-cleanup-client/src/runtime.rs`). So the runtime goes where the app executable is. No release file is new: the runtime is inside the 12 that exist.

| Release file | Where `sayso-cleanup` is | Signing |
|---|---|---|
| macOS DMG | `Sayso.app/Contents/MacOS/sayso-cleanup`, beside `Sayso` and `SaysoEngine` | Developer ID, hardened runtime, no entitlements, identifier `dev.sayso.Sayso.cleanup`. The notarization of the app covers it. |
| Windows installer | `sayso-cleanup.exe` in the install folder, beside `Sayso.exe` and `sayso-engine.exe` | Signed with the same certificate as the exes, when the secrets exist |
| Linux tarball | `bin/sayso-cleanup`, beside `bin/sayso` | None |
| `.deb`, `.rpm`, AUR | `/usr/lib/sayso/sayso-cleanup`, beside `sayso` (`/usr/bin/sayso` is a link) | None |
| AppImage | `usr/bin/sayso-cleanup` | None |
| Flatpak | `/app/bin/sayso-cleanup` | None |
| Nix | `$out/lib/sayso/sayso-cleanup` | None |
| Homebrew cask | Inside `Sayso.app`, as in the DMG | The DMG |

`scripts/bundle.sh`, `scripts/bundle-windows.ps1`, and `scripts/bundle-linux.sh` build the runtime in release mode, also for a debug app, because llama.cpp is slow in a debug build. Every other Linux format takes the binary from the tarball.

The CI job **Cleanup runtime** (`ci.yml`, start it with `gh workflow run ci.yml`) builds, tests, and lints the runtime on the four targets: macOS arm64, Windows x86_64, Linux x86_64, and Linux arm64. It prints the size and the libraries of the binary, and fails when the binary needs OpenMP. `linux.yml` also starts the installed runtime in the `.deb` test and checks that it answers `hello` with the `cpu` backend.

## Build the release files on your Mac

`scripts/release.sh` does the same steps as the macOS job and writes the files to `dist/`. It needs `create-dmg` (`brew install create-dmg`) and CMake (for the cleanup runtime).

```sh
scripts/release.sh --skip-notarize   # bundle and DMG only. Nothing goes to Apple.
scripts/release.sh                   # also notarize and staple the app and the DMG
scripts/release.sh --debug           # a debug app, for a test of the updater
```

The script notarizes two times: the app, then the DMG. The updater copies the app out of the DMG and asks macOS to check it, and an app with its own stapled ticket passes without a network.

A build on your Mac is not an official build, so it does not look for updates. Set `SAYSO_OFFICIAL_BUILD=1` for the build to turn the updater on.

For notarization, the script uses the keychain profile `notarytool-password`, the same profile that the Pindrop release uses. Set `SAYSO_NOTARY_PROFILE` to use another profile.

## The engine release files

Each release also has the speech engine as a separate file for each system, for programs that run an engine without Sayso ([extensions.md](extensions.md), "The engine as a public product"). Sayso does not use these files: the app has its own engine and never downloads one.

| File | Made by | The engine comes from |
|---|---|---|
| `sayso-engine-<version>-macos-arm64.tar.gz` | `scripts/package-engine.sh`, in the **macOS** job after `scripts/release.sh` | `dist/Sayso.app/Contents/MacOS/SaysoEngine`, signed and notarized with the app |
| `sayso-engine-<version>-windows-x64.zip` | `scripts/package-engine.ps1`, in the **Windows** job after `scripts\bundle-windows.ps1` | `sayso-engine.exe` of the app folder, signed when the certificate is set, with every DLL of that folder |
| `sayso-engine-<version>-linux-x86_64.tar.gz` and `-linux-arm64.tar.gz` | `scripts/package-engine.sh`, in the `build` job of `linux.yml` after `scripts/bundle-linux.sh` | `bin/sayso-engine` of the Linux tarball |

The scripts copy the engine. They never build one, so the file has the binary that was signed. Each file has the folder `sayso-engine-<version>/` with `sayso-engine` (`sayso-engine.exe` on Windows), the DLLs on Windows, `README.md` (from `packaging/engine/README.md.in`), `LICENSE`, and `licenses/`. The licenses are the ones that the app ships for the engine. On macOS, where the app ships none, the file has the licenses of the two Swift packages that the engine links (FluidAudio and WhisperKit). The Linux engine needs the Vulkan loader (`libvulkan.so.1`) from the system, and the README says so.

The Linux files reach the release in the `manifest` job, like the Linux tarballs. The macOS and Windows jobs attach theirs at once. The `manifest` job stops when one of the four is missing.

The job **Engine file check** (`engine-check.yml`) runs after the three build jobs and before the `manifest` job, so a release with a bad engine file does not become the latest release. It runs on `macos-latest`, `ubuntu-24.04`, `ubuntu-24.04-arm`, and `windows-latest`. Each job installs only Rust (and the Vulkan loader on Linux), unpacks the file, checks the files, checks that `hello` has the version of the release, and runs the core level of the conformance suite against the unpacked engine. It builds no engine.

To test the files without a release, run `gh workflow run ci.yml`. The jobs **Engine file, macOS arm64 (manual)**, **Engine file, Windows x86_64 (manual)**, and **Engine file, Linux** build the app without a signature, pack the engine with the same scripts, and then **Engine file check** runs. The files from CI are not signed and not notarized. On a Mac, to make a file from a build that you have:

```sh
scripts/package-engine.sh --out /tmp/engine dist/Sayso.app/Contents/MacOS/SaysoEngine
```

`--require-signed` stops the script when the engine has no Developer ID signature. The release workflow uses it.

## Build the Windows files on a PC

`scripts/bundle-windows.ps1 -Installer` does the same steps as the Windows job and writes the installer and its `.sha256` to `dist\`. It builds the speech engine and the cleanup runtime, so it needs CMake and libclang (`scripts\windows-env.ps1` finds them), and [Inno Setup 6](https://jrsoftware.org/isinfo.php). Set `SAYSO_SIGN_CERT` and `SAYSO_SIGN_PASSWORD` to sign.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 -Installer
```

## Build the Linux files

The Linux jobs are in their own workflow, `linux.yml`, which the Release workflow calls. It has two jobs for each architecture:

1. **Build** compiles the three binaries one time, in an Ubuntu 22.04 container, and makes the tarball (`scripts/bundle-linux.sh`) and the engine release file (`scripts/package-engine.sh`, see "The engine release files"). The container sets the oldest glibc that Sayso runs on: 2.35.
2. **Packages** makes the `.deb`, the `.rpm`, the AppImage, and the Flatpak from the tarball (`scripts/package-linux.sh`), with no compiler. The AppImage gets its copies of the fallback libraries from the packages of this container. It then installs the `.deb` on Ubuntu 22.04 and starts Sayso on a virtual display.

`linux.yml` has the input `skip_packages`, which the CI workflow sets: the **Packages** jobs then do not run.

`linux.yml` does not touch a release, so you can test a packaging change without one. A pull request that changes the packaging runs it. You can also start it by hand, and then download the files from the run:

```sh
gh workflow run linux.yml --ref <branch>
```

`packaging/linux/nfpm.yaml` has the layout and the dependencies of the `.deb` and the `.rpm`. It also declares the glibc and libstdc++ versions. `package-linux.sh` stops when one of the three binaries needs a newer version, so change the numbers there when you change the build container.

`package-linux.sh` downloads nfpm, appimagetool, and the AppImage runtime at fixed versions and checks their SHA-256. To use a newer version, change the URL and the hash in the script for both architectures.

The Flatpak is a one-file bundle, not a Flathub app: Flathub builds each app from source with no network, and the Sayso build downloads the sherpa-onnx library. `package-linux.sh` has the runtime version (`flatpak_runtime`) and the permissions. The workflow does not start the Flatpak, because a start needs the runtime and a sandbox that the job does not have. Test a change to it on a desktop.

`scripts/aur-pkgbuild.sh` makes the AUR files of `sayso-bin` from `packaging/linux/aur/PKGBUILD.in` and the two tarballs of the run.

## Add a platform

The Release workflow has one job for each platform, and the `manifest` job that needs all of them. For a new platform, add a job that builds the files, runs `gh release upload "$TAG" <files> --clobber`, and keeps the files with `actions/upload-artifact` under a name that starts with `release-`. Name the files `Sayso-<version>-<os>-<arch>.<ext>`. Add the engine release file too (the engine check and the `manifest` job have the list). Then add the job to `needs` of the `manifest` job and the file to its list of files. The `manifest` job stops when a file of its lists is missing, so a release cannot become the latest release without it.

## Limits

- Only macOS installs an update by itself. On Windows and Linux, Sayso shows the new version and opens the download page.
- Versions 0.2.0 and older have no updater. Their users get each new version from the download page.
- The workflow does not run the tests. Step 2 above is the only test run before a release.
