# Sayso

[![Newest release](https://img.shields.io/github/v/release/watzon/sayso?style=flat-square)](https://github.com/watzon/sayso/releases/latest)
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-GitHub_Sponsors-EA4AAA?style=flat-square&logo=githubsponsors&logoColor=white)](https://github.com/sponsors/watzon)

Sayso is a dictation app for macOS, Windows, and Linux. You press a key, speak, and Sayso writes the text into the app you are using. Speech recognition runs on your computer: on the Neural Engine of a Mac, and on the processor of a Windows or Linux computer. Nothing leaves the computer unless you choose a cloud model or turn on an AI style with a cloud provider.

![The Sayso Hub in the light theme](docs/images/hub-home.png)

Sayso is written in Go with [MyGo](https://github.com/egoist/mygo). The SDK crates and the cleanup runtime are Rust. On macOS the speech engine is a Swift program that uses [WhisperKit](https://github.com/argmaxinc/argmax-oss-swift) and [FluidAudio](https://github.com/FluidInference/FluidAudio). On Windows and Linux it is a Go program that loads [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx), and also [whisper.cpp](https://github.com/ggml-org/whisper.cpp) on Windows. The same developer made [Pindrop](https://github.com/watzon/pindrop).

## What Sayso does

- **Dictation into any app.** Option+Space (Ctrl+Space on Windows, Ctrl+Alt+Space on Linux) starts a dictation, and the same key stops it. You can also set a push-to-talk key. Sayso pastes the text into the focused app and then restores your clipboard.
- **Live preview.** An overlay shows a waveform, a timer, and the words while you speak.
- **Local models.** On macOS: Parakeet, Fluid Speech Mini, Nemotron, Cohere Transcribe, Canary, SenseVoice, Paraformer, eight Whisper variants, and Apple Speech on macOS 26. On Windows: Parakeet TDT v2 and v3, and six Whisper variants. On Linux: Parakeet, Whisper, SenseVoice, Moonshine, a streaming Zipformer, and Fluid Speech Mini on a GPU. You download the models that you want in the Models page.
- **Cloud models, if you want them.** OpenAI, Groq, ElevenLabs, Deepgram, AssemblyAI, Mistral, or any server with the OpenAI transcription API. You use your own API key.
- **AI styles.** A style edits the transcript with a prompt, for example Clean, Polished, Rambling, Message, Email, or Notes. A style uses an OpenAI-compatible endpoint, the Claude CLI, or your ChatGPT plan. Raw is the style with no AI. Each style is one TOML file that you can edit. All styles share a base prompt that removes filler words, applies your self-corrections ("no wait, Friday"), writes numbers as digits, and puts a word that you spell out in place of the wrong one. The built-in styles keep your own words: your slang and your contractions stay. You can edit the base prompt and reset it.
- **Style by app and site.** A style can list the apps and websites that use it, so Mail gets Email and a chat app gets Message with no action from you. This is off until you turn it on in Settings › Dictation. Sayso reads the website on macOS and Windows. On Linux it works by app only, and not on GNOME or KDE with Wayland.
- **Dictionary.** Words bias the recognition toward names and terms. Replacements change text after transcription, for example "git hub" to "GitHub".
- **History.** Sayso stores each dictation with its transcript, its final text, and its audio. Text stays until you delete it. Audio expires after 30 days by default.
- **Import from Pindrop.** If you used Pindrop on your Mac, Sayso copies your dictations, dictionary, and prompt presets. Find it at the end of onboarding and in Settings › History and privacy.
- **No lost text.** If the insertion or the AI style fails, the text stays on the screen and in History.
- **Config as a file.** All settings are in `config.toml`, and Sayso reloads the file when you save it.

[CONTEXT.md](CONTEXT.md) defines the terms.

## Install

### Supported platforms

Select a button to download the file of the newest release. The [Releases page](https://github.com/watzon/sayso/releases/latest) has all files, each with a `.sha256` file.

| System | Versions | Download |
|---|---|---|
| macOS | 14 or later, on Apple Silicon | [![DMG for macOS](https://img.shields.io/badge/DMG-Apple_Silicon-000000?style=for-the-badge&logo=apple&logoColor=white)][dmg]<br>`brew install --cask watzon/tap/sayso` |
| Windows | 10 (version 1809) or later, and 11, on a 64-bit Intel or AMD processor | [![Installer for Windows](https://img.shields.io/badge/Installer-x64-0078D4?style=for-the-badge)][exe] |
| Debian, Ubuntu | Debian 12 or later, Ubuntu 22.04 or later | [![.deb for x86_64](https://img.shields.io/badge/.deb-x86__64-A81D33?style=for-the-badge&logo=debian&logoColor=white)][deb-x86_64] [![.deb for aarch64](https://img.shields.io/badge/.deb-aarch64-A81D33?style=for-the-badge&logo=debian&logoColor=white)][deb-aarch64] |
| Fedora, openSUSE, RHEL family | Fedora, Leap 15.6 or later, Tumbleweed, RHEL 10 | [![.rpm for x86_64](https://img.shields.io/badge/.rpm-x86__64-51A2DA?style=for-the-badge&logo=fedora&logoColor=white)][rpm-x86_64] [![.rpm for aarch64](https://img.shields.io/badge/.rpm-aarch64-51A2DA?style=for-the-badge&logo=fedora&logoColor=white)][rpm-aarch64] |
| Linux with Flatpak | Any distribution | [![Flatpak for x86_64](https://img.shields.io/badge/Flatpak-x86__64-4A90D9?style=for-the-badge&logo=flatpak&logoColor=white)][flatpak-x86_64] [![Flatpak for aarch64](https://img.shields.io/badge/Flatpak-aarch64-4A90D9?style=for-the-badge&logo=flatpak&logoColor=white)][flatpak-aarch64] |
| Other Linux | glibc 2.35 or later | [![AppImage for x86_64](https://img.shields.io/badge/AppImage-x86__64-1F6FEB?style=for-the-badge&logo=linux&logoColor=white)][appimage-x86_64] [![AppImage for aarch64](https://img.shields.io/badge/AppImage-aarch64-1F6FEB?style=for-the-badge&logo=linux&logoColor=white)][appimage-aarch64]<br>[![tarball for x86_64](https://img.shields.io/badge/tar.gz-x86__64-555555?style=for-the-badge&logo=linux&logoColor=white)][tarball-x86_64] [![tarball for aarch64](https://img.shields.io/badge/tar.gz-aarch64-555555?style=for-the-badge&logo=linux&logoColor=white)][tarball-aarch64] |
| Arch Linux | Current | [`sayso-bin`](https://aur.archlinux.org/packages/sayso-bin) on the AUR |
| NixOS | Flakes | `nix run github:watzon/sayso` |

Linux runs on X11 and on Wayland. [docs/linux.md](docs/linux.md) lists what works on each desktop.

[dmg]: https://github.com/watzon/sayso/releases/download/v0.15.0/Sayso-0.15.0-macos-arm64.dmg
[exe]: https://github.com/watzon/sayso/releases/download/v0.15.0/Sayso-0.15.0-windows-x64-setup.exe
[deb-x86_64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-x86_64.deb
[deb-aarch64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-aarch64.deb
[rpm-x86_64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-x86_64.rpm
[rpm-aarch64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-aarch64.rpm
[flatpak-x86_64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-x86_64.flatpak
[flatpak-aarch64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-aarch64.flatpak
[appimage-x86_64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-x86_64.AppImage
[appimage-aarch64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-aarch64.AppImage
[tarball-x86_64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-x86_64.tar.gz
[tarball-aarch64]: https://github.com/watzon/sayso/releases/download/v0.15.0/sayso-0.15.0-linux-aarch64.tar.gz

### macOS

With [Homebrew](https://brew.sh):

```sh
brew install --cask watzon/tap/sayso
```

Or by hand:

1. Download `Sayso-<version>-macos-arm64.dmg` with the button above.
2. Open the DMG and move Sayso to the Applications folder.
3. Open Sayso. Onboarding starts.

The DMG is signed with the Developer ID of Watzon Ventures LLC and notarized by Apple. Each release also has a `.sha256` file. To check your download:

```sh
shasum -a 256 -c Sayso-<version>-macos-arm64.dmg.sha256
```

### Windows

1. Download `Sayso-<version>-windows-x64-setup.exe` with the button above.
2. Open it. It installs Sayso for your user account, without administrator rights.
3. Sayso starts after the install. Onboarding starts.

To check your download, compare the hash with the `.sha256` file:

```powershell
(Get-FileHash Sayso-<version>-windows-x64-setup.exe).Hash
```

If the installer is not signed, Windows SmartScreen asks first. Select **More info › Run anyway**.

### Linux

1. Download the file for your system with a button above. Each file is `sayso-<version>-linux-<arch>` with one of these endings, and `<arch>` is `x86_64` or `aarch64`.

   | File | System | Install |
   |---|---|---|
   | `.deb` | Debian 12 or later, Ubuntu 22.04 or later | `sudo apt install ./sayso-<version>-linux-<arch>.deb` |
   | `.rpm` | Fedora, openSUSE, and the RHEL 10 family | `sudo dnf install ./sayso-<version>-linux-<arch>.rpm` (openSUSE: `sudo zypper install`) |
   | AUR | Arch Linux and systems based on it | `yay -S sayso-bin`, or another AUR helper |
   | Nix flake | NixOS, and Nix on other distributions | `nix run github:watzon/sayso`. [docs/linux.md](docs/linux.md) has the NixOS configuration. |
   | `.flatpak` | Any distribution with Flatpak | `flatpak install --user ./sayso-<version>-linux-<arch>.flatpak` |
   | `.AppImage` | Other distributions, no install | `chmod +x` the file, then start it. Sayso is also in the [AppImage catalog](https://appimage.github.io/Sayso/). |
   | `.tar.gz` | Other distributions | Unpack it and run `./install.sh`. Sayso goes to `~/.local`. `sudo ./install.sh --system` installs it for all users, with the udev rule for Wayland key input. |

2. Open Sayso from your app menu, or start the AppImage. Onboarding starts.

The `.deb`, the `.rpm`, the AppImage, and the tarball need glibc 2.35 or later. The Flatpak brings its own libraries. [docs/linux.md](docs/linux.md) lists the tested distributions and has the details for each desktop.

## First start

1. Choose a model and download it. The default, Parakeet Unified, is about 1.3 GB.
2. Allow the microphone and, on macOS, Accessibility. Sayso needs Accessibility to paste text into other apps. Windows needs no extra permission. On Linux, [docs/linux.md](docs/linux.md) lists the permissions for each desktop.
3. Press Option+Space (Ctrl+Space on Windows, Ctrl+Alt+Space on Linux), speak, and press the key again.

## Privacy

With a local model and the Raw style, your speech and your text stay on the computer.

| You turn on | What leaves the computer | Where it goes |
|---|---|---|
| A cloud model | The audio of each dictation and your dictionary words | The speech provider that you added |
| An AI style | The transcript, the style prompt, and your dictionary words | The AI provider of that style |

The official build also looks for a new version when it starts and one time each day. It gets one file from the Releases page of this repository on GitHub. The request names your Sayso version and your system (for example `Sayso/0.3.0 (macos; aarch64)`), and GitHub sees your IP address. It has no identifier of you or your computer. To stop these checks, turn off **Settings › General › Check for updates**. A build from source makes no such request.

### Anonymous usage telemetry

Sayso sends a few anonymous signals to [TelemetryDeck](https://telemetrydeck.com), so its developer can see how many people use it and which models they pick. It is on by default. Turn it off on the last step of onboarding, or in **Settings › General › Anonymous usage telemetry**. On Android, the switch is in Setup and in Settings. Off stops every signal at once, including any that wait to be sent. Nothing is sent before you finish onboarding (on Android, before you enable the keyboard).

There are three signals: `app.installed` (once per install), `app.launched`, and `dictation.completed`. Each holds only these fields:

| Field | What it is |
|---|---|
| `clientUser` | The SHA-256 of a random ID that Sayso makes on first use and keeps on the device (`telemetry.json` in the data folder). TelemetryDeck hashes it again with a salt. It is not linked to you, your account, or your hardware. |
| `sessionID` | A random ID for each launch |
| `os`, `arch`, `platform` | For example `darwin`, `arm64`, `desktop` (or `android`) |
| `appVersion`, `buildChannel` | For example `0.16.0`, `release` |
| `modelId`, `modelKind` | Only in `dictation.completed`: the model's catalog name and `local` or `cloud`. A cloud model is the provider type and a public model name, such as `groq:whisper-large-v3`. A custom endpoint's model is not named. Android's own recognizer is `android_system` and `system`. |
| `styleKind` | Only in `dictation.completed`: `raw`, `ai_local`, or `ai_cloud` |

Sayso never sends your words, transcripts, audio, how long you spoke, dictionary words, style prompts, the app or website you dictate into, API keys, or the names and addresses of your providers. Incognito dictations are not counted, and neither are dictations into password or private fields on Android. TelemetryDeck sees your IP address, as any server does, and its [privacy FAQ](https://telemetrydeck.com/docs/guides/privacy-faq/) says it never stores IP addresses. Its servers are in the EU.

A build from source sends signals only when it has a TelemetryDeck App ID, and it marks them as test signals.

### Cloud destinations and keys

The Models page and the Styles page label each cloud destination. Your API keys are in the macOS Keychain, in Windows Credential Manager, or in the Secret Service keyring on Linux.

## License and price

The source code is free software under the [GNU General Public License, version 3](LICENSE). You can read it, change it, and build it for yourself at no cost.

The official build is the signed and notarized app. It is free, and it needs no license key. Sayso is paid for by the people who use it: the download page at [justsayso.app](https://justsayso.app) asks what you want to pay, and $0 is a valid answer.

The SDK is an exception. The crates in `crates/` and the packages in `packages/` are MIT-licensed, so that other apps can use Sayso's speech. Each has its own `LICENSE` file.

The name Sayso and the app icon are not part of the license. If you publish a changed version, give it another name and another icon.

## Support Sayso

Sayso has no price and no ads. The people who use it pay for the work on it.

If you got Sayso from GitHub (a release file, the Nix flake, or a build from source), nothing asked you what you want to pay. If Sayso is useful to you, sponsor its developer:

[![Sponsor Sayso on GitHub Sponsors](https://img.shields.io/badge/Sponsor-GitHub_Sponsors-EA4AAA?style=for-the-badge&logo=githubsponsors&logoColor=white)](https://github.com/sponsors/watzon)

A one-time amount and a monthly amount are both possible. A sponsorship gives you no extra features: each build of Sayso is the full app.

## Build from source

### macOS

You need Xcode 26 or later and Go (the version in `go.mod`). Rust 1.98.1 (`rust-toolchain.toml`) builds the SDK and the cleanup runtime.

```sh
git clone https://github.com/watzon/sayso.git
cd sayso
SAYSO_SIGN_IDENTITY=- scripts/bundle.sh
open build/Sayso.app
```

`SAYSO_SIGN_IDENTITY=-` makes an ad hoc signature, which needs no Apple developer account. With an ad hoc signature, macOS asks for the microphone and Accessibility again after each rebuild. To keep the grants, set `SAYSO_SIGN_IDENTITY` to the name of your own signing certificate.

### Windows

You need the Visual Studio 2022 Build Tools (the "Desktop development with C++" workload and a Windows SDK), CMake, LLVM (for libclang; set `LIBCLANG_PATH` when it is not on `PATH`), Go, and Rust 1.98.1.

```powershell
git clone https://github.com/watzon/sayso.git
cd sayso
powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1
build\windows\Sayso\Sayso.exe
```

Add `-Installer` to also make the installer in `dist\`. That needs [Inno Setup 6](https://jrsoftware.org/isinfo.php).

### Linux

You need the packages in `scripts/linux-deps.sh` (Debian and Ubuntu names), Go, and Rust 1.98.1.

```sh
git clone https://github.com/watzon/sayso.git
cd sayso
scripts/linux-deps.sh
scripts/bundle-linux.sh
```

The script makes `build/sayso-<version>-linux-<arch>.tar.gz`. Unpack it and run `./install.sh`. To make the `.deb`, the `.rpm`, the AppImage, and the Flatpak from that tarball, run `scripts/package-linux.sh build/sayso-<version>-linux-<arch>.tar.gz`.

[CONTRIBUTING.md](CONTRIBUTING.md) has the development commands, the checks, and the layout of the code.

## Files on disk

| Kind | macOS | Windows | Linux |
|---|---|---|---|
| Config (`config.toml`, `styles/`) | `~/Library/Application Support/Sayso` | `%APPDATA%\Sayso` | `~/.config/sayso` |
| History, audio, models | `~/Library/Application Support/Sayso` | `%LOCALAPPDATA%\Sayso` | `~/.local/share/sayso` |
| Telemetry ID (`telemetry.json`) | `~/Library/Application Support/Sayso` | `%LOCALAPPDATA%\Sayso` | `~/.local/share/sayso` |
| Logs | `~/Library/Caches/Sayso/logs/sayso.log` | `%LOCALAPPDATA%\Sayso\Cache\logs\sayso.log` | `~/.cache/sayso/logs/sayso.log` |

On all three, `~/.config/sayso` wins for the config when it exists, and `$XDG_CONFIG_HOME`, `$XDG_DATA_HOME`, and `$XDG_CACHE_HOME` win over all of them.

API keys are in the Keychain (macOS), in Windows Credential Manager, or in the Secret Service keyring (Linux), under `dev.sayso.Sayso`. The Windows uninstaller keeps your history, models, and settings. To remove them too, delete the two `Sayso` folders above.

## Help and contributions

- To report a bug or ask for a feature, open an [issue](https://github.com/watzon/sayso/issues).
- To report a security problem, follow [SECURITY.md](SECURITY.md). Do not open a public issue.
- To change the code, read [CONTRIBUTING.md](CONTRIBUTING.md).

## More documents

- [docs/plan.md](docs/plan.md): decisions and milestones.
- [docs/research.md](docs/research.md): research and sources.
- [docs/releasing.md](docs/releasing.md): how to make a release.
- [docs/linux.md](docs/linux.md): Sayso on each Linux desktop.
- [docs/porting.md](docs/porting.md): the platform seams, for the next ports.
