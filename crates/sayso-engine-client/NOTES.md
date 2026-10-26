# sayso-engine-client notes

`EngineClient` starts the Swift sidecar `SaysoEngine` (`native/macos/SaysoEngine`) and implements `sayso_platform::SttBackend`.

Build the sidecar: `cd native/macos/SaysoEngine && swift build -c release`.
Find it at run time with `EngineClient::default_engine_path()`: env `SAYSO_ENGINE_PATH`, then `SaysoEngine` next to the app executable, then the dev build path.

## Windows and Linux

The sidecar there is the Rust engine `sayso-engine` (`crates/sayso-engine`): `cargo build -p sayso-engine --release` gives `target/release/sayso-engine(.exe)`. The app looks for it next to its own binary; a debug app with a release engine needs `SAYSO_ENGINE_PATH` (`scripts/dev.ps1` sets it). It speaks the same protocol (`docs/engine-protocol.md`) and runs the kinds `sherpa`, `parakeet_cpp` (Linux), and `whisper_cpp` (Windows) of `sayso_core::models_onnx`. A `sherpa` model with `redecode_preview` (Parakeet TDT v3) gets its live preview by decoding the growing recording again, so `committed` grows and `tentative` holds the re-decoded tail. Windows used its own engine before 0.12 (`SaysoEngine.exe`, `native/portable`). Build steps, runtime files, and measured results are in `crates/sayso-engine/NOTES.md`.

## Protocol

The protocol is specified in `docs/engine-protocol.md`. This section has only what the spec leaves to the client and to each engine.

### Features

`hello` lists the optional parts of the protocol in `features`. The Swift engine lists `language_model`, `prepare`, and `unload`. The Rust engine lists `keep_preview` and `unload`. The client does not read `features` yet: it sends `prepare`, `unload`, and `language_model` to each engine and handles the `error` reply of an engine that does not have them.

### Optional part preparation

The Swift engine loads the stream part only when `preview` is true. For Parakeet Unified,
it also loads the CTC assets and builds a booster for the exact requested word list.
Preparation and real requests share those loads. Preparation runs no inference.
Unload closes the shared parts and rejects results that finish after unload.
The Rust engine accepts `prepare` as a no-op. An older engine can reply
with `error`; the client logs the failure and keeps the model usable.

`SttBackend::part_readiness(model, true)` returns preview readiness. Pass `false` for
the dictionary helper. The values are `Unprepared`, `Preparing`, `Ready`, and `Failed`.
The client tracks the latest request for each model and ignores older events. Unload
and an engine restart clear the record. No overlay text changes with this protocol.

### Reason codes

The app translates the `reason` code (`crates/sayso-app/src/engine_text.rs`, catalog `engine.en.yml`, keys `engine.<reason>`) and shows `message` only as a detail, or when there is no usable `reason`. An engine that sends no `reason` (an older engine) still works: the app shows a general sentence with `message`. A `reason` that the client does not know gives the same result. The client reads the code into `sayso_core::stt::EngineReason` (`ModelStatus::Failed { message, reason }`, and `Unavailable { reason, message }` from `LanguageModel::availability`). A new code therefore needs a variant in `EngineReason`, a key in `engine.en.yml`, and a line in the table of the spec.

Who sends which code: both engines send `incomplete_download`, `download_failed`, and `load_failed`. The Rust engine sends `no_gpu` (Linux) and `format_changed` (Windows, for the Parakeet files of the Windows engine before 0.12). The Swift engine sends the language model codes.

The `error` reply of a `download` or `load` carries no `reason`. The client keeps the reason of the `model_state failed` event that came just before it, when the message is the same.

### Language model

Only the Swift engine has `language_model` and `generate`: it runs the language model of macOS (Apple Intelligence, `LanguageModel.swift`, macOS 26 and later) with guided generation, so the reply is bound to one `text` field. The client waits 500 ms longer than `timeout_ms` before it gives up. The Rust engine answers `error` (unknown request type), which the client reports as "not set up". `EngineClient` implements `sayso_core::enhance::LanguageModel` with these two requests.

### Engine kinds

The Swift engine runs these kinds (`engine.kind`, with their fields): `parakeet_unified {streaming_tier}`, `parakeet_eou`, `parakeet_tdt {version}` (`v2`, `v3`, `ultra`, `redux`, `phonon2`, `tdt_ctc_110m`, `ja`, `fluid_mini`), `nemotron {chunk_ms}`, `nemotron_multilingual {chunk_ms}`, `cohere`, `canary`, `sense_voice`, `paraformer`, `whisper {variant}`, `apple_speech`. The Rust engine runs `sherpa`, `parakeet_cpp` (Linux), and `whisper_cpp` (Windows). The kind `remote` (a cloud model) never reaches an engine: `sayso-app` sends that audio with `sayso-transcribe`.

