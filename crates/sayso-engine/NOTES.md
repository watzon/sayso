# sayso-engine notes

The speech engine sidecar for Linux and Windows. It speaks NDJSON protocol v1 (`crates/sayso-engine-client/NOTES.md`), the same as the Swift engine on macOS. `docs/linux.md` covers the Linux side. This file covers Windows and the redecode live preview.

Windows used its own engine before 0.12: `SaysoEngine.exe`, a separate Cargo workspace in `native/portable` (Parakeet on ONNX Runtime through transcribe-rs, Whisper on whisper.cpp). 0.12 replaced it with this crate. ADR 0004 (`docs/adr/0004-one-engine-for-windows-and-linux.md`) gives the reasons. The old engine is in the history before that change.

## What runs where

| Engine kind | Linux | Windows |
|---|---|---|
| `sherpa` (sherpa-onnx, CPU) | yes | yes |
| `parakeet_cpp` (parakeet.cpp, Vulkan GPU) | feature `parakeet` (default) | no |
| `whisper_cpp` (whisper.cpp through transcribe-rs 0.3.12, CPU) | feature `whisper-cpp`, for tests only | always |

The catalogs are in `crates/sayso-core/src/models_onnx.rs` (`catalog_for`). Windows has Whisper on whisper.cpp; Linux has Whisper on sherpa-onnx. Both systems run Parakeet TDT v3 with a redecode live preview.

## Build on Windows

```text
cargo build -p sayso-engine --release
```

The result is `target\release\sayso-engine.exe`. `scripts\dev.ps1` and `scripts\bundle-windows.ps1` build it.

Prerequisites: Visual Studio 2022 Build Tools (MSVC and a Windows SDK), CMake, and libclang (for the bindgen step of `whisper-rs-sys`). `scripts\windows-env.ps1` finds them. With a long checkout path (a worktree), the Visual Studio generator of CMake can fail on the 260-character path limit; use Ninja (`CMAKE_GENERATOR=Ninja`), and after a failed configure delete `target\release\build\whisper-rs-sys-*\out\build`.

Network at build time: the build script of `sherpa-onnx-sys` downloads the prebuilt sherpa-onnx 1.13.8 libraries into `target\sherpa-onnx-prebuilt`. whisper.cpp builds from the sources in `whisper-rs-sys`.

### Why sherpa-onnx is a DLL on Windows

The Windows libraries of sherpa-onnx are built with the static C runtime (`/MT`), both the static and the shared archive. whisper.cpp (`whisper-rs-sys`, through the `cmake` crate) builds with the DLL C runtime (`/MD`), as Rust does by default. One exe that links both static archives fails with LNK2038 (`RuntimeLibrary` mismatch). So on Windows sherpa-onnx uses its `shared` feature: `sherpa-onnx-c-api.dll` and `onnxruntime.dll` keep their own C runtime, and the exe links only their import library.

The exe delay-loads `sherpa-onnx-c-api.dll` (`build.rs`). Windows loads it at the first call, after `Output::protocol_stdout` pointed stdout at stderr, so the C runtime inside the DLL starts with stderr as its stdout and a stray print cannot corrupt the protocol.

## Runtime files to ship on Windows

