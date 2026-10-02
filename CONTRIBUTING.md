# Contributing to Sayso

This file tells you how to build Sayso, run it during development, check a change, and send it.

## Before you start

- For a bug fix, open a pull request directly.
- For a new feature or a change in behavior, open an issue first. [docs/plan.md](docs/plan.md) lists what is in scope, and a pull request outside that scope will probably not be merged.
- Use the terms in [CONTEXT.md](CONTEXT.md) in code, comments, and UI text.
- Your contribution is licensed under the GPL, version 3, the same as the rest of the code.

## Requirements

- macOS 14 or later on Apple Silicon.
- Xcode 16 or later (Swift 6 toolchain) for the speech engine.
- Rust 1.98.1. `rust-toolchain.toml` selects it.

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

`SAYSO_FAKE_MIC=/path/to/16k-mono.wav` replaces the microphone with a WAV file played in real time, for end-to-end tests without a microphone grant. `SAYSO_ENGINE_PATH` selects another engine binary.

`swift scripts/shot.swift sayso /tmp/shot.png 800` takes a screenshot of the largest Sayso window.

## Checks

Run these before you open a pull request:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-deps.sh                     # dependency rules from docs/plan.md §4
```

These tests need models, API keys, or a login, so they do not run by default:

```sh
cargo test -p sayso-engine-client -- --ignored   # real engine; needs downloaded models
SAYSO_REAL_MODELS=whisper-tiny cargo test -p sayso-engine-client --test real_models -- --ignored --nocapture   # download and run catalog models; "all" runs every model
SAYSO_TEST_GROQ_KEY=... cargo test -p sayso-transcribe --test real -- --ignored --nocapture   # real cloud providers; one variable per provider
cargo test -p sayso-enhance --test real_cli -- --ignored   # real claude command; needs a login
```

### What CI runs

CI runs `scripts/check-deps.sh` on each push to `main` and on each pull request. It does not run the tests or clippy, because a GPUI build takes minutes. A maintainer starts the test job by hand with `gh workflow run ci.yml`. So run the checks above on your Mac, and say in the pull request which checks you ran.

## Layout

| Path | Contents |
|---|---|
| `crates/sayso-core` | Domain logic: config, paths, dictation state machine, pipeline, styles, dictionary, inks, model catalog, stats. No platform or UI code. |
| `crates/sayso-platform` | Platform traits: hotkeys, text insertion, context, permissions, audio, speech engine, sounds, login item. |
| `crates/sayso-platform-macos` | macOS implementations: Carbon hotkeys, listen-only event tap, clipboard paste, AVFoundation permissions, cpal capture, window glue. |
| `crates/sayso-engine-client` | Rust client for the engine sidecar (NDJSON over stdio, restart on crash). |
| `native/macos/SaysoEngine` | Swift sidecar for the local models: Parakeet, Nemotron, Cohere, Canary, SenseVoice, Paraformer (FluidAudio), Whisper (WhisperKit), and Apple Speech (macOS 26). |
| `crates/sayso-store` | SQLite history and dictionary, FLAC audio, retention. |
| `crates/sayso-enhance` | AI styles: OpenAI-compatible HTTP, Claude CLI, Codex CLI. |
| `crates/sayso-transcribe` | Cloud models: OpenAI, Groq, Mistral, ElevenLabs, Deepgram, AssemblyAI, and any OpenAI-compatible server. |
| `crates/sayso-ui` | The paper design system on GPUI and gpui-kit. |
| `crates/sayso-app` | The app: model, dictation controller, overlay, popover, Hub, onboarding. |
| `assets/` | Fonts (OFL), textures, sounds, icons, and the scripts that make them. |
| `spikes/` | The throwaway prototypes from M0. |

`scripts/check-deps.sh` enforces the dependency rules between the crates. For example, `sayso-core` must not import GPUI or another Sayso crate.

## Pull requests

- Keep one change in one pull request.
- Add or update a test when you change behavior.
- Do not update `gpui-kit` or the Swift packages in the same pull request as other work. They are pinned to exact versions, and an update needs its own test run.
- Write the commit subject as a command, for example "Add a retry to the Groq provider".