The client sends each model the nearest `language` value the model supports (`ModelInfo::language_for`).

### Unload

The Swift engine ends the live preview sessions of the model. In the Rust engine a stream or a final pass that runs keeps its part of the model until it ends. The Rust engine on Linux then calls `malloc_trim` (glibc), so the freed memory goes back to the system.

`keep_preview`: the Rust engine holds a model with a streaming archive (for example `parakeet-tdt-v2`) as two parts, and drops only the final pass. A model with a redecode preview (`parakeet-tdt-v3` in the Rust engine) leaves whole: its preview is the final pass. The Swift engine ignores the field and unloads the whole model. It has two parts for Parakeet Unified (the batch manager and the streaming manager), but it does not split them yet.

The client keeps the `preview: true` models in a set behind `SttBackend::preview_ready`. Any other `model_state` of the model, and an engine crash, clear the entry. The status stays `Downloaded`.

`sayso-app` decides when a model leaves memory (`crates/sayso-app/src/memory.rs`): it keeps the active model and the live-preview model in memory, unloads every other model, and unloads the active model after the idle time of `dictation.keep_model_minutes`. When the active model also gives the live preview, that unload has `keep_preview: true`.

Notes for the sidecar side:

- `size_bytes` comes from the catalog. FluidAudio reports only a fraction, so `bytes_done` is `fraction * size_bytes`.
- "Downloaded" for Parakeet Unified means three parts: the batch encoder, the `70_2_2` streaming encoder, and the CTC 110M vocabulary helper (`parakeet-ctc-110m-coreml`). `load` needs only the first two.
- The macOS streaming decoders (Parakeet Unified, Parakeet EOU, Nemotron) only append, so no word in a partial changes before the final pass. `tentative` is the words heard in about the last second, and always the words of the newest update (`PreviewTail.swift`). The overlay shows them in pencil italic. The partial at `stream_end` has all text in `committed`. A split next to Chinese, Japanese, Thai, Lao, Myanmar, or Khmer text can fall between two characters, and the overlay then joins the parts without a space.
- Vocabulary boosting runs in `VocabularyBooster.swift`. FluidAudio 0.17.5 `configureVocabularyBoosting` reads `tokenizer.json` from `~/Library/Application Support/FluidAudio`, outside `SAYSO_MODELS_DIR`. The sidecar rebuilds the same pipeline from public FluidAudio parts and keeps its files in the models directory. It also applies each replacement to the original text, because the library's own rebuilt text drops punctuation and words such as "the". Two settings differ from the library: the acoustic rescue pass is off, and the minimum similarity is 0.60. With a short dictionary the rescue pass put dictionary words in place of ordinary words ("macOS" became "MayFirmOS").
- Whisper gets the vocabulary as prompt tokens. Nemotron multilingual gets it as a decoder bias (`setCustomVocabulary`). The other new families ignore it.
- Parakeet TDT, Nemotron, Cohere, Canary, SenseVoice, and Paraformer each own one folder under the models directory. The sidecar writes `.sayso-downloaded` into the folder when a download ends without an error. Only then is the model "downloaded". FluidAudio creates bundle folders before their files are complete, so the files alone do not prove a complete download.
- FluidAudio reports the download in the first half of its progress fraction. `DownloadReporter.stage` scales it to the full range.
- Fluid Speech Mini (`fluid_mini`) has its own downloader (`FluidSpeechMini.swift`) for the Hugging Face repo `FluidVoice-Labs/fluid-speech-mini-coreml`, pinned to one revision, because `ModelHub` only takes its own `Repo` list. The folder is `fluid-speech-mini-coreml`. It loads as Parakeet Ultra with `AsrModels.loadLocal(from:version: .ultra)`. `AsrModels.load` would read and download FluidAudio's own `parakeet-ultra` folder instead. FluidAudio wants the joint model as `JointDecisionv3.mlmodelc`, so the sidecar adds a symbolic link to `JointDecision.mlmodelc`.
- Parakeet TDT-CTC 110M loads with CPU-only compute units. On the Neural Engine its fused preprocessor and encoder returned only blanks (M5 Pro, macOS 27), with no error. On the CPU it runs at more than 100 times real time.
- Parakeet Redux needs about six minutes for its first load (the Neural Engine compile of the 2-bit encoder). Later loads take less than a second.
- Nemotron models and Parakeet EOU stream and also do the final pass. The final pass feeds the whole recording through the same streaming manager, after the live preview of that dictation ended (`transcribeStreaming` waits for it).
- SenseVoice and Paraformer take one window of about 30 s. The sidecar cuts a longer recording at the quietest point in the last 5 s of each 25 s piece (`AudioChunks`).
- SenseVoice uses the fp32 encoder (`SenseVoice.swift`). The int8 and fp16 encoders compute in fp16, and on about 10% of clear speech every logit was NaN, so the text was empty. The fp32 encoder takes a fixed 1800 frames, which FluidAudio's `SenseVoiceManager` does not send, so the sidecar runs the models itself. A folder without the fp32 encoder counts as not downloaded, and a new download removes the old int8 encoder.
- Apple Speech (`AppleSpeech.swift`, macOS 26 and later) uses `SpeechAnalyzer`. macOS downloads and stores its assets, one set per language. "Downloaded" means that macOS has the assets of at least one language. `download` installs the language of this Mac. A final pass in another language installs that language first. `delete` returns an error, because the files belong to macOS. `auto` means the language of this Mac, because the model cannot detect a language. Dictionary words go in as contextual strings, but a test with "Kubernetes" showed no effect, so the catalog does not claim vocabulary support. It has no live preview yet.
- Cohere Transcribe cannot detect the language. `auto` means English.
- Language codes: Nemotron multilingual maps a plain code to its prompt key (`ja` to `ja-JP`). SenseVoice takes `zh`, `yue`, `en`, `ja`, `ko`, else detection.

