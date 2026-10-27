# sayso-engine-sdk

Run a Sayso speech engine from your own app. You do not need Sayso.

The engine is a separate program. It turns audio into text on the computer of the user. This crate starts the engine, sends it requests with the [engine protocol](https://github.com/watzon/sayso/blob/main/docs/engine-protocol.md), and gives you the replies and events. The calls block and the events come on a channel, so you need no async runtime. One `Engine` works from many threads.

The engine is `SaysoEngine` on macOS and `sayso-engine` on Linux and Windows. It has the GPL-3.0-only license. This crate has the MIT license and only starts the engine as a program, so your app is not a derived work of the engine.

## Get the engine

Each Sayso release has the engine as its own file:

- `sayso-engine-<version>-macos-arm64.tar.gz`
- `sayso-engine-<version>-linux-x86_64.tar.gz` and `sayso-engine-<version>-linux-arm64.tar.gz`
- `sayso-engine-<version>-windows-x64.zip`

Unpack the file next to your app. The file holds the folder `sayso-engine-<version>`, with the program, a README, and the licenses. Keep the licenses with the program when you ship it. `find_engine` finds the program in the folder that you unpacked.

## Example

```rust
use sayso_engine_sdk::{Engine, EngineOptions, TranscribeOptions, find_engine};

fn main() -> Result<(), sayso_engine_sdk::Error> {
    // The folder where you unpacked the release file.
    let program = find_engine("engine")?;
    // The folder for the model files. The engine makes it when it is not there.
    let engine = Engine::start(EngineOptions::new(program, "models"))?;
    println!("engine {}, features: {:?}", engine.version(), engine.features());

    // The engine has a catalog of models. The ids are stable.
    let catalog = engine.list_models()?;
    for model in &catalog.models {
        println!("{} ({} bytes)", model.id, model.size_bytes);
    }

    let model = "parakeet-tdt-v3";
    engine.download_with(model, None, |progress| println!("download: {:.0}%", progress.fraction * 100.0))?;
    engine.load(model)?;

    // A WAV file: 16 kHz, mono, 16-bit. Use `transcribe_f32` or `transcribe_i16` for samples in memory.
    let transcript = engine.transcribe(model, "talk.wav", &TranscribeOptions::default().with_segments())?;
    println!("{}", transcript.text);
    for segment in transcript.segments.unwrap_or_default() {
        println!("{} to {} ms: {}", segment.start_ms, segment.end_ms, segment.text);
    }
    Ok(())
} // The engine stops when it is dropped.
```

## What you get

- Every request of the protocol as a method: `list_models`, `check_models`, `download`, `delete`, `load`, `unload`, `prepare`, `transcribe`, `stream_start`, `stream_audio_*`, `stream_end`, `vad`, `vad_start`, `diarize`, `language_model`, `generate`, and `shutdown`. `Engine::call` sends any request.
- Errors with the `code` and the `reason` of the engine: `error.code()` is `Some(ErrorCode::UnknownModel)` for a model that the catalog does not have.
- Events (`model_state`, `download_progress`, `part_state`, `partial`, `speech_start`, `speech_end`, `log`) on `Engine::events()`. A call such as `download_with` also gives its own events to a function.
- A timeout for each kind of call, which you can change with `Engine::set_timeouts`.
- `Engine::features()`: check a feature before you use an optional request. The Swift engine and the Rust engine have different features.
- `wav::TempWav` and `wav::write_i16`: WAV files in the format that the engine reads.

## Good to know

- Do not let two engines use one models folder at the same time.
- A drop of the `Engine` closes stdin, waits up to 2 seconds for the engine to exit, and then kills it. `Engine::shutdown` sends `shutdown` first.
- The engine writes free text to stderr. By default it goes to the stderr of your app. `EngineOptions::stderr(Stderr::lines(..))` sends each line to a function, and `Stderr::Null` discards it.
- Events wait in the channel until you read them. Read them when you use streams.
- The macOS engine needs macOS 14 or later on Apple silicon. The Linux engine needs the Vulkan loader (`libvulkan.so.1`).

## License

MIT. See `LICENSE`.
