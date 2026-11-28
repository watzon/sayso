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

The key pair exists. The private key is the secret `SAYSO_UPDATE_SIGNING_KEY` and the 1Password item "Sayso update signing key (Ed25519, key id 2026-10)". The public key is in `ReleaseKeys` in `internal/update/manifest.go`, and its id is `SAYSO_UPDATE_KEY_ID` in `.github/workflows/release.yml`.

To set the secret again from 1Password:

```sh
op read "op://Personal/<item id>/private key" | gh secret set SAYSO_UPDATE_SIGNING_KEY
```

Make sure that `op read` printed the key before you trust the secret. `gh secret set` also accepts empty input.

**Change the key.** Do this when you want a new key and the old key is still safe.

1. Make a key pair: `go run ./scripts/update-tool keygen <new id>`. The entry for `ReleaseKeys` goes to stdout, and the private key goes to stderr.
2. Add the entry to `ReleaseKeys`. Keep the old entry.
3. Ship releases that still sign with the old key. Each app that updates in this period learns the new key.
4. Set the secret to the new key and `SAYSO_UPDATE_KEY_ID` to the new id. An app that never updated in the period shows "A newer Sayso is available" with a link to the download page.

**The key is lost.** Do the steps above, without the period: the next release signs with the new key. All apps with only the old key show the link to the download page. Tell users on the website.

**The key is stolen.** Do the same, and also remove the old entry from `ReleaseKeys` in that release. An app that still trusts the old key accepts a manifest from the thief, but only for files on the Releases page of this repository. Tell users on the website and in the release notes to install the new version by hand at once.

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

The Windows job signs the installer and the exes (`Sayso.exe`, the speech engine `sayso-engine.exe`, and the cleanup runtime `llama\sayso-cleanup.exe` with the llama.cpp DLLs and `libffi-8.dll` beside it) when these secrets exist. The Visual C++ runtime DLLs keep the signature of Microsoft. Without them it builds an unsigned installer, and Windows SmartScreen warns the user.

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

2. Set the new version in `Version` in `internal/update/version.go`, the one source of the version: the bundle scripts, the engine file scripts, and the Release workflow read it with `scripts/version.sh`. Set the same version in `Version` in `internal/localengine/protocol.go` and in `engineVersion` in `native/macos/SaysoEngine/Sources/SaysoEngine/Protocol.swift`, because each engine answers `hello` with its own copy. `TestTheEnginesHaveTheVersionOfTheApp` (step 3) fails when one differs. Commit the three files and `release-notes.toml` to `main`.
3. Run the checks on your Mac.

   ```sh
   CGO_ENABLED=0 go vet ./internal/... ./cmd/...
   CGO_ENABLED=0 go test ./internal/... ./cmd/...
   ```

4. Publish the release as a prerelease. The tag must be `v` plus the version in `internal/update/version.go`, or the workflow stops. `--notes-start-tag` names the release before this one, because GitHub does not use a prerelease as the start of the notes by itself.

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
   - the engine release files: `sayso-engine-<version>-macos-arm64.tar.gz`, `sayso-engine-<version>-macos-x86_64.tar.gz`, `sayso-engine-<version>-linux-x86_64.tar.gz`, `sayso-engine-<version>-linux-arm64.tar.gz`, and `sayso-engine-<version>-windows-x64.zip`
   - the cleanup runtime files: `sayso-cleanup-<version>-macos-arm64.tar.gz`, `sayso-cleanup-<version>-linux-x86_64.tar.gz`, `sayso-cleanup-<version>-linux-arm64.tar.gz`, and `sayso-cleanup-<version>-windows-x64.zip`
   - a `.sha256` file for each of the twenty-three
   - `latest.json`

   The tap `watzon/homebrew-tap` must also have a new commit, `sayso <version>`.

   The AUR package `sayso-bin` must also have the new version.

7. Point the Nix package and the download buttons of the README to the new release, and commit the two files to `main`. Until this step, `nix run github:watzon/sayso` and the buttons give the release before. The Nix package takes the cleanup runtime from `bin/llama` of the tarball, or from `bin/` for a tarball of 0.16.0 or older.

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

