# Paths

The rules for the Host API socket, the model folders, and the model store. An SDK that follows them finds a running Sayso, and shares the model files of Sayso in Local mode.

`testdata/paths.json` has a table of cases. Each SDK and the host check every case.

## Host socket

The host binds the first path that applies:

| Order | System | Path |
|---|---|---|
| 1 | any | the environment variable `SAYSO_HOST_SOCKET`, when it is not empty |
| 2 | Linux with `XDG_RUNTIME_DIR` | `$XDG_RUNTIME_DIR/sayso/host.sock`; in a Flatpak (`FLATPAK_ID` is set) `$XDG_RUNTIME_DIR/app/$FLATPAK_ID/host.sock` |
| 3 | macOS | `<user temp dir>/sayso/host.sock`, where the user temp dir is `confstr(_CS_DARWIN_USER_TEMP_DIR)` without its last `/` |
| 4 | other Unix systems, or 2 and 3 without their folder | `/tmp/sayso-<uid>/host.sock` |
| 5 | Windows | the named pipe `\\.\pipe\sayso-host-<SID>`, with the SID of the user, such as `S-1-5-21-...` |

The host makes the folder with mode `0700`. The pipe on Windows has an ACL for the user only.

A client looks in the same place, with one difference: on Linux with `XDG_RUNTIME_DIR` and without `SAYSO_HOST_SOCKET`, a client always looks in `$XDG_RUNTIME_DIR/sayso/host.sock`, also inside a Flatpak.

On macOS, `TMPDIR` is not the user temp dir when a parent process changed it. Use `confstr`, or `/usr/bin/getconf DARWIN_USER_TEMP_DIR` where a language cannot call it.

## Data and models folders

The data folder of Sayso:

| Order | System | Data folder |
|---|---|---|
| 1 | any | `$XDG_DATA_HOME/sayso`, when `XDG_DATA_HOME` is not empty |
| 2 | macOS | `~/Library/Application Support/Sayso` |
| 2 | Windows | `%LOCALAPPDATA%\Sayso`, else `<home>\AppData\Local\Sayso` |
| 2 | Linux and others | `~/.local/share/sayso` |

`XDG_DATA_HOME` counts on every system.

- Models folder: `<data>/models`. Engines get it as `SAYSO_MODELS_DIR`.
- Model store: `<models>/v1`. Engines with the feature `store_v1` get it as `SAYSO_MODEL_STORE`.
- Cleanup models: `<models>/cleanup/<model id>/`, with the model file and `ready.json`.
- Approved apps: `<data>/apps.toml`.

## Model store v1

Engines own the store. An SDK only passes its path. The layout, for reference:

```
v1/
  artifacts/<digest>/                 files that never change
  models/<model id>/<format>.json     the manifest of a model in one format
  locks/meta                          the lock of the metadata
  locks/<digest>.use                  shared while loaded, exclusive to delete
  locks/<digest>.get                  exclusive while one engine downloads the artifact
  staging/<digest>.<pid>.<random>/    a download in progress
  trash/<digest>.<random>/            artifacts that wait to be deleted
```

The digest is the SHA-256, in lowercase hex, of the list of the files of an artifact: for each file, sorted by the bytes of its `/` path, the path, a NUL byte, the size in decimal, a NUL byte, the SHA-256 of the file in lowercase hex, and a line feed.

## Engine program

An engine release folder holds `sayso-engine` (`sayso-engine.exe` on Windows), or folders `sayso-engine-<version>/` that hold it. Of those folders with a program, the one whose name sorts last wins. On x86-64 Linux and Windows, the folder can also hold `sayso-engine-avx` and `sayso-engine-baseline`: the first program that the CPU can run, in the order `sayso-engine` (AVX2), `sayso-engine-avx`, `sayso-engine-baseline`.
