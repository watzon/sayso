# Contributing to Sayso

This file tells you how to build Sayso, run it during development, check a change, and send it.

## Before you start

- For a bug fix, open a pull request directly.
- For a new feature or a change in behavior, open an issue first. [docs/plan.md](docs/plan.md) lists what is in scope, and a pull request outside that scope will probably not be merged.
- Use the terms in [CONTEXT.md](CONTEXT.md) in code, comments, and UI text.
- Your contribution is licensed under the GPL, version 3, the same as the rest of the code.

## Requirements

- macOS 14 or later on Apple Silicon, with Xcode 16 or later (Swift 6 toolchain) for the speech engine.
- Or Linux (x86_64 or aarch64) with the packages in `scripts/linux-deps.sh` (Debian and Ubuntu names).
- Or Windows 10 or 11 (x64), with the Visual Studio 2022 Build Tools ("Desktop development with C++" and a Windows SDK), and CMake and libclang for the speech engine (`LIBCLANG_PATH`; see [native/portable/NOTES.md](native/portable/NOTES.md)).
- Rust 1.98.1. `rust-toolchain.toml` selects it.

The macOS commands follow. Windows has its own section after them.

## Build and run

```sh
# The speech engine (Swift, WhisperKit + FluidAudio). Needed once, and after engine changes.
(cd native/macos/SaysoEngine && swift build -c release)

# Signed app bundle in build/Sayso.app (release build).
scripts/bundle.sh
open build/Sayso.app
```

Always start the bundle with `open`. macOS gives permissions to the app that starts a process, so a bundle started from a terminal uses the terminal's permissions.

`scripts/bundle.sh` signs with the Developer ID identity of the maintainer. Set `SAYSO_SIGN_IDENTITY` to the name of your own certificate, or to `-` for an ad hoc signature. A certificate keeps the permission grants after a rebuild. An ad hoc signature does not.

### Development runs

`scripts/dev.sh <flags>` builds a debug bundle, signs it, starts it with `open`, and shows the log in the terminal. Press Ctrl+C to quit Sayso. Use this script when you test onboarding or dictation. Permission prompts and grants then belong to Sayso.

```sh
scripts/dev.sh --onboarding
```

`cargo run -p sayso-app -- <flags>` is faster, but the app gets the terminal's permissions, and macOS does not ask for the microphone. Use it for screens that do not need permissions. The permission screens show a warning when Sayso runs outside a bundle.

Use temporary folders so your real history stays clean. Both ways pass these variables to the app:

```sh
export XDG_CONFIG_HOME=/tmp/sayso/c XDG_DATA_HOME=/tmp/sayso/d XDG_CACHE_HOME=/tmp/sayso/k
cargo run -p sayso-app -- --seed-demo --route=history --dark
```

| Flag | Effect |
|---|---|
| `--hub` | Open the Hub at startup. |
| `--route=home\|history\|dictionary\|styles\|models\|settings-<page>` | Open the Hub at a page. Pages: general, dictation, overlay, audio, appearance, history, permissions, advanced. |
| `--onboarding[=N]` | Open onboarding at step N (0 to 6). |
| `--popover` | Open the menu bar popover after launch. |
| `--dark`, `--light` | Force the appearance. |
| `--seed-demo` | Fill an empty database with the sample content from the design. |

### Linux

```sh
scripts/linux-deps.sh                          # once, on Debian or Ubuntu
cargo build -p sayso-app -p sayso-engine       # the app finds the engine next to it
cargo run -p sayso-app -- --hub
scripts/bundle-linux.sh                        # build/sayso-<version>-linux-<arch>.tar.gz
```

On Linux the permissions are capabilities, not grants, so a terminal run behaves like an installed app. [docs/linux.md](docs/linux.md) explains the desktops, the permissions, and the `sayso --toggle` command. `SAYSO_UI_BACKEND=x11` or `=wayland` forces the display server of the UI.

`SAYSO_FAKE_MIC=/path/to/16k-mono.wav` replaces the microphone with a WAV file played in real time, for end-to-end tests without a microphone grant. `SAYSO_ENGINE_PATH` selects another engine binary.

`swift scripts/shot.swift sayso /tmp/shot.png 800` takes a screenshot of the largest Sayso window.

### Windows

```powershell
# The portable speech engine (Rust: transcribe-rs with ONNX Runtime and whisper.cpp).
# Needed once, and after engine changes. It is its own Cargo workspace.
cargo build --release --manifest-path native\portable\Cargo.toml

# The app folder in build\windows\Sayso (release build). -Installer also makes the installer.
powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1
```

For a development run, use `scripts\dev.cmd <flags>` (or `./scripts/dev.sh <flags>` in Git Bash, which hands over to it). It builds the engine and the debug app, quits a Sayso that still runs from `target\debug`, starts Sayso, and shows the log in the terminal. Press Ctrl+C to quit Sayso. It finds CMake, Ninja, and libclang in their usual places (`scripts\windows-env.ps1`).

```powershell
scripts\dev.cmd --onboarding
```

