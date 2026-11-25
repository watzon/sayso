# Contributing to Sayso

This file tells you how to build Sayso, run it during development, check a change, and send it.

## Before you start

- For a bug fix, open a pull request directly.
- For a new feature or a change in behavior, open an issue first. [docs/plan.md](docs/plan.md) lists what is in scope, and a pull request outside that scope will probably not be merged.
- Use the terms in [CONTEXT.md](CONTEXT.md) in code, comments, and UI text.
- Your contribution is licensed under the GPL, version 3, the same as the rest of the code.

## Requirements

- Go (the version in `go.mod`). The app is built with `CGO_ENABLED=0`.
- macOS 14 or later on Apple Silicon, with Xcode 26 or later (Swift 6.2 toolchain) for the speech engine.
- Or Linux (x86_64 or aarch64) with the packages in `scripts/linux-deps.sh` (Debian and Ubuntu names).
- Or Windows 10 or 11 (x64), with the Visual Studio 2022 Build Tools, for the Visual C++ runtime that ships next to the app.

## Build and run

```sh
scripts/dev.sh [flags]          # go run ./cmd/sayso (builds the Swift engine on macOS, the Go engine elsewhere)
scripts/dev.sh --bundle         # macOS: a signed debug Sayso.app, started with open (permission prompts work)
scripts/bundle.sh               # macOS: signed release bundle in build/Sayso.app
scripts/bundle-linux.sh         # Linux: build/sayso-<version>-linux-<arch>.tar.gz
scripts/package-linux.sh build/sayso-*.tar.gz   # .deb, .rpm, AppImage, Flatpak from the tarball
powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1   # Windows: build\windows\Sayso (-Installer for the installer)
```

The release builds use `CGO_ENABLED=0 go build -tags mygo_noinspector -ldflags="-s -w" ./cmd/sayso`. On macOS the Swift engine (`native/macos/SaysoEngine`) goes into `Sayso.app`. On Windows and Linux the Go engine (`cmd/sayso-engine`) loads sherpa-onnx, whisper.cpp, and parakeet at run time; `SAYSO_NATIVE_LIBS_DIR` names the folder with those libraries for the bundle scripts. The cleanup runtime (`cmd/sayso-cleanup`) runs llama.cpp from pinned prebuilt libraries with yzma; the bundle scripts get them with `go run ./scripts/llama-libs fetch` into `build/llama-libs`, once. `scripts/bundle.sh` builds the Swift engine and the MediaRemoteAdapter framework again only when `native/macos` or the tools change, and a debug bundle is signed without secure timestamps.

Always start the macOS bundle with `open`. macOS gives permissions to the app that starts a process, so a bundle started from a terminal uses the terminal's permissions. `scripts/bundle.sh` signs with the Developer ID identity of the maintainer. Set `SAYSO_SIGN_IDENTITY` to your own certificate, or to `-` for an ad hoc signature.

Use temporary folders so your real history stays clean: `export XDG_CONFIG_HOME=/tmp/sayso/c XDG_DATA_HOME=/tmp/sayso/d XDG_CACHE_HOME=/tmp/sayso/k`. `SAYSO_FAKE_MIC=/path/to/16k-mono.wav` replaces the microphone with a WAV file. [docs/linux.md](docs/linux.md) explains the Linux desktops.

## Checks

Run these before you open a pull request:

```sh
CGO_ENABLED=0 go vet ./internal/... ./cmd/...
CGO_ENABLED=0 go test ./internal/... ./cmd/...
(cd native/macos/SaysoEngine && swift test)   # macOS, when you change the Swift engine
```

### What CI runs

CI runs `go vet`, the Go tests, and the release builds of the three programs on macOS, Linux, and Windows on each push to `main` and on each pull request. A maintainer starts the slow jobs (the engine release files and their check) with `gh workflow run ci.yml`.

A pull request that changes the Linux packaging (`packaging/linux`, `scripts/bundle-linux.sh`, `scripts/package-linux.sh`, `scripts/linux-deps.sh`, `scripts/linux-vulkan-deps.sh`, or `linux.yml`) also runs the Linux workflow.

## Layout

| Path | Contents |
|---|---|
| `cmd/sayso`, `cmd/sayso-engine` | The Go/MyGo app, and the Go speech engine for Windows and Linux. |
| `cmd/sayso-cleanup` | The cleanup runtime: llama.cpp through yzma, behind the JSON-lines protocol of the cleanup runtime. Only this program may import yzma (`internal/cleanupd/llama`). |
| `internal/llamalib`, `scripts/llama-libs` | The pins of the llama.cpp libraries, and where the runtime finds them; the tool that downloads them for the bundle scripts and makes new pins. |
| `internal/` | The Go packages of the app: dictation, platform, engine, store, providers, config, update, i18n. |
| `native/macos/SaysoEngine` | Swift sidecar for the local models on macOS (FluidAudio, WhisperKit, Apple Speech). |
| `mobile/` | The Android app and its Go core. |
| `assets/` | The app icons, the DMG background, and the web art. |

[docs/porting.md](docs/porting.md) lists the places where the platforms differ. `cmd/sayso/deps_test.go` keeps yzma and libffi out of the app.

## Pull requests

- Keep one change in one pull request.
- Add or update a test when you change behavior.
- Do not update MyGo or the Swift packages in the same pull request as other work. They are pinned to exact versions, and an update needs its own test run.
- Write the commit subject as a command, for example "Add a retry to the Groq provider".