Each release file holds the app, the speech engine, and the cleanup runtime `sayso-cleanup` (`cmd/sayso-cleanup`, Go with `CGO_ENABLED=0`) with its llama.cpp libraries. The runtime loads llama.cpp through [yzma](https://github.com/hybridgroup/yzma) (purego and libffi) from prebuilt shared libraries of one pinned llama.cpp release (`internal/llamalib/pins.go`). No release file builds llama.cpp, and no app build runs Cargo. [cleanup-models.md](cleanup-models.md), "Runtime", has the design.

The libraries are in a folder of their own, never beside the speech engine, so the ggml of llama.cpp and the ggml of whisper.cpp and parakeet.cpp never meet. The runtime starts in its own folder and loads the ggml backends only from the libraries folder (llama.cpp also looks in the folder of the program and the working folder).

| Release file | The runtime | The libraries | Signing |
|---|---|---|---|
| macOS DMG | `Sayso.app/Contents/MacOS/sayso-cleanup` | `Sayso.app/Contents/Frameworks/llama/` (code belongs in `Frameworks`, not in `Resources`), with `libffi.8.dylib` | Developer ID: the runtime with the hardened runtime, no entitlements, identifier `dev.sayso.Sayso.cleanup`; each dylib with the same identity, so library validation accepts it. The notarization of the app covers them. |
| Windows installer | `llama\sayso-cleanup.exe` in the install folder | The same `llama\` folder: `llama.dll`, `ggml.dll`, `ggml-base.dll`, the 14 `ggml-cpu-*.dll`, `libomp.dll`, `libffi-8.dll`, and a copy of the Visual C++ runtime | The exe and the DLLs, with the same certificate as the exes, when the secrets exist |
| Linux tarball | `bin/llama/sayso-cleanup` | `bin/llama/`, with the soname links and `libgomp.so.1` | None |
| `.deb`, `.rpm`, AUR | `/usr/lib/sayso/llama/sayso-cleanup` | `/usr/lib/sayso/llama/` | None |
| AppImage | `usr/bin/llama/sayso-cleanup` | `usr/bin/llama/` | None |
| Flatpak | `/app/bin/llama/sayso-cleanup` | `/app/bin/llama/` | None |
| Nix | `$out/lib/sayso/llama/sayso-cleanup` | `$out/lib/sayso/llama/` (autoPatchelf rewrites them) | None |
| Homebrew cask | Inside `Sayso.app`, as in the DMG | | The DMG |

The pinned release is llama.cpp `v1.0.0`, whose binaries are the nightly build `b12359` (the default of yzma v1.33.0). Each archive and each file Sayso keeps from it has a SHA-256 in `internal/llamalib/pins.go`:

| Platform | Archive | Backend |
|---|---|---|
| macOS arm64 | `llama-b12359-bin-macos-arm64.tar.gz` (ggml-org/llama.cpp) | Metal |
| macOS x86_64 | `llama-b12359-bin-macos-x64.tar.gz` (pinned; Sayso has no Intel app) | CPU |
| Windows x86_64 | `llama-b12359-bin-win-cpu-x64.zip` | CPU |
| Linux x86_64 | `llama-b12359-bin-ubuntu-x64.tar.gz` | CPU |
| Linux arm64 | `llama-v1.0.0-bin-ubuntu-cpu-arm64.tar.gz` (hybridgroup/llama-cpp-builder, a third-party build: llama.cpp publishes no Linux arm64 CPU build) | CPU |

On x86_64, one build runs on every processor: ggml has a CPU library for each processor level (`ggml-cpu-x64`, `-sse42`, `-sandybridge`, `-haswell`, and newer) and loads the best one that the processor runs. A processor without AVX2 works (issue #37); `linux.yml` runs the runtime under QEMU down to x86-64 with no SSE 4.2 (`scripts/test-llama-cpus.sh`). The libraries need glibc 2.34 and GLIBCXX 3.4.29 at most, under the floor of the packages; `scripts/package-linux.sh` checks them with the programs.

The bundle scripts get the libraries with `go run ./scripts/llama-libs fetch`, which downloads each archive once into `build/llama-libs/archives`, checks it, and keeps the files in `build/llama-libs/<tag>-<os>-<arch>`, so a second build downloads nothing. libffi: on macOS the runtime is built with `-tags ffi_no_embed` and `-X github.com/jupiterrider/ffi.filename=@executable_path/../Frameworks/llama/libffi.8.dylib` (the file comes from the yzma binding, pinned by `go.sum` and a SHA-256 in `internal/llamalib`); on Windows `libffi-8.dll` is beside the exe, where the default search finds it; Linux uses the system's `libffi.so.8` (the packages depend on `libffi8`, `libffi.so.8()(64bit)`, or `libffi`). Linux also ships `libgomp.so.1`, from the build system, because the llama.cpp libraries need it and find it through their RUNPATH `$ORIGIN`.

A runtime that finds no libraries (a development build, a broken install) says so in its `hello`, and the app downloads the archive of its platform into the user cache (`<cache>/llama/<tag>-<os>-<arch>`), checks the archive and each file, and the runtime loads them from there. A signed macOS app cannot load these: library validation accepts only libraries signed with its team identity, so a Mac always uses the libraries of the bundle.

`linux.yml` starts the installed Go runtime in the `.deb` test and checks that it answers `hello` with the `cpu` backend and its libraries, and that its libraries take `libgomp` from their own folder.

### The load error of macOS 27

A Mach-O file built with SDK 27 or newer whose string table is not aligned to 8 bytes does not load on macOS 27: dyld says `mis-aligned LINKEDIT string pool`. The linker of Xcode 27 does not pad `stroff` of `LC_SYMTAB` (`indirectsymoff + 4 × nindirectsyms`), so a file with an odd count of indirect symbols is 4 bytes off, by chance. `strip` does not change it. CI builds with Xcode 26, so the files load today. `scripts/check-macho.py` checks every Mach-O file of a release in the macOS job, so a bad file stops the release.

## Build the release files on your Mac

`scripts/release.sh` does the same steps as the macOS job and writes the files to `dist/`. It needs `create-dmg` (`brew install create-dmg`) and CMake (for the MediaRemoteAdapter framework). It does not build the engine file for Intel Macs. See "The engine release files".

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
| `sayso-engine-<version>-macos-x86_64.tar.gz` | `scripts/package-engine.sh --require-signed`, in the **macOS** job after `scripts/build-engine-x86_64.sh` | `dist/engine-x86_64/SaysoEngine`, a cross-compile of the Swift engine for Intel Macs. The script signs and notarizes it like the engine of the app. |
| `sayso-engine-<version>-windows-x64.zip` | `scripts/package-engine.ps1`, in the **Windows** job after `scripts\bundle-windows.ps1` | `sayso-engine.exe` of the app folder, signed when the certificate is set, with every DLL of that folder |
| `sayso-engine-<version>-linux-x86_64.tar.gz` and `-linux-arm64.tar.gz` | `scripts/package-engine.sh`, in the `build` job of `linux.yml` after `scripts/bundle-linux.sh` | `bin/sayso-engine` of the Linux tarball |

The packing scripts copy the engine. They never build one, so the file has the binary that was signed. Only the Intel engine has its own build script, because Sayso has no Intel app that holds an engine. Each file has the folder `sayso-engine-<version>/` with `sayso-engine` (`sayso-engine.exe` on Windows), the DLLs on Windows, `README.md` (from `packaging/engine/README.md.in`), `LICENSE`, and `licenses/`. The licenses are the ones that the app ships for the engine. On macOS, where the app ships none, the file has the licenses of the two Swift packages that the engine links (FluidAudio and WhisperKit). The Linux engine needs the Vulkan loader (`libvulkan.so.1`) from the system, and the README says so.

The Linux files reach the release in the `manifest` job, like the Linux tarballs. The macOS and Windows jobs attach theirs at once. The `manifest` job stops when one of the five is missing.

`scripts/package-engine.sh` reads the architecture of a macOS engine with `lipo` and puts it in the file name. It stops when the engine has more than one architecture, so an arm64 engine never gets the name `macos-x86_64`.

The Intel engine is for Macs with an Intel processor, on macOS 14 or later. An Intel Mac has no Neural Engine, so the models run on the CPU and the GPU and are slower than on Apple silicon. The macOS job cross-compiles the engine on the arm64 runner (`swift build --arch x86_64`) and signs it with the same identity, identifier (`dev.sayso.Sayso.engine`), entitlements, hardened runtime, and secure timestamp as the engine of the app. Then it uploads the engine to Apple with `notarytool`, in a zip, because Apple takes a bare executable only in an archive. A bare executable cannot have a stapled ticket, so macOS looks the ticket up online the first time that it runs the engine.

The job **Engine file check** (`engine-check.yml`) runs after the three build jobs and before the `manifest` job, so a release with a bad engine file does not become the latest release. It runs on `macos-latest`, `macos-15-intel`, `ubuntu-24.04`, `ubuntu-24.04-arm`, and `windows-latest`. `macos-15-intel` is a real Intel Mac. GitHub plans to end its Intel macOS runners after the macOS 15 image, in 2027. When that happens, the Intel check needs another machine. Each job installs only the Vulkan loader (on Linux), unpacks the file, checks the files and their libraries, (on macOS) checks that the architecture of the engine is the one in the file name, and checks that `hello` has the version of the release. It builds no engine.

To test the files without a release, run `gh workflow run ci.yml`. The jobs **Engine file, macOS arm64 (manual)**, **Engine file, macOS x86_64 (manual)**, **Engine file, Windows x86_64 (manual)**, and **Engine file, Linux** build the app (the Intel job builds only the engine) without a signature, pack the engine with the same scripts, and then **Engine file check** runs. The files from CI are not signed and not notarized. On a Mac, to make a file from a build that you have:

```sh
scripts/package-engine.sh --out /tmp/engine dist/Sayso.app/Contents/MacOS/SaysoEngine
```

`--require-signed` stops the script when the engine has no Developer ID signature. The release workflow uses it.

To make the Intel file on a Mac with Apple silicon, build the Intel engine and pack it. `SAYSO_SIGN_IDENTITY=-` signs ad hoc and skips notarization. Without it, the script signs with the Developer ID and notarizes (the credentials are the ones of `scripts/release.sh`). `--skip-notarize` signs without notarization.

```sh
SAYSO_SIGN_IDENTITY=- scripts/build-engine-x86_64.sh
scripts/package-engine.sh --out /tmp/engine dist/engine-x86_64/SaysoEngine
```

## The cleanup runtime files

The SDKs run the local cleanup models with the cleanup runtime, so each release also has it as a file of its own: `sayso-cleanup-<version>-<platform>`, with `README.md`, the GPL `LICENSE`, and `licenses/` (llama.cpp, ggml, and libffi or libgomp or the LLVM OpenMP runtime). There is no Intel Mac file, because Sayso has no Intel app that holds the runtime.

| File | Made by | From |
|---|---|---|
| `sayso-cleanup-<version>-macos-arm64.tar.gz` | `scripts/package-cleanup.sh --require-signed dist/Sayso.app`, in the **macOS** job | `Contents/MacOS/sayso-cleanup` and `Contents/Frameworks/llama/` of the signed and notarized app, as `bin/sayso-cleanup` and `Frameworks/llama/` |
| `sayso-cleanup-<version>-windows-x64.zip` | `scripts/package-cleanup.ps1`, in the **Windows** job | the `llama\` folder of the app folder: `sayso-cleanup.exe`, the llama.cpp DLLs, `libffi-8.dll`, and the Visual C++ runtime |
| `sayso-cleanup-<version>-linux-x86_64.tar.gz` and `-linux-arm64.tar.gz` | `scripts/package-cleanup.sh`, in the `build` job of `linux.yml` | `bin/llama/` of the Linux tarball. The runtime loads `libffi.so.8` from the system. |

The layout is the one in which the runtime finds its libraries (`internal/llamalib`). Before it packs, each script runs `hello` on the packed copy with an empty cache folder and stops unless the reply says `"libraries":"found"`, because a runtime that does not find them downloads them, for up to 10 minutes. The job **Cleanup file check** (`cleanup-check.yml`) runs after the build jobs and before the `manifest` job: on `macos-latest`, `ubuntu-24.04`, `ubuntu-24.04-arm`, and `windows-latest` it unpacks each file, checks `"libraries":"found"` with empty home and cache folders, and loads and runs a tiny model from `scripts/tiny-gguf.py`. `gh workflow run ci.yml` runs the same check on unsigned files. On a Mac, from a build that you have:

```sh
scripts/package-cleanup.sh --out /tmp/cleanup dist/Sayso.app
```

`scripts/npm-packages` makes the folders of the GPL npm packages (`@justsayso/engine`, `@justsayso/cleanup`, and their platform packages) from the release files. Nothing publishes them yet.

## Build the Windows files on a PC

`scripts/bundle-windows.ps1 -Installer` does the same steps as the Windows job and writes the installer and its `.sha256` to `dist\`. It needs Go, the Visual Studio 2022 Build Tools (for the Visual C++ runtime DLLs), `SAYSO_NATIVE_LIBS_DIR` with the engine DLLs (`go run ./scripts/engine-libs fetch` prints a folder with the pinned ones), and [Inno Setup 6](https://jrsoftware.org/isinfo.php). Set `SAYSO_SIGN_CERT` and `SAYSO_SIGN_PASSWORD` to sign.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 -Installer
```

## Build the Linux files

The Linux jobs are in their own workflow, `linux.yml`, which the Release workflow calls. It has two jobs for each architecture:

1. **Build** compiles the three binaries one time, in an Ubuntu 22.04 container, gets the speech engine libraries (`scripts/engine-libs` and `scripts/build-parakeet.sh`), and makes the tarball (`scripts/bundle-linux.sh`) and the engine release file (`scripts/package-engine.sh`, see "The engine release files"). The container sets the oldest glibc that Sayso runs on: 2.35.
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

- macOS installs an update by itself, and Windows hands it to Setup (`docs/updates.md`, "Windows"); an install for all users on Windows opens the download page. On Linux, Sayso shows the new version and opens the download page.

- Versions 0.2.0 and older have no updater. Their users get each new version from the download page.
- The workflow does not run the tests. Step 2 above is the only test run before a release.
