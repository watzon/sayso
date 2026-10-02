# Sayso

Local dictation for macOS. You press a key, speak, and Sayso writes the text into the app you are using. Speech recognition runs on this Mac on the Neural Engine. Nothing leaves the Mac unless you turn on an AI style with a cloud provider.

- Design: Paper file "Sayso — v0.1 Design".
- Decisions and milestones: [docs/plan.md](docs/plan.md).
- Research and sources: [docs/research.md](docs/research.md).
- Terms: [CONTEXT.md](CONTEXT.md).

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

The first start shows onboarding:

1. Choose a model and download it (about 1.3 GB for Parakeet Unified). Onboarding continues when the download is done.
2. Allow the microphone and Accessibility.
3. Press Option+Space, speak, and press Option+Space again.

`scripts/bundle.sh` signs with the Developer ID identity, so permission grants stay valid after a rebuild. Set `SAYSO_SIGN_IDENTITY` to use another identity.

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

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-deps.sh                     # dependency rules from docs/plan.md §4
cargo test -p sayso-engine-client -- --ignored   # real engine; needs downloaded models
cargo test -p sayso-enhance --test real_cli -- --ignored   # real claude command; needs a login
```

## Layout

| Path | Contents |
|---|---|
| `crates/sayso-core` | Domain logic: config, paths, dictation state machine, pipeline, styles, dictionary, inks, model catalog, stats. No platform or UI code. |
| `crates/sayso-platform` | Platform traits: hotkeys, text insertion, context, permissions, audio, speech engine, sounds, login item. |
| `crates/sayso-platform-macos` | macOS implementations: Carbon hotkeys, listen-only event tap, clipboard paste, AVFoundation permissions, cpal capture, window glue. |
| `crates/sayso-engine-client` | Rust client for the engine sidecar (NDJSON over stdio, restart on crash). |
| `native/macos/SaysoEngine` | Swift sidecar: Parakeet (FluidAudio) and Whisper (WhisperKit) on the Neural Engine. |
| `crates/sayso-store` | SQLite history and dictionary, FLAC audio, retention. |
| `crates/sayso-enhance` | AI styles: OpenAI-compatible HTTP, Claude CLI, Codex CLI. |
| `crates/sayso-ui` | The paper design system on GPUI and gpui-kit. |
| `crates/sayso-app` | The app: model, dictation controller, overlay, popover, Hub, onboarding. |
| `assets/` | Fonts (OFL), textures, sounds, icons, and the scripts that make them. |
| `spikes/` | The throwaway prototypes from M0. |

## Files on disk

| Kind | Default location |
|---|---|
| Config (`config.toml`, `styles/`) | `~/Library/Application Support/Sayso`, or `~/.config/sayso` when it exists, or `$XDG_CONFIG_HOME/sayso` |
| History, audio, models | `~/Library/Application Support/Sayso`, or `$XDG_DATA_HOME/sayso` |
| Logs | `~/Library/Caches/Sayso/logs/sayso.log`, or `$XDG_CACHE_HOME/sayso/logs` |

`config.toml` reloads when you save it. API keys are in the Keychain under `dev.sayso.Sayso`.