- `sayso-engine.exe`.
- Every DLL beside `sherpa-onnx-c-api.dll` in `target\sherpa-onnx-prebuilt\...\lib` (at least `sherpa-onnx-c-api.dll` and `onnxruntime.dll`). The build script of `sherpa-onnx-sys` also copies them to `target\release`, so a development run finds them.
- The Visual C++ 2015-2022 runtime (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`, `msvcp140_1.dll`), which the app ships beside the exes.

Size (release build, 2026-10-23): `sayso-engine.exe` 3.9 MB, `onnxruntime.dll` 17.8 MB, `sherpa-onnx-c-api.dll` 4.6 MB, `sherpa-onnx-cxx-api.dll` 0.3 MB, `onnxruntime_providers_shared.dll` 0.1 MB. Together about 26.7 MB. The `SaysoEngine.exe` of 0.11 was 25.5 MB.

`scripts\bundle-windows.ps1` copies all of these. The installer deletes the old `SaysoEngine.exe` on an update.

## Files of the Windows engine before 0.12

The old engine kept a model's files directly in `<models>\<id>\` with the marker `.sayso-downloaded`. This engine keeps them in `<models>\<id>\<archive>\` with `.complete` (`store.rs`).

- Whisper: the GGML file is the same file. `ModelStore::adopt_legacy` moves it into `main\` and writes `.complete`, at the first `list_models`, `download`, or `load`. Nothing downloads again.
- Parakeet: the old files are the istupakov ONNX export, which sherpa-onnx cannot read. `list_models` reports `failed` with the reason `format_changed`. The app then downloads the model again if it is in use (the dictation model or the live preview model) and deletes it if not (`DictationApp::replace_old_model_files`). A download first removes everything in the folder that is not a complete archive of the model, so the old files go.

## Redecode live preview

Parakeet TDT v3 has no streaming model for its 25 languages. With `redecode_preview` in the catalog, the engine gives the final pass a live preview that decodes the growing recording again (`redecode.rs`, ported from the old Windows engine):

- A worker thread per stream decodes the uncommitted audio about every 600 ms when at least 300 ms of new audio arrived.
- When the uncommitted audio passes 10 s, the worker cuts at the quietest 100 ms window between 7 s and 2 s before the end, decodes the part before the cut one last time, and commits its text. A decode never covers more than about 10 s of audio, because the cost of a decode grows faster than its length.
- When a decode takes longer than the interval, the next one waits half the decode time, so the preview uses at most about two thirds of the processor time.
- The preview and the final pass share one model. While a final pass runs, the worker starts no decode, and `stream_end` skips its last decode.
- The stream worker of the engine asks the stream for text at each `stream_audio`. New committed text comes back as an ended segment, and the engine asks again at once for the tentative text, so both go out in one `partial`.
- `unload` with `keep_preview` unloads all of such a model: the preview is the final pass.

### Measurements

Windows 11, i9-13900K, the old engine (same preview logic, Parakeet v3 on ONNX Runtime), 2026-10-19:

- Decode cost: 10 s of audio 395 to 423 ms, 20 s 2329 to 2736 ms, 29 s 3786 to 4222 ms.
- A 93 s recording streamed at speaking speed: with a commit point of 10 s, 90 decodes, mean 679 to 687 ms, longest 1038 to 1099 ms, 70 partials, about one update per second. With 20 s, the longest decode was 2 s and the updates came every 1.8 s. The last preview text had the same 252 words as the final pass with both.
- Words lost or added at the commit points against one final pass: none for 10 s (keep 2 s, search 5 s). A search of 4 s cut "export" into "ex" and "port" where no pause was found.

Windows 11, i9-13900K, this engine against the old engine of 0.11, the same clips (8.1 s and 29.2 s), run in turns in the same session, 2026-10-23:

| | Load | 8.1 s clip | 29.2 s clip |
|---|---|---|---|
| Old engine, Parakeet v3 (istupakov export, ONNX Runtime through transcribe-rs) | 978 to 1053 ms | 296 to 315 ms | 3469 to 4006 ms |
| This engine, Parakeet v3 int8 (sherpa-onnx, 8 threads) | 1374 to 1400 ms | 264 to 293 ms (one 479 ms) | 2066 to 2093 ms |
| This engine, Whisper base (whisper.cpp) | 305 ms | 682 to 738 ms | 1620 to 1798 ms |

- The final pass of Parakeet uses 8 threads (`final_pass_threads`). On this processor (8 P-cores, 16 E-cores) 4 threads took about 2850 ms for the 29.2 s clip, 16 threads about 2780 ms, and 24 or more threads took 4 s and longer.
- The redecode preview of the 8.1 s clip streamed at speaking speed: the first partial at 0.85 s, then one about every 0.69 s. The committed text was the same as the final pass. A final pass right after `stream_end` took 787 ms.

Linux arm64 in Docker on an Apple M-series Mac, this engine (sherpa-onnx, Parakeet v3 int8), 2026-10-23:

- Download 487 MB in 22 s, load 930 ms. Final pass: 3.9 s clip in 123 ms, 24.9 s clip in 503 ms.
- The 3.9 s clip streamed in 160 ms chunks at speaking speed: the first partial 1.3 s after the stream started, then one about every 650 ms. The last partial had the full sentence of the final pass.

## Tests

- `cargo test -p sayso-engine`: unit tests with a fake recognizer (protocol, downloads against a local server, legacy files, the redecode logic with synthetic speech), and the client test of the built binary.
- `cargo test -p sayso-engine --no-default-features --features whisper-cpp` builds whisper.cpp on Linux and runs the same tests with it.
- `tests/real_models.rs` (ignored) downloads real models and runs them through `EngineClient`. On Windows its default models are `parakeet-tdt-v3` and `whisper-base`. It needs `SAYSO_IT_AUDIO_DIR`: a folder with `short.wav` (16 kHz mono, with the word "dictation") and `long30.wav` (about 30 s, with "deployment"). The first-partial time it prints is measured when the test reads the events, after the stream ends, so it is not the latency of the preview.

## Not tested yet

- The packaged Windows app (`bundle-windows.ps1` and the installer) and the update from 0.11 through the app. The engine alone was tested on Windows on 2026-10-23: the release build, the delay-load (`sherpa-onnx-c-api.dll` is a delay import, and the exe does not import `onnxruntime.dll`), legacy Whisper adoption and the Parakeet `format_changed` download with a copy of real 0.11 files, transcription, the preview, `unload`, and `delete`.
- Resume of an interrupted download: this engine starts a file again; the old Windows engine resumed it with a `Range` request.
