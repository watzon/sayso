# Cleanup runtime notes

`sayso-cleanup` is the process that runs the local cleanup models (S1-mini, Cleanup 1). It knows no model. It loads one GGUF file with llama.cpp and completes raw text with greedy decoding. The app builds the prompts and checks the answers. See `docs/cleanup-models.md`, "Runtime and protocol".

This folder is its own Cargo workspace (package `sayso-cleanup`, library `sayso_cleanup`, binary `sayso-cleanup`), with its own `Cargo.lock` and `target`. The app workspace does not build llama.cpp. `rust-toolchain.toml` of the repository applies here too.

## Build

```text
cd native/cleanup
cargo build --release
```

The result is `target/release/sayso-cleanup` (`.exe` on Windows).

- `llama-cpp-2` is pinned to `=0.1.160` with default features off. That means no OpenMP (no `libgomp` on Linux), no `common` (llama.cpp's command-line helpers; the crate builds and runs without it), and no Android C++ runtime.
- On macOS arm64 the `metal` feature is on. Everywhere else the runtime uses the CPU.
- Build needs: a C++ compiler, CMake, and libclang (for the bindgen step of `llama-cpp-sys-2`). On Windows use the Visual Studio Build Tools and, with a long checkout path, `CMAKE_GENERATOR=Ninja` (see `crates/sayso-engine/NOTES.md`). Windows and Linux builds are not tested yet.

## Protocol

NDJSON, protocol version 1: one JSON object per line on stdin, one reply line on stdout. The Rust types are in `crates/sayso-core/src/cleanup/protocol.rs`. Both sides use them.

- Every request has `v` and `id`. The reply has the same `id`.
- Requests: `hello`, `load {model_id, path, n_ctx}`, `count {model_id, text}`, `complete {model_id, prompt, end, max_tokens}`, `unload`.
- Replies: `hello {version, llama_cpp, backend}`, `loaded {model_id, load_ms}`, `count {tokens}`, `completion {text, stop, tokens_in, tokens_out, ms}`, `unloaded`, `error {code, message}`.
- One request at a time. The runtime handles a request fully before it reads the next line.
- A line that does not parse, or is not UTF-8, gives `error bad_request`. The `id` is the one in the line when it can be read, else `""`. A blank line is skipped. A `v` other than 1 gives `bad_request`, except for `hello`, so a client can always read the version.
- `hello`: `llama_cpp` is the version of the `llama-cpp-2` crate (`0.1.160`). The crate gives no llama.cpp build number. `backend` is `metal` or `cpu`.
- `load`: unloads any other model first, then loads the file. Failure gives `load_failed`, and then no model is loaded. A load of the same `model_id`, `path`, and `n_ctx` that is already loaded answers `loaded` with `load_ms: 0`. A different `n_ctx` loads again. `n_ctx` 0 gives `bad_request`. The runtime checks nothing about the file but that llama.cpp can load it (the app checks the hash).
- One context of `n_ctx` tokens lives as long as the model. Each `complete` clears its KV cache first, so a request never sees an older one. The batch size is `n_ctx`, so one decode takes the whole prompt.
- `count`: tokens of `text` with `add_special = false` and `parse_special = true`.
- `complete`: the prompt is tokenized with the BOS rule of the model and `parse_special = true`. Decoding is greedy.
  - `stop: "end"`: an end-of-generation token, or one of the `end` strings in the text so far. The text is cut before the `end` string. Neither the string nor the token is in `text`.
  - `stop: "limit"`: `max_tokens` tokens were made.
  - `stop: "context"`: the prompt plus `max_tokens` does not fit in `n_ctx`. Nothing is decoded and `text` is empty. This is a normal reply, not an error.
  - `text` is not trimmed. The runtime collects bytes and converts to UTF-8 (lossy) at the end, because one token can be half a character.
- `not_loaded` and `wrong_model` come from `count` and `complete`. `decode_failed` is a llama.cpp decode error or a panic.

## Threads, stdout, logs

- The main thread reads stdin and handles each request. There is no other Rust thread. llama.cpp starts its own compute threads: on the CPU backend up to 8 (the number of cores, at most 8); with Metal the llama.cpp default.
- The backend starts at the first `load`, not at the start. `hello`, bad lines, and `unload` are cheap.
- At start the runtime duplicates the real stdout for the protocol and points file descriptor 1 at stderr (on Windows also the Win32 standard output handle). The code is `src/stdio.rs`, the same as `Output::protocol_stdout` in `crates/sayso-engine/src/protocol.rs`. So a print from ggml or Metal cannot corrupt the stream. Stdout carries protocol lines only.
- A panic while it handles a request becomes `error decode_failed` and the process keeps running (`catch_unwind`). The panic message goes to stderr.
- Logs go to stderr, level from `SAYSO_CLEANUP_LOG` (default `info`: one line at start, one per load and unload). llama.cpp and ggml logs are dropped, also during the backend start (the Metal setup writes about 30 lines). `SAYSO_CLEANUP_LOG=debug` (or `trace`) keeps them.
- End of stdin exits with code 0. A write error on stdout exits with code 1. The client kills the process to cancel a request (see `docs/cleanup-models.md`).

## What to ship

The app finds the runtime in the folder of its own executable (`Runtime::default_path` in `crates/sayso-cleanup-client/src/runtime.rs`). So every package puts `sayso-cleanup` beside the app, where the engine is. `docs/releasing.md`, "What ships where", has the table.

- **macOS arm64:** `Sayso.app/Contents/MacOS/sayso-cleanup`. The bundle script (`scripts/bundle.sh`) builds it in release mode, strips it, and signs it with the Developer ID, the hardened runtime, no entitlements (it records no audio), and the identifier `dev.sayso.Sayso.cleanup`. The notarization of the app covers it. One binary, 11 MB (10 MB stripped). `otool -L` shows only system libraries: CoreFoundation, Foundation, Metal, MetalKit, Accelerate, `libc++`, `libSystem`, and `libobjc`. llama.cpp and ggml are linked statically, and the Metal shaders are embedded in the binary. The first start on a Mac compiles them, about 4 s. A stripped binary with an ad hoc hardened-runtime signature loads a model and completes on Metal (checked 2026-10-21). Not checked: the Developer ID signature and the notarization of a real release.
- **Windows x86_64:** `sayso-cleanup.exe` in the install folder (`scripts/bundle-windows.ps1`), signed with the exes when the certificate secrets exist. The Visual C++ runtime that the bundle ships for the other two exes serves this one too.
- **Linux x86_64 and arm64:** `bin/sayso-cleanup` in the tarball (`scripts/bundle-linux.sh`). The `.deb`, `.rpm`, AUR package, and Nix package put it in `/usr/lib/sayso`, beside `sayso`. The AppImage and the Flatpak copy the `bin` folder, so they have it in `usr/bin` and `/app/bin`. `scripts/package-linux.sh` checks its glibc and libstdc++ symbols against the floor in `packaging/linux/nfpm.yaml`, as for the other binaries. The license of the linked llama.cpp has the same text as `ggml.txt`, which the tarball already ships.

Windows and Linux builds are not run locally. The CI job **Cleanup runtime** (`.github/workflows/ci.yml`, manual) builds, tests, and lints on all four targets, and prints the size and the libraries of each binary. It fails when the binary needs OpenMP (`libgomp`, `libomp`, `vcomp`) or, on macOS, a library that is not part of the system. Expect on Windows and Linux a binary linked statically to llama.cpp, with CPU kernels only, that needs the C++ runtime (Windows: the Visual C++ 2015-2022 runtime; Linux: `libstdc++`, `libgcc_s`, and glibc). Read the first CI output and write the sizes here. The Linux packages workflow (`linux.yml`) also starts the installed runtime and checks for `hello` with the `cpu` backend.

## Tests

```text
cd native/cleanup
cargo test
cargo clippy --all-targets -- -D warnings
```

- Unit tests: the line handling (bad lines, version, panic) and the stop-string search.
- `tests/protocol.rs`: starts the built binary and checks `hello`, bad lines, `not_loaded`, `load_failed` (missing file and a file that is no model), `bad_request`, and the exit on end of input. No model file needed.
- `tests/models.rs`: needs the model files, so it is `#[ignore]`. Set `SAYSO_CLEANUP_S1` (the path of `s1-mini-q4_k_m.gguf`) and `SAYSO_CLEANUP_SPOKENLY` (the path of `cleanup-1-q8_0.gguf`):

```text
SAYSO_CLEANUP_S1=~/.cache/sayso-cleanup-eval/models/s1-mini-q4_k_m.gguf \
SAYSO_CLEANUP_SPOKENLY=~/.cache/sayso-cleanup-eval/models/cleanup-1-q8_0.gguf \
cargo test -- --ignored --test-threads=1 --nocapture
```

It checks the answers of both models for the meeting sentence, the `limit` and `context` stops, a prompt of 1,200 tokens, `count`, `wrong_model`, `not_loaded` after `unload`, a no-op `load`, a swap of models, and that two completions in a row give the same text. With `--nocapture` it prints the load and completion times.

## Measured (2026-10-21, M5 Pro, release build, `n_ctx` 2048)

| | S1-mini q4_k_m | Cleanup 1 q8_0 |
|---|---|---|
| `load` | 330 to 730 ms | 330 to 680 ms |
| `complete`, meeting sentence | 77 to 145 ms (93 tokens in, 13 out) | 86 to 140 ms (24 in, 12 out) |

Not measured: memory, a cold start after a reboot, a speech model loaded at the same time, and any CPU-only target.
