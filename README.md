# Sayso

Sayso is a dictation app for macOS and Linux. You press a key, speak, and Sayso writes the text into the app you are using. Speech recognition runs on your computer: on the Neural Engine of a Mac, and on the CPU on Linux. Nothing leaves the computer unless you choose a cloud model or turn on an AI style with a cloud provider.

![The Sayso Hub in the light theme](docs/images/hub-home.png)

Sayso is written in Rust with [GPUI](https://www.gpui.rs/). On macOS the speech engine is a Swift program that uses [WhisperKit](https://github.com/argmaxinc/argmax-oss-swift) and [FluidAudio](https://github.com/FluidInference/FluidAudio). On Linux it is a Rust program that uses [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx). The same developer made [Pindrop](https://github.com/watzon/pindrop).

## What Sayso does

- **Dictation into any app.** Option+Space starts a dictation, and Option+Space stops it (Ctrl+Alt+Space on Linux). You can also set a push-to-talk key. Sayso pastes the text into the focused app and then restores your clipboard.
- **Live preview.** An overlay shows a waveform, a timer, and the words while you speak.
- **Local models.** Parakeet, Nemotron, Cohere Transcribe, Canary, SenseVoice, Paraformer, eight Whisper variants, and Apple Speech on macOS 26. You download the models that you want in the Models page.
- **Cloud models, if you want them.** OpenAI, Groq, ElevenLabs, Deepgram, AssemblyAI, Mistral, or any server with the OpenAI transcription API. You use your own API key.
- **AI styles.** A style rewrites the transcript with a prompt, for example Clean, Polished, Message, Email, or Notes. A style uses an OpenAI-compatible endpoint, the Claude CLI, or the Codex CLI. Raw is the style with no AI. Each style is one TOML file that you can edit.
- **Dictionary.** Words bias the recognition toward names and terms. Replacements change text after transcription, for example "git hub" to "GitHub".
- **History.** Sayso stores each dictation with its transcript, its final text, and its audio. Text stays until you delete it. Audio expires after 30 days by default.
- **Import from Pindrop.** If you used Pindrop on this Mac, Sayso copies your dictations, dictionary, and prompt presets. Find it at the end of onboarding and in Settings › History and privacy.
- **No lost text.** If the insertion or the AI style fails, the text stays on the screen and in History.
- **Config as a file.** All settings are in `config.toml`, and Sayso reloads the file when you save it.

[CONTEXT.md](CONTEXT.md) defines the terms.

## Requirements

- macOS 14 or later on a Mac with Apple Silicon.
- Or Linux on x86_64 or aarch64, with X11 or Wayland. [docs/linux.md](docs/linux.md) lists what works on each desktop.

A Windows version is planned.

## Install

### macOS

1. Download `Sayso-<version>-macos-arm64.dmg` from the [Releases page](https://github.com/watzon/sayso/releases).
2. Open the DMG and move Sayso to the Applications folder.
3. Open Sayso. Onboarding starts.

The DMG is signed with the Developer ID of Watzon Ventures LLC and notarized by Apple. Each release also has a `.sha256` file. To check your download:

```sh
shasum -a 256 -c Sayso-<version>-macos-arm64.dmg.sha256
```

### Linux

1. Download `sayso-<version>-linux-<arch>.tar.gz` from the [Releases page](https://github.com/watzon/sayso/releases).
2. Unpack it and run `./install.sh`. Sayso goes to `~/.local`. To install it for all users, with the udev rule for Wayland key input, run `sudo ./install.sh --system`.
3. Open Sayso from your app menu. Onboarding starts.

[docs/linux.md](docs/linux.md) has the details for each desktop.

## First start

1. Choose a model and download it. The default, Parakeet Unified, is about 1.3 GB.
2. Allow the microphone and Accessibility. Sayso needs Accessibility to paste text into other apps.
3. Press Option+Space (Ctrl+Alt+Space on Linux), speak, and press it again.

## Privacy

With a local model and the Raw style, Sayso sends nothing off the computer. There is no telemetry.

| You turn on | What leaves the computer | Where it goes |
|---|---|---|
| A cloud model | The audio of each dictation and your dictionary words | The speech provider that you added |
| An AI style | The transcript, the style prompt, and your dictionary words | The AI provider of that style |

The Models page and the Styles page label each cloud destination. Your API keys are in the macOS Keychain.

## License and price

The source code is free software under the [GNU General Public License, version 3](LICENSE). You can read it, change it, and build it for yourself at no cost.

The official build is the signed and notarized app. It is free, and it needs no license key. Sayso is paid for by the people who use it: the download page at [justsayso.app](https://justsayso.app) asks what you want to pay, and $0 is a valid answer.

The name Sayso and the app icon are not part of the license. If you publish a changed version, give it another name and another icon.

## Build from source

You need Xcode 16 or later and Rust 1.98.1. `rust-toolchain.toml` selects the Rust version.

```sh
git clone https://github.com/watzon/sayso.git
cd sayso
SAYSO_SIGN_IDENTITY=- scripts/bundle.sh
open build/Sayso.app
```

`SAYSO_SIGN_IDENTITY=-` makes an ad hoc signature, which needs no Apple developer account. With an ad hoc signature, macOS asks for the microphone and Accessibility again after each rebuild. To keep the grants, set `SAYSO_SIGN_IDENTITY` to the name of your own signing certificate.

[CONTRIBUTING.md](CONTRIBUTING.md) has the development commands, the checks, and the layout of the code.

## Files on disk

| Kind | Default location |
|---|---|
| Config (`config.toml`, `styles/`) | macOS: `~/Library/Application Support/Sayso`, or `~/.config/sayso` when it exists. Linux: `~/.config/sayso`. `$XDG_CONFIG_HOME/sayso` on both. |
| History, audio, models | macOS: `~/Library/Application Support/Sayso`. Linux: `~/.local/share/sayso`. `$XDG_DATA_HOME/sayso` on both. |
| Logs | macOS: `~/Library/Caches/Sayso/logs/sayso.log`. Linux: `~/.cache/sayso/logs/sayso.log`. `$XDG_CACHE_HOME/sayso/logs` on both. |

API keys are in the Keychain (macOS) or the Secret Service keyring (Linux) under `dev.sayso.Sayso`.

## Help and contributions

- To report a bug or ask for a feature, open an [issue](https://github.com/watzon/sayso/issues).
- To report a security problem, follow [SECURITY.md](SECURITY.md). Do not open a public issue.
- To change the code, read [CONTRIBUTING.md](CONTRIBUTING.md).

## More documents

- [docs/plan.md](docs/plan.md): decisions and milestones.
- [docs/research.md](docs/research.md): research and sources.
- [docs/releasing.md](docs/releasing.md): how to make a release.
