# Porting Sayso to another platform

The terms are in [CONTEXT.md](../CONTEXT.md). This file says where the platform code of the Go app lives and what a new platform adds.

## What each platform adds

| Part | macOS | Windows | Linux | What a new platform adds |
|---|---|---|---|---|
| Platform services (`Services` in `internal/platform/platform.go`) | `*_darwin.go` | `*_windows.go` | `*_linux.go` | `new_<os>.go`, which builds `Services`, and one file for each interface it fills |
| Speech engine | Swift sidecar, `native/macos/SaysoEngine` | Go sidecar, `cmd/sayso-engine` (sherpa-onnx, whisper.cpp) | Go sidecar, `cmd/sayso-engine` (sherpa-onnx, parakeet.cpp) | The native libraries that `internal/localengine` loads with purego, and a catalog in `internal/localengine/catalog/` |
| Model catalog | `internal/models/catalog.go`, the Apple list | `internal/localengine/catalog/catalog_windows.json` | `internal/localengine/catalog/catalog_linux.json` | A catalog file and its embed |
| Secrets | `internal/providers/secrets_darwin.go` (Keychain) | `secrets_windows.go` (Credential Manager) | `secrets_unix.go` (Secret Service) | A `secrets_<os>.go` |
| Cleanup runtime | `cmd/sayso-cleanup` with the llama.cpp libraries of `internal/llamalib` | the same | the same | A pin for the platform in `internal/llamalib` |
| Packaging | `scripts/bundle.sh`, DMG | `scripts/bundle-windows.ps1`, Inno Setup | `scripts/bundle-linux.sh`, tarball. `scripts/package-linux.sh` makes the `.deb`, the `.rpm`, the AppImage, and the Flatpak from it. `flake.nix` is the Nix package | A script and a job in `release.yml` |

Windows and Linux run the same Go engine (ADR 0004). Only the native libraries and the catalog differ.

## Seams

- **`internal/platform`**: every service is an interface in `platform.go`. A field of `Services` that a platform cannot fill stays nil, and the app leaves the feature out (for example `AppIcons`). The build tags are the file names (`_darwin`, `_windows`, `_linux`), so `go test ./...` builds everywhere.
- **`internal/paths`**: the folders without XDG variables, and the Host API socket (`socket.go`). A system without its own branch gets the XDG defaults.
- **`internal/config/hotkey.go`**: the keycap words, the config names of the modifiers, and the default hotkeys of each system.
- **`internal/engine/client.go`**: the file name of the engine (`EngineProgram`), next to the app (`DefaultEnginePath`). `[advanced] engine_path` in `config.toml` replaces it.
- **`internal/ui`**: a few `runtime.GOOS` checks for words and layout (the title bar, the onboarding steps of permissions).

## Known gaps on Windows

- The build is not code signed unless the release has a certificate (`WINDOWS_CERTIFICATE_PFX`). Unsigned downloads get a SmartScreen warning.
