# sayso-engine-client notes

`EngineClient` starts the Swift sidecar `SaysoEngine` (`native/macos/SaysoEngine`) and implements `sayso_platform::SttBackend`.

Build the sidecar: `cd native/macos/SaysoEngine && swift build -c release`.
Find it at run time with `EngineClient::default_engine_path()`: env `SAYSO_ENGINE_PATH`, then `SaysoEngine` next to the app executable, then the dev build path.

## Protocol v1 (NDJSON over stdio)

One JSON object per line. Every message has `"v":1`. The sidecar needs env `SAYSO_MODELS_DIR`. It exits when stdin closes.

Every request with an `id` gets exactly one **reply**: `hello`, `ok`, `final`, or `error`. Events (`model_state`, `download_progress`, `partial`, `log`) can arrive at any time, and they echo the `id` of the request that caused them. `engine` is the `EngineKind` JSON of `sayso-core`.

| Request | Fields | Reply and events |
|---|---|---|
| `hello` | | `hello {version, protocol}` |
| `list_models` | `models:[{id, engine}]` | one `model_state` per model (disk check, no network), then `ok` |
| `download` | `model, engine, size_bytes?` | `model_state downloading` and `download_progress {fraction, bytes_done, bytes_total}` (at most 8 per second), `model_state downloaded`, `ok` |
| `delete` | `model, engine?` | `model_state not_downloaded`, `ok` |
| `load` | `model, engine` | `model_state optimizing`, `model_state ready {load_ms}`, `ok` |
| `stream_start` | `session, model` | `ok`, then `partial {session, committed, tentative}` events |
| `stream_audio` | `session, pcm` (base64 of 16 kHz mono little-endian i16), no `id` | none (errors have no `id`) |
| `stream_end` | `session` | last `partial` if the text changed, `ok {text}` |
| `transcribe` | `model, path` (16 kHz mono WAV), `language, vocabulary` | `final {text, elapsed_ms}` |
| `shutdown` | | `ok`, then exit |

States: `not_downloaded`, `downloading`, `optimizing`, `downloaded`, `ready`, `failed {message}`.
Other events: `log {level, message}`. Errors: `error {message}` with the request `id`.

Notes for the sidecar side:

- `size_bytes` comes from the catalog. FluidAudio reports only a fraction, so `bytes_done` is `fraction * size_bytes`.
- "Downloaded" for Parakeet Unified means three parts: the batch encoder, the `70_2_2` streaming encoder, and the CTC 110M vocabulary helper (`parakeet-ctc-110m-coreml`). `load` needs only the first two.
- Partials from Parakeet are append-only, so `committed` holds the whole text and `tentative` is empty.
- Vocabulary boosting runs in `VocabularyBooster.swift`. FluidAudio 0.17.5 `configureVocabularyBoosting` reads `tokenizer.json` from `~/Library/Application Support/FluidAudio`, outside `SAYSO_MODELS_DIR`. The sidecar rebuilds the same pipeline from public FluidAudio parts and keeps its files in the models directory. It also applies each replacement to the original text, because the library's own rebuilt text drops punctuation and words such as "the".
- Whisper gets the vocabulary as prompt tokens.

## Client behavior

- Blocking calls (`load`, `transcribe`, `delete`, `start_stream`) wait on a per-request channel. Defaults: 15 s for quick calls, 600 s for `load` and `transcribe` (`Options`).
- `download` and `end_stream` return at once. Progress arrives as `EngineEvent::ModelStatus`. The cache behind `status()` updates from `model_state` and `download_progress`.
- `push_audio` converts to i16 and queues the frame. A writer thread encodes and writes it. After 500 queued frames it drops new ones (see `dropped_audio_frames()`).
- Crash: pending calls fail at once. `Crashed` goes out, `Ready` and `Optimizing` models show as `Downloaded`, and the sidecar restarts after 0.5, 1, 2, 4, then 5 s. A recovery thread says hello, refreshes statuses, loads again the models that a caller loaded, then sends `Restarted`. Calls made while the sidecar is down fail at once.
- Live preview sessions do not survive a crash. The app must start a new session. Final transcription of kept audio still works.

## Tests

- `cargo test -p sayso-engine-client`: unit tests for the protocol code, and `tests/fake_engine.rs` against `sayso-fake-engine` (`src/bin/fake_engine.rs`). They cover reply matching with out-of-order replies, temp WAV cleanup, broadcast to two subscribers, the status cache from `download_progress`, crash then `Crashed`, restart, model reload and `Restarted`, fast failure of a pending call, fast failure while the sidecar is down, shutdown on drop, and a non-blocking `push_audio`.
- `cargo test -p sayso-engine-client --test real_engine -- --ignored --nocapture`: the real sidecar and the spike models (linked read-only into `target/engine-it-models`). It downloads the CTC helper once (about 100 MB). It lists, loads, transcribes with and without vocabulary, and streams `short.wav` in 160 ms chunks.

## Not tested

- Downloading the two big Parakeet encoders from scratch through the client (the sidecar was run by hand for the EOU and Whisper small downloads, and for the CTC helper).
- Whisper large-v3 and turbo, and a Whisper or EOU run through the Rust client.
- Download resume after a network error or a kill in the middle of a download.
- Sidecar crash while a real model loads or while a real download runs.
- Running inside a signed app bundle (the Core ML cache path changes, so the first load compiles again).