Windows ties no permission to the app, so `cargo run -p sayso-app -- <flags>` also works for everything, dictation too. It finds the engine in `native\portable\target\release`. The flags and the `XDG_*` variables above work the same:

```powershell
$env:XDG_CONFIG_HOME="$env:TEMP\sayso\c"; $env:XDG_DATA_HOME="$env:TEMP\sayso\d"; $env:XDG_CACHE_HOME="$env:TEMP\sayso\k"
cargo run -p sayso-app -- --seed-demo --route=history --dark
```

A debug build opens a console window with the log; a release build has none. Only one Sayso runs at a time: a second start opens the Hub of the first.

`powershell -ExecutionPolicy Bypass -File scripts\shot.ps1 sayso $env:TEMP\shot.png` takes a screenshot of the largest Sayso window (`-Screen` for the whole display).

## Checks

Run these before you open a pull request:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-deps.sh                     # dependency rules from docs/plan.md §4
```

On Windows, also test the speech engine:

```powershell
cargo test --manifest-path native\portable\Cargo.toml
```

The tests with the fake `claude` and `codex` scripts and the Swift engine test run on macOS only.

These tests need models, API keys, or a login, so they do not run by default:

```sh
cargo test -p sayso-engine-client -- --ignored   # real engine; needs downloaded models
SAYSO_REAL_MODELS=whisper-tiny cargo test -p sayso-engine-client --test real_models -- --ignored --nocapture   # download and run catalog models; "all" runs every model
SAYSO_TEST_GROQ_KEY=... cargo test -p sayso-transcribe --test real -- --ignored --nocapture   # real cloud providers; one variable per provider
cargo test -p sayso-enhance --test real_cli -- --ignored   # real claude command; needs a login
```

### What CI runs

CI runs `scripts/check-deps.sh` on each push to `main` and on each pull request. It does not run the tests or clippy, because a GPUI build takes minutes. A maintainer starts the test jobs (macOS, Linux, and Windows) by hand with `gh workflow run ci.yml`. So run the checks above on your computer, and say in the pull request which checks you ran, and on which platform.

## Layout

| Path | Contents |
|---|---|
| `crates/sayso-core` | Domain logic: config, paths, dictation state machine, pipeline, styles, dictionary, inks, model catalog, stats. No platform or UI code. |
| `crates/sayso-platform` | Platform traits: hotkeys, text insertion, context, permissions, audio, speech engine, sounds, login item. |
| `crates/sayso-platform-common` | The portable parts of the platform crates: cpal capture, the resampler, rodio sounds, the Esc detector, the paste receipt rules. |
| `crates/sayso-platform-macos` | macOS implementations: Carbon hotkeys, listen-only event tap, clipboard paste, AVFoundation permissions, window glue. |
| `crates/sayso-platform-linux` | Linux implementations for X11 and Wayland: key grabs, the shortcuts portal, evdev, clipboard paste, key injection, the tray icon, window glue. |
| `crates/sayso-platform-windows` | Windows implementations: RegisterHotKey chords, low-level keyboard hook, clipboard paste with delayed rendering, privacy settings, cpal capture, Win32 window glue. |
| `crates/sayso-engine-client` | Rust client for the engine sidecar (NDJSON over stdio, restart on crash). |
| `native/macos/SaysoEngine` | Swift sidecar for the local models on macOS: Parakeet, Nemotron, Cohere, Canary, SenseVoice, Paraformer (FluidAudio), Whisper (WhisperKit), and Apple Speech (macOS 26). |
| `crates/sayso-engine` | Rust sidecar for the local models on Linux: Parakeet, Whisper, SenseVoice, Moonshine, and a streaming Zipformer, on sherpa-onnx. |
| `native/portable` | Rust sidecar for the local models on Windows: Parakeet (ONNX Runtime) and Whisper (whisper.cpp) through transcribe-rs. Same protocol as the Swift sidecar. |
| `crates/sayso-store` | SQLite history and dictionary, FLAC audio, retention. |
| `crates/sayso-enhance` | AI styles: OpenAI-compatible HTTP, Claude CLI, Codex CLI. |
| `crates/sayso-transcribe` | Cloud models: OpenAI, Groq, Mistral, ElevenLabs, Deepgram, AssemblyAI, and any OpenAI-compatible server. |
| `crates/sayso-ui` | The paper design system on GPUI and gpui-kit. |
| `crates/sayso-app` | The app: model, dictation controller, overlay, popover, Hub, onboarding. `src/shell` holds the glue for each system. |
| `assets/` | Fonts (OFL), textures, sounds, icons, and the scripts that make them. |
| `spikes/` | The throwaway prototypes from M0. |

[docs/porting.md](docs/porting.md) lists the places where the platforms differ.

`scripts/check-deps.sh` enforces the dependency rules between the crates. For example, `sayso-core` must not import GPUI or another Sayso crate.

## Pull requests

- Keep one change in one pull request.
- Add or update a test when you change behavior.
- Do not update `gpui-kit` or the Swift packages in the same pull request as other work. They are pinned to exact versions, and an update needs its own test run.
- Write the commit subject as a command, for example "Add a retry to the Groq provider".
