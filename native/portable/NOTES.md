# Portable speech engine notes

`SaysoEngine` for Windows and Linux. It speaks NDJSON protocol v1 (`crates/sayso-engine-client/NOTES.md`), so `EngineClient` starts it and talks to it with no changes. It runs Parakeet TDT through ONNX Runtime and Whisper through whisper.cpp, both with [transcribe-rs](https://github.com/cjpais/transcribe-rs) 0.3.12 (the library of the Handy dictation app).

This folder is its own Cargo workspace (package `sayso-engine`, binary `SaysoEngine`), with its own `Cargo.lock` and `target`. The app workspace does not build ONNX Runtime or whisper.cpp. `rust-toolchain.toml` of the repository applies here too.

## Build

```text
cd native/portable
cargo build --release
```

The result is `target/release/SaysoEngine.exe` (Windows) or `target/release/SaysoEngine` (Linux), the development path of `EngineClient::default_engine_path()`.

Prerequisites:

- Windows: Visual Studio 2022 Build Tools (MSVC and a Windows SDK), CMake, and libclang (for the bindgen step of `whisper-rs-sys`; without it the build uses bundled bindings and warns). Set `LIBCLANG_PATH` when `libclang.dll` is not on `PATH`.
  - With a long checkout path (a worktree under `.claude/worktrees/...`), the default Visual Studio generator of CMake fails: MSBuild writes `.tlog` files past the 260-character path limit. Use Ninja: `CMAKE_GENERATOR=Ninja`. After a failed configure, delete `target/release/build/whisper-rs-sys-*/out/build` so CMake forgets the old generator.
  - Tested with CMake 4.4.3 and Ninja from pip (`python -m pip install --user cmake ninja`, which puts them in `%APPDATA%\Python\Python310\Scripts`) and the `libclang.dll` of the Swift toolchain.
- Linux (not built yet, see below): `build-essential cmake pkg-config libssl-dev clang libclang-dev`. `libssl-dev` is for the build script of `ort-sys`, which fetches ONNX Runtime with `ureq` and native TLS. `SaysoEngine` itself uses rustls.
- Network at build time: `ort-sys` (feature `download-binaries`, on by default in transcribe-rs) downloads a prebuilt ONNX Runtime 1.24.2 from pyke's CDN (`cdn.pyke.io`) into `%LOCALAPPDATA%\ort.pyke.io` (Linux: `~/.cache/ort.pyke.io`) and checks its SHA-256. At run time the engine only downloads from Hugging Face.

`ort` is pinned to `2.0.0-rc.12` in `Cargo.lock`, the version in the lock file of transcribe-rs 0.3.12. Cargo would otherwise pick rc.13.

## Runtime files to ship

ONNX Runtime is linked statically (`onnxruntime.lib` from the pyke build). whisper.cpp is linked statically. On Windows the pyke build includes the DirectML execution provider, so the exe imports `DirectML.dll` (not delay-loaded). Ship beside `SaysoEngine.exe`:

| File | Size | Where it comes from |
|---|---|---|
| `SaysoEngine.exe` | 25 MB | `target/release` |
| `DirectML.dll` | 18.5 MB (1.15.4) | The `ort-sys` build script (feature `copy-dylibs`) copies it to `target/release` on every build. The source is `%LOCALAPPDATA%\ort.pyke.io\dfbin\x86_64-pc-windows-msvc\<hash>\DirectML.dll`. |

The exe also needs the Visual C++ 2015-2022 runtime (`VCRUNTIME140.dll`, `VCRUNTIME140_1.dll`, `MSVCP140.dll`, `MSVCP140_1.dll`): install `vc_redist.x64.exe`, or copy the four DLLs beside the exe. `d3d12.dll` and `dxgi.dll` come with Windows.

Windows 11 has `DirectML.dll` 1.15.5 in `System32`, and the loader would take it when the app folder has none. Older Windows 10 builds have an older one, so ship the copy. The loader looks in the folder of the exe first, so a `DirectML.dll` beside `SaysoEngine.exe` (in `target/release`, or in the app folder) is the one that loads.

Linux: one binary, statically linked to ONNX Runtime and whisper.cpp; it needs `libstdc++` and glibc.

## Protocol coverage

Every request of protocol v1: `hello`, `list_models`, `download`, `delete`, `load`, `stream_start`, `stream_audio`, `stream_end`, `transcribe`, `shutdown`. Engine kinds: `onnx` with family `parakeet`, and `whisper_cpp`. They are parsed with the `EngineKind` of `sayso-core`.

- Threads: `main` reads stdin. `stream_start`, `stream_audio`, and `stream_end` run in arrival order on one thread. Every other request gets a thread. `shutdown` replies `ok` and exits. End of stdin exits at once (a download in progress stays as `*.partial` and resumes next time).
- stdout carries protocol lines only. At start the engine duplicates the real stdout for the protocol and points stdout at stderr (C descriptor 1, and on Windows also the Win32 standard output handle), like the Swift engine, so a print in whisper.cpp or ONNX Runtime cannot corrupt the stream. Logs go to stderr (`env_logger`, level from `SAYSO_ENGINE_LOG`, default `info,ort=warn,transcribe_rs=warn`), and the ready line also goes out as a `log` event.
- Errors: `error {message}` with the request `id`. Invalid JSON or bytes that are not UTF-8 give an `error` without `id`. A panic in a request (or in a model) becomes an `error`, and a poisoned model lock is recovered. `stream_audio` for an unknown session reports the error once per session, not for every frame.
- `list_models`: disk only. `ready` when loaded, `downloading` while a download runs, else `downloaded` or `not_downloaded`. A model kind this engine cannot run (a macOS kind) gets `model_state failed {message}`, and the other models still get their state, so `EngineClient::spawn` does not fail on one bad entry.
- `download`: each model owns `SAYSO_MODELS_DIR/<model id>/`. Files stream into `<file>.partial` and are renamed when their size matches `Content-Length`. `.sayso-downloaded` goes in after every file succeeded; only a folder with the marker and every file is "downloaded". `bytes_total` is the sum of the `Content-Length` of a HEAD request per file (redirects followed), or `size_bytes` while a size is unknown. `download_progress` and `model_state downloading` go out at most 8 times per second, the fraction never goes backwards, and the first and last message are always sent. A file that already has its final name is skipped without a request.
  - Resume: a `.partial` file continues with `Range: bytes=<n>-` (206 appends; 200 starts again). Each request reads for at most 120 s, then the next request resumes, which also ends a stalled connection. An attempt that moved the file forward does not count as a failure; three failures in a row without progress fail the download. HTTP 404 and other 4xx fail at once.
  - URLs: `https://huggingface.co/{repo}/resolve/main/{file}` for Parakeet, `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{file}` for Whisper. `SAYSO_HF_ENDPOINT` replaces `https://huggingface.co` (tests use a local server). Model ids, repositories, and file names must be plain names (letters, digits, `.`, `_`, `-`), so they cannot leave the models folder or change the URL.
  - Parakeet files (what `ParakeetModel::load` with `Quantization::Int8` reads): `encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`. `config.json` is not needed.
  - No checksum check. Hugging Face gives the SHA-256 of LFS files in `X-Linked-Etag`; checking it would need a hash crate.
- `load`: `optimizing`, then `ready {load_ms}`. A loaded model answers `ready {load_ms: 0}`. A failed load sends `model_state failed` and `error` (like the Swift engine). One load at a time; a second request for the same model waits and then finds it loaded.
- `transcribe {model, path, language, vocabulary}`: reads the WAV (16-bit PCM from the app; also 8/24/32-bit PCM and 32-bit float, more channels mixed down, other rates resampled linearly), loads the model on demand when it is downloaded (with `optimizing` and `ready` events), and answers `final {text, elapsed_ms}`. `elapsed_ms` counts the inference, from after the file read.
  - Parakeet: language and vocabulary are not used (v3 detects the language; the catalog marks Parakeet without vocabulary). A recording longer than 90 s goes in pieces cut at the quietest point of the last 10 s of each piece, because the encoder's memory grows with the square of the input length.
  - Whisper: `auto` (or no language) lets whisper.cpp detect it. Dictionary words go into the initial prompt as `word, word`, like the WhisperKit prompt tokens on macOS. Beam search with 3 beams (transcribe-rs default), up to 8 threads. Input shorter than 1.25 s is padded with silence (whisper.cpp ignores input under 1 s). Near digital silence (RMS under 0.0005) gives empty text, because Whisper makes up "You" for silence. Tags such as `[BLANK_AUDIO]` are removed.
- `delete`: stops the preview sessions of the model, unloads it, and removes its folder (retrying for about 2 s on Windows, where a file can stay open for a moment). Deleting a model that is not on disk is `ok`. A model with a download or load running is `busy`.
- `stream_start {session, model, language}` loads the model on demand. Only Parakeet has a live preview; Whisper gets an error (the app takes the preview from a Parakeet model). A new session replaces the sessions that run.

## Live preview

transcribe-rs has no streaming decoder for Parakeet, so `preview.rs` emulates one by decoding the recording again:

- `stream_audio` appends to the session buffer. A worker thread decodes the uncommitted audio about every 600 ms when at least 300 ms of new audio arrived, and sends `partial {session, committed, tentative}` only when the text changed.
- Bounded cost: when the uncommitted audio passes 20 s, the worker cuts at the quietest 100 ms window between 12 s and 4 s before the end, decodes the part before the cut one last time, appends its text to `committed`, and drops that audio. `committed` only grows. `tentative` is the text of the remaining 4 to 12 s (up to 20 s), decoded again on each tick. A decode never covers more than about 20 s of audio.
- When a decode takes longer than the interval (a long tail on a slow processor), the next one waits half the decode time, so the preview uses at most about two thirds of the time.
- Final pass first: a running `transcribe` counts as a final pass. While one runs, the worker starts no decode. `stream_end` stops the worker (it ends after the decode that runs now, if any), decodes the new audio one last time unless a final pass waits, sends a last `partial` (all text in `committed`) if the text the client saw last was different, then `ok {session, text}`. The model is shared behind a mutex, so the final pass waits for at most one preview decode.
- The worker logic is behind the `Decoder` trait, so the unit tests run it with synthetic audio and a fake decoder (commit points in silence, no word lost or repeated over 80 s, bounded decode length, pause, prompt stop).

## GPU options (off by default)

The engine runs on the CPU. transcribe-rs has these features; none is on:

- ONNX Runtime: `ort-directml` (Windows; the pyke Windows build already contains the DirectML provider, so this mostly needs the feature and `set_ort_accelerator(OrtAccelerator::DirectMl)`), `ort-cuda`, `ort-tensorrt`, `ort-rocm`, `ort-webgpu`, `ort-xnnpack`. CUDA and TensorRT make `ort-sys` download a different, much larger ONNX Runtime build and need the CUDA libraries at run time.
- whisper.cpp: `whisper-vulkan` (Windows and Linux, needs the Vulkan SDK to build), `whisper-cuda` (CUDA Toolkit 12). With such a feature, `WhisperLoadParams::use_gpu` follows `transcribe_rs::get_whisper_accelerator()`, which this engine already passes on.

## Tests

- `cargo test`: 50 unit tests (protocol parsing and replies, request dispatch with a fake recognizer, download bookkeeping against a local `tiny_http` server with HEAD, Range, 500s, and a body that stops halfway, WAV reading, the preview logic) and two integration tests of the built binary: `tests/protocol.rs` (raw NDJSON: hello, bad input, unknown kinds, errors, a download from a local server, a load of a broken file, delete, shutdown, exit on end of input, no `SAYSO_MODELS_DIR`) and `tests/client.rs` (`EngineClient` from `crates/sayso-engine-client`: spawn, statuses of the whole catalog, errors without models).
- `cargo test --release --test real_models -- --ignored --nocapture --test-threads=1`: downloads Parakeet TDT v3 and Whisper tiny into `target/test-models` (kept for the next run), runs the final pass on `spikes/engine/audio/short.wav` ("dictation") and `long30.wav` ("deployment"), streams `short.wav` in 160 ms chunks at speaking speed through `EngineClient`, then a final pass right after `stream_end`, streams `long30.wav` at twice speaking speed so the preview commits, and measures preview decode cost.
- `cargo clippy --all-targets -- -D warnings` is clean.

## Verified results (2026-10-02)

Windows 11, release build, CPU only, through `EngineClient`:

| | Parakeet TDT v3 (int8) | Whisper tiny |
|---|---|---|
| Download from Hugging Face | 671 MB in 8 s | 78 MB in 1 s |
| Load | 934 to 981 ms | 71 to 72 ms |
| `short.wav` (8.1 s) | 284 to 302 ms (27 to 29x real time) | 319 to 332 ms (25x) |
| `long30.wav` (29.2 s) | 1550 to 1600 ms (18 to 19x) | 789 to 803 ms (36x) |

Two runs; the ranges cover both.

- Both clips are English and both models heard the key words. Parakeet on `short.wav`: "Hello from Seizo. This is a dictation test with GitHub, Kubernetes, and a phone number 5555555555550134." Whisper tiny with the vocabulary `Sayso, Kubernetes` wrote "Sayso" instead of "Seizo".
- Whisper tiny on 2 s of digital silence: "" (before the silence check: "You").
- Preview decode cost (Parakeet, one decode, mean of 3): 2 s of audio 107 to 110 ms, 5 s 221 to 227 ms, 10 s 433 to 446 ms, 20 s 935 to 1045 ms, 29 s 1682 to 1732 ms. So the 20 s commit point keeps a decode near 1 s at most on this processor.
- Live preview of `short.wav` in 160 ms chunks at speaking speed: 13 partials, from "Hello from the" to the full sentence. The last preview text has 100 % of the words of the final pass. The final pass sent right after `stream_end` took 425 to 464 ms in the engine and 434 to 472 ms wall time, so the preview did not hold it up.
- Live preview of `long30.wav` at twice speaking speed: 19 partials, 6 with committed text. The commit fell at a sentence end ("...a container image for every pull request." committed, "Please review the migration plan..." tentative). The last preview text has 100 % of the words of the final pass.

## Not tested

- A Linux build and run. The code has no Windows-only parts except the `cfg(windows)` stdout redirect in `stdio.rs`. Its `cfg(unix)` twin type-checks for `x86_64-unknown-linux-gnu`, but no Linux build of the whole engine ran (the WSL Ubuntu here has no Rust, CMake, or `libssl-dev`).
- The packaged app: `SaysoEngine.exe` and `DirectML.dll` in an installer, on a Windows 10 machine, or without the Visual C++ runtime.
- Parakeet TDT v2, and the larger Whisper models (base to large-v3). Only Parakeet v3 and Whisper tiny ran.
- Speech in other languages; a dictation of several minutes through the preview; a final pass over 90 s (the Parakeet piece split ran with synthetic audio only).
- Resume of a real Hugging Face download after a network error (resume ran against the local server only).
- GPU features.
