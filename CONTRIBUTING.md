# Contributing to Sayso

This file tells you how to build Sayso, run it during development, check a change, and send it.

## Before you start

- For a bug fix, open a pull request directly.
- For a new feature or a change in behavior, open an issue first. [docs/plan.md](docs/plan.md) lists what is in scope, and a pull request outside that scope will probably not be merged.
- Use the terms in [CONTEXT.md](CONTEXT.md) in code, comments, and UI text.
- Your contribution is licensed under the GPL, version 3, the same as the rest of the code. The exception is the MIT crates (ADR 0002), which are the crates in `crates/` and the packages in `packages/`: a contribution to these crates is MIT-licensed.

## Requirements

- Go (the version in `go.mod`). The app is built with `CGO_ENABLED=0`.
- macOS 14 or later on Apple Silicon, with Xcode 26 or later (Swift 6.2 toolchain) for the speech engine.
- Or Linux (x86_64 or aarch64) with the packages in `scripts/linux-deps.sh` (Debian and Ubuntu names).
- Or Windows 10 or 11 (x64), with the Visual Studio 2022 Build Tools, CMake, and LLVM (libclang; set `LIBCLANG_PATH`) for the cleanup runtime.
- Rust 1.98.1 for the SDK crates and the cleanup runtime. `rust-toolchain.toml` selects it.
- Node 22 or later and pnpm for the TypeScript SDK.

## Build and run

```sh
scripts/dev.sh [flags]          # go run ./cmd/sayso (builds the Swift engine on macOS, the Go engine elsewhere)
scripts/dev.sh --bundle         # macOS: a signed debug Sayso.app, started with open (permission prompts work)
scripts/bundle.sh               # macOS: signed release bundle in build/Sayso.app
scripts/bundle-linux.sh         # Linux: build/sayso-<version>-linux-<arch>.tar.gz
scripts/package-linux.sh build/sayso-*.tar.gz   # .deb, .rpm, AppImage, Flatpak from the tarball
powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1   # Windows: build\windows\Sayso (-Installer for the installer)
```

The release builds use `CGO_ENABLED=0 go build -tags mygo_noinspector -ldflags="-s -w" ./cmd/sayso`. On macOS the Swift engine (`native/macos/SaysoEngine`) goes into `Sayso.app`. On Windows and Linux the Go engine (`cmd/sayso-engine`) loads sherpa-onnx, whisper.cpp, and parakeet at run time; `SAYSO_NATIVE_LIBS_DIR` names the folder with those libraries for the bundle scripts. The cleanup runtime (`cmd/sayso-cleanup`) runs llama.cpp from pinned prebuilt libraries with yzma; the bundle scripts get them with `go run ./scripts/llama-libs fetch` into `build/llama-libs`, once. No app build needs Cargo. `scripts/bundle.sh` builds the Swift engine and the MediaRemoteAdapter framework again only when `native/macos` or the tools change, and a debug bundle is signed without secure timestamps.

Always start the macOS bundle with `open`. macOS gives permissions to the app that starts a process, so a bundle started from a terminal uses the terminal's permissions. `scripts/bundle.sh` signs with the Developer ID identity of the maintainer. Set `SAYSO_SIGN_IDENTITY` to your own certificate, or to `-` for an ad hoc signature.

Use temporary folders so your real history stays clean: `export XDG_CONFIG_HOME=/tmp/sayso/c XDG_DATA_HOME=/tmp/sayso/d XDG_CACHE_HOME=/tmp/sayso/k`. `SAYSO_FAKE_MIC=/path/to/16k-mono.wav` replaces the microphone with a WAV file. [docs/linux.md](docs/linux.md) explains the Linux desktops.

## Checks

Run these before you open a pull request:

```sh
CGO_ENABLED=0 go vet ./internal/... ./cmd/...
CGO_ENABLED=0 go test ./internal/... ./cmd/...
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/check-deps.sh                     # the SDK crates only use each other as documented
node packages/sdk/scripts/build-addon.mjs && cargo build -p sayso-sdk --example sayso-fake-engine
pnpm install --frozen-lockfile && pnpm build && pnpm test
```

### What CI runs

CI runs the dependency rules, the Go tests on macOS, Linux, and Windows, the Rust SDK tests with clippy, and the TypeScript SDK tests on each push to `main` and on each pull request. A maintainer starts the slow jobs (cleanup runtime, engine files) with `gh workflow run ci.yml`.

A pull request that changes the Linux packaging (`packaging/linux`, `scripts/bundle-linux.sh`, `scripts/package-linux.sh`, `scripts/linux-deps.sh`, `scripts/linux-vulkan-deps.sh`, or `linux.yml`) also runs the Linux workflow.

## Layout

| Path | Contents |
|---|---|
| `cmd/sayso`, `cmd/sayso-engine` | The Go/MyGo app, and the Go speech engine for Windows and Linux. |
| `cmd/sayso-cleanup` | The cleanup runtime: llama.cpp through yzma, behind the JSON-lines protocol of the cleanup runtime. Only this program may import yzma (`internal/cleanupd/llama`). |
| `internal/llamalib`, `scripts/llama-libs` | The pins of the llama.cpp libraries, and where the runtime finds them; the tool that downloads them for the bundle scripts and makes new pins. |
| `internal/` | The Go packages of the app: dictation, platform, engine, store, providers, config, update, i18n. |
| `native/macos/SaysoEngine` | Swift sidecar for the local models on macOS (FluidAudio, WhisperKit, Apple Speech). |
| `native/cleanup` | The Rust cleanup runtime of the SDKs (`sayso-sdk`, `@justsayso/cleanup`, the `sayso-cleanup` release files): the same protocol, llama.cpp linked. Its own Cargo workspace. The app does not use it. |
| `crates/sayso-protocol` | The engine and host protocol types. MIT. |
| `crates/sayso-engine-sdk` | Rust helpers to write an engine. MIT. |
| `crates/sayso-speech` | Speech types and the transcription pipeline pieces. MIT. |
| `crates/sayso-capture` | Microphone and system audio capture. MIT. |
| `crates/sayso-cleanup-models` | What Sayso knows about cleanup models: the catalog, the protocol, the checks. MIT. No I/O. |
| `crates/sayso-cleanup-client` | Starts the cleanup runtime and downloads the model files, for the Rust SDK. MIT. |
| `crates/sayso-sdk` | The SDK for apps: `Speech`, the embedded session with the program engine and the microphone, cleanup, and Deepgram. MIT. |
| `crates/sayso-node` | The native Node addon of `@justsayso/sdk`. MIT. |
| `packages/` | The npm packages: `sdk`, `engine`, `cleanup`. |
| `assets/` | The app icons, the DMG background, and the web art. |

[docs/porting.md](docs/porting.md) lists the places where the platforms differ. `scripts/check-deps.sh` enforces the dependency rules between the crates.

## Pull requests

- Keep one change in one pull request.
- Add or update a test when you change behavior.
- Do not update MyGo or the Swift packages in the same pull request as other work. They are pinned to exact versions, and an update needs its own test run.
- Write the commit subject as a command, for example "Add a retry to the Groq provider".