## Client behavior

- Blocking calls (`load`, `transcribe`, `delete`, `start_stream`) wait on a per-request channel. Defaults: 15 s for quick calls, 600 s for `load` and `transcribe` (`Options`).
- `download`, `unload`, and `end_stream` return at once. Progress arrives as `EngineEvent::ModelStatus`. `unload` changes a `Ready` status to `Downloaded` at once, and the recovery thread does not load that model again. The cache behind `status()` updates from `model_state` and `download_progress`.
- `push_audio` converts to i16 and queues the frame. A writer thread encodes and writes it. After 500 queued frames it drops new ones (see `dropped_audio_frames()`).
- Crash: pending calls fail at once. `Crashed` goes out, `Ready` and `Optimizing` models show as `Downloaded`, and the sidecar restarts after 0.5, 1, 2, 4, then 5 s. A recovery thread says hello, refreshes statuses, loads again the models that a caller loaded, then sends `Restarted`. Calls made while the sidecar is down fail at once.
- Live preview sessions do not survive a crash. The app must start a new session. Final transcription of kept audio still works.

## Tests

- `cargo test -p sayso-engine-client`: unit tests for the protocol code, `tests/conformance.rs` (the core and ready levels of `sayso-engine-conformance` against the fake engine), and `tests/fake_engine.rs` against `sayso-fake-engine` (`src/bin/fake_engine.rs`). They cover reply matching with out-of-order replies, temp WAV cleanup, broadcast to two subscribers, the status cache from `download_progress`, crash then `Crashed`, restart, model reload and `Restarted`, `unload` (status at once, no reload after a restart), fast failure of a pending call, fast failure while the sidecar is down, shutdown on drop, and a non-blocking `push_audio`.
- `cargo test -p sayso-engine-client --test real_engine -- --ignored --nocapture`: the real sidecar and the spike models (linked read-only into `target/engine-it-models`). It downloads the CTC helper once (about 100 MB). It lists, loads, transcribes with and without vocabulary, and streams `short.wav` in 160 ms chunks.

- `SAYSO_REAL_MODELS=<ids or all> cargo test -p sayso-engine-client --test real_models -- --ignored --nocapture`: for each model, download into `target/engine-it-models`, load, final pass on `short.wav` and `long30.wav`, and a stream plus a final pass for the streaming models. English models must hear a known word in each clip. The other models must return without an error.

## Not tested

- Downloading the two big Parakeet Unified encoders from scratch through the client.
- Speech in a language other than English. The test clips are English, so the Japanese and Mandarin models and the language hints have no accuracy check.
- macOS 14: the catalog hides the models that need macOS 15 (`min_macos`), but no run on macOS 14 confirms the list.
- Download resume after a network error or a kill in the middle of a download.
- Sidecar crash while a real model loads or while a real download runs.
- Running inside a signed app bundle (the Core ML cache path changes, so the first load compiles again).
