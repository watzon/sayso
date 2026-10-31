# Tutorial: build a speech app with the Sayso SDK

In this tutorial you build `scribe`, a small command-line program. It transcribes an audio file. Then it sends the same audio as a live session and prints the text while the text comes in.

The program uses `Mode::Local`. It starts its own engine and never talks to Sayso, so you do not need Sayso on your computer. Later you can change one line to `Mode::Auto` (see [How to react to mode changes](react-to-mode-changes.md)).

We ran this tutorial on a Mac with Apple silicon. The Rust code is the same on Linux and Windows. The engine and the audio tools are different there, and we did not run those steps.

The tutorial takes about 15 minutes. The first run downloads one model of 77 MB.

## What you need

- Rust 1.88 or later. Run `rustc --version` to check.
- A checkout of the Sayso repository. The crates are not on crates.io yet, so your app uses a path dependency for now. In this tutorial, `$SAYSO` is the folder of your checkout. Set it in your shell, for example `export SAYSO=~/src/sayso`.
- The Xcode command line tools, to build the engine. You can use a release file of the engine instead (see [How to ship the engine in your app](ship-the-engine.md)).

## 1. Get the engine

An app that uses `Mode::Local` ships its own engine. The engine is a separate program that runs the models. You get it as a release file: a `.tar.gz` that holds the folder `sayso-engine-<version>`.

Make a work folder for the tutorial:

```sh
mkdir scribe-work
cd scribe-work
```

Build the engine from your checkout and pack it in the same way as a release file:

```sh
(cd "$SAYSO/native/macos/SaysoEngine" && swift build -c release)
"$SAYSO/scripts/package-engine.sh" --out "$PWD/dist" "$SAYSO/native/macos/SaysoEngine/.build/release/SaysoEngine"
```

The first build downloads the Swift packages and takes several minutes. The script prints a warning that the engine has no Developer ID signature. You can ignore it here.

Unpack the file:

```sh
mkdir engine
tar -xzf dist/sayso-engine-*-macos-arm64.tar.gz -C engine
ls engine
```

You see one folder, for example:

```text
sayso-engine-0.14.0
```

## 2. Make a sample file

`scribe` reads a WAV file with 16 kHz, mono, 16-bit audio. On a Mac, make one with `say`:

```sh
say -o talk.wav --data-format=LEI16@16000 "Hello from the Sayso SDK. This is a short test of the guide. The quick brown fox jumps over the lazy dog."
```

To convert a file that you have, use `ffmpeg -i in.m4a -ar 16000 -ac 1 -c:a pcm_s16le talk.wav`.

## 3. Create the project

```sh
cargo new scribe
cd scribe
```

Add the SDK to `Cargo.toml`. Use the path of your checkout:

```toml
[dependencies]
sayso-sdk = { path = "/path/to/sayso/crates/sayso-sdk" }
```

On macOS, add a file `build.rs` next to `Cargo.toml`:

<!-- check: unchecked -->
```rust
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-weak_framework,CoreAudio");
    }
}
```

The audio library inside the SDK needs this line. Without it, the program does not start on macOS 14.0 and 14.1. [How to prepare a macOS app](prepare-a-macos-app.md) explains why.

## 4. Connect

Replace the content of `src/main.rs` with this program:

<!-- check: unchecked -->
```rust
use sayso_sdk::protocol::Capability;
use sayso_sdk::{App, Engine, Mode, Options, Speech};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [engine, file] = args.as_slice() else {
        eprintln!("usage: scribe <engine folder> <16 kHz mono WAV file>");
        return ExitCode::from(2);
    };
    match run(Path::new(engine), Path::new(file)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(engine: &Path, file: &Path) -> Result<(), String> {
    let options = Options::new(Mode::Local, App::new("Scribe"))
        .capabilities([Capability::ModelsDownload, Capability::TranscribeFile, Capability::SessionLive])
        .engine(Engine::from_release(engine));
    let speech = Speech::connect(options).map_err(|e| e.to_string())?;
    eprintln!("mode: {:?}", speech.mode());
    Ok(())
}
```

`Speech::connect` starts the SDK in the mode that you ask for. `Options::capabilities` lists what the app uses. In `Mode::Local`, the SDK does not ask a person about them, but the list stays the same when you change to a mode that does. `Engine::from_release` takes the folder where you unpacked the release file.

Run it. The first path is the engine folder, and the second is the WAV file:

```sh
cargo run -- ../engine ../talk.wav
```

The compiler warns that `file` is unused. You use it in step 6. The program prints:

```text
mode: Local
```

If you give a wrong engine folder, the program prints `cannot read ... (unsupported)` and stops. `Mode::Local` checks that the engine is there before it returns.

## 5. Download a model

The SDK has no model in it. The engine lists the models that it knows, and you ask it to download one. `whisper-tiny` is small and fast, so it suits a first run.

Add a constant below the `use` lines:

<!-- check: verbatim crates/sayso-sdk/examples/scribe.rs -->
```rust
const MODEL: &str = "whisper-tiny";
```

Add the download to `run`, below the line with `eprintln!`:

<!-- check: lines crates/sayso-sdk/examples/scribe.rs -->
```rust
    speech.download_model(MODEL, "To transcribe your files").map_err(|e| e.to_string())?;
```

In `Mode::Local`, `download_model` starts at once and returns when the model is on disk. The second argument is the reason that Sayso shows to its user in `Mode::Sayso`. Write it so a person understands it.

The model goes to the model store of your user. On macOS, the store is `~/Library/Application Support/Sayso/models/v1`. Sayso uses the same store, so a model is on disk once. To keep this tutorial away from your real store, set `XDG_DATA_HOME` to a new folder first. Do this in `scribe-work`, and not in `scribe`:

```sh
export XDG_DATA_HOME="$PWD/data"
```

Run the program again from `scribe`. The first run takes some seconds for the download and prints no progress:

```sh
cargo run -- ../engine ../talk.wav
```

The output is the same as before. Run it a second time. It is fast now, because the model is on disk. Look at the store:

```sh
ls "$XDG_DATA_HOME/sayso/models/v1"
```

```text
artifacts
locks
models
staging
trash
```

## 6. Transcribe a file

A job runs in the background. `transcribe_file` returns the id of the job at once. The result comes as events on `speech.events()`. A job always ends with one `job_done` or one `job_failed`.

Add this function below `run`:

<!-- check: verbatim crates/sayso-sdk/examples/scribe.rs -->
```rust
fn transcribe(speech: &Speech, path: PathBuf) -> Result<String, String> {
    let request = TranscribeFile { path, model: Some(MODEL.into()), ..Default::default() };
    let job = speech.transcribe_file(request).map_err(|e| e.to_string())?;
    for event in speech.events() {
        match event {
            Event::Message(protocol::Event::JobProgress { job: id, fraction }) if id == job => eprintln!("{:.0} %", fraction * 100.0),
            Event::Message(protocol::Event::JobDone { job: id, text }) if id == job => return Ok(text),
            Event::Message(protocol::Event::JobFailed { job: id, message, .. }) if id == job => return Err(message),
            _ => {}
        }
    }
    Err("the events of the SDK ended".into())
}
```

Add these lines at the start of `run`, and these below the download:

<!-- check: lines crates/sayso-sdk/examples/scribe.rs -->
```rust
    let path = std::fs::canonicalize(file).map_err(|e| format!("cannot find {}: {e}", file.display()))?;
```

<!-- check: lines crates/sayso-sdk/examples/scribe.rs -->
```rust
    let text = transcribe(&speech, path.clone())?;
    println!("File: {text}");
```

Replace the `use` lines at the top with these:

<!-- check: unchecked -->
```rust
use sayso_sdk::protocol::{self, Capability, TranscribeFile};
use sayso_sdk::{App, Engine, Event, Mode, Options, Speech};
use std::path::{Path, PathBuf};
```

Run it:

```sh
cargo run -- ../engine ../talk.wav
```

```text
mode: Local
0 %
100 %
File: Hello from the SASOSDK, this is a short test of the guide, the quick brown fox jumps over the lazy dog.
```

The text is not perfect. `whisper-tiny` is the smallest model. A larger model gives a better text ([How to choose a model and download it](choose-and-download-a-model.md)).

The loop reads every event of the SDK, so it skips the events of other work with `_ => {}`. The `if id == job` guards keep the events of this job only.

If the file is not audio, the job ends with `job_failed`, and the program prints the message of the failure. Try it with `echo hi > bad.wav`.

## 7. Run a live session

A live session transcribes audio while it comes in. The audio can come from the microphone (`Source::Mic`), from the sound of other apps (`Source::System`), or from your own code (`Source::Client`). The microphone and the system audio need permissions from the user. `Source::Client` needs none, so we use it here. We send the samples of the file at the speed of real time, as a microphone does.

Add two functions below `transcribe`:

<!-- check: verbatim crates/sayso-sdk/examples/scribe.rs -->
```rust
fn live(speech: &Speech, samples: &[i16]) -> Result<String, String> {
    let request = SessionStart { sources: vec![Source::Client], model: Some(MODEL.into()), ..Default::default() };
    let session = speech.start_session(request).map_err(|e| e.to_string())?.session;
    let silence = [0i16; 16_000];
    for chunk in samples.chunks(1_600).chain(silence.chunks(1_600)) {
        speech.session_audio(session, chunk).map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(100));
        for event in speech.events().try_iter() {
            match event {
                Event::Message(protocol::Event::Partial(partial)) => eprintln!("partial: {}", partial.text()),
                Event::Message(protocol::Event::Segment(segment)) => eprintln!("segment: {}", segment.segment.text),
                _ => {}
            }
        }
    }
    speech.stop_session(session).map_err(|e| e.to_string())
}
```

<!-- check: verbatim crates/sayso-sdk/examples/scribe.rs -->
```rust
/// The samples of a WAV file with 16 kHz, mono, 16-bit audio.
fn read_wav(path: &Path) -> Result<Vec<i16>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let find = |tag: &[u8]| bytes.windows(4).position(|window| window == tag);
    let (Some(format), Some(data)) = (find(b"fmt ").filter(|at| at + 24 <= bytes.len()), find(b"data").filter(|at| at + 8 <= bytes.len())) else {
        return Err(format!("{} is not a WAV file", path.display()));
    };
    let channels = u16::from_le_bytes([bytes[format + 10], bytes[format + 11]]);
    let rate = u32::from_le_bytes([bytes[format + 12], bytes[format + 13], bytes[format + 14], bytes[format + 15]]);
    let bits = u16::from_le_bytes([bytes[format + 22], bytes[format + 23]]);
    if (channels, rate, bits) != (1, 16_000, 16) {
        return Err(format!("{} must be 16 kHz mono 16-bit, and it is {rate} Hz, {channels} channels, {bits} bits", path.display()));
    }
    Ok(bytes[data + 8..].as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect())
}
```

Add the live session to the end of `run`, before `Ok(())`:

<!-- check: lines crates/sayso-sdk/examples/scribe.rs -->
```rust
    let samples = read_wav(&path)?;
    let text = live(&speech, &samples)?;
    println!("Live: {text}");
```

Replace the `use` lines at the top again. The last version of them adds `SessionStart`, `Source`, and `Duration`:

<!-- check: lines crates/sayso-sdk/examples/scribe.rs -->
```rust
use sayso_sdk::protocol::{self, Capability, SessionStart, Source, TranscribeFile};
use sayso_sdk::{App, Engine, Event, Mode, Options, Speech};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
```

`live` does four things:

1. `start_session` starts the session and returns its id.
2. `session_audio` sends 100 ms of samples (1600 samples at 16 kHz) at a time. After the audio, the loop sends one second of silence, so the voice detection sees the end of the last sentence.
3. Between the chunks, the loop reads the events that came in. A `partial` event is the text so far. A `segment` event is a part of the text that will not change.
4. `stop_session` ends the session and returns the full text, after the last events.

Run it:

```sh
cargo run -- ../engine ../talk.wav
```

```text
mode: Local
0 %
100 %
File: Hello from the SASOSDK, this is a short test of the guide, the quick brown fox jumps over the lazy dog.
segment: Hello from the SASOSDK, this is a short test of the guide, the quick brown fox jumps over the lazy dog.
Live: Hello from the SASOSDK, this is a short test of the guide, the quick brown fox jumps over the lazy dog.
```

You see no `partial:` lines, because `whisper-tiny` does not stream. A model with the role `streaming` gives them, for example `parakeet-eou-120m`. If a model like this is on disk, the SDK uses it for the live preview, and you see lines like `partial: good morning did you finish the record`.

## 8. The finished program

This is the complete `src/main.rs`. Compare it with yours. The file is also in the repository as `crates/sayso-sdk/examples/scribe.rs`. A script checks that this page and the file match, line for line.

<!-- check: file crates/sayso-sdk/examples/scribe.rs -->
```rust
use sayso_sdk::protocol::{self, Capability, SessionStart, Source, TranscribeFile};
use sayso_sdk::{App, Engine, Event, Mode, Options, Speech};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const MODEL: &str = "whisper-tiny";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [engine, file] = args.as_slice() else {
        eprintln!("usage: scribe <engine folder> <16 kHz mono WAV file>");
        return ExitCode::from(2);
    };
    match run(Path::new(engine), Path::new(file)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(engine: &Path, file: &Path) -> Result<(), String> {
    let path = std::fs::canonicalize(file).map_err(|e| format!("cannot find {}: {e}", file.display()))?;
    let options = Options::new(Mode::Local, App::new("Scribe"))
        .capabilities([Capability::ModelsDownload, Capability::TranscribeFile, Capability::SessionLive])
        .engine(Engine::from_release(engine));
    let speech = Speech::connect(options).map_err(|e| e.to_string())?;
    eprintln!("mode: {:?}", speech.mode());

    speech.download_model(MODEL, "To transcribe your files").map_err(|e| e.to_string())?;

    let text = transcribe(&speech, path.clone())?;
    println!("File: {text}");

    let samples = read_wav(&path)?;
    let text = live(&speech, &samples)?;
    println!("Live: {text}");
    Ok(())
}

fn transcribe(speech: &Speech, path: PathBuf) -> Result<String, String> {
    let request = TranscribeFile { path, model: Some(MODEL.into()), ..Default::default() };
    let job = speech.transcribe_file(request).map_err(|e| e.to_string())?;
    for event in speech.events() {
        match event {
            Event::Message(protocol::Event::JobProgress { job: id, fraction }) if id == job => eprintln!("{:.0} %", fraction * 100.0),
            Event::Message(protocol::Event::JobDone { job: id, text }) if id == job => return Ok(text),
            Event::Message(protocol::Event::JobFailed { job: id, message, .. }) if id == job => return Err(message),
            _ => {}
        }
    }
    Err("the events of the SDK ended".into())
}

fn live(speech: &Speech, samples: &[i16]) -> Result<String, String> {
    let request = SessionStart { sources: vec![Source::Client], model: Some(MODEL.into()), ..Default::default() };
    let session = speech.start_session(request).map_err(|e| e.to_string())?.session;
    let silence = [0i16; 16_000];
    for chunk in samples.chunks(1_600).chain(silence.chunks(1_600)) {
        speech.session_audio(session, chunk).map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(100));
        for event in speech.events().try_iter() {
            match event {
                Event::Message(protocol::Event::Partial(partial)) => eprintln!("partial: {}", partial.text()),
                Event::Message(protocol::Event::Segment(segment)) => eprintln!("segment: {}", segment.segment.text),
                _ => {}
            }
        }
    }
    speech.stop_session(session).map_err(|e| e.to_string())
}

/// The samples of a WAV file with 16 kHz, mono, 16-bit audio.
fn read_wav(path: &Path) -> Result<Vec<i16>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let find = |tag: &[u8]| bytes.windows(4).position(|window| window == tag);
    let (Some(format), Some(data)) = (find(b"fmt ").filter(|at| at + 24 <= bytes.len()), find(b"data").filter(|at| at + 8 <= bytes.len())) else {
        return Err(format!("{} is not a WAV file", path.display()));
    };
    let channels = u16::from_le_bytes([bytes[format + 10], bytes[format + 11]]);
    let rate = u32::from_le_bytes([bytes[format + 12], bytes[format + 13], bytes[format + 14], bytes[format + 15]]);
    let bits = u16::from_le_bytes([bytes[format + 22], bytes[format + 23]]);
    if (channels, rate, bits) != (1, 16_000, 16) {
        return Err(format!("{} must be 16 kHz mono 16-bit, and it is {rate} Hz, {channels} channels, {bits} bits", path.display()));
    }
    Ok(bytes[data + 8..].as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect())
}
```

## What you learned

- `Speech::connect` with `Options` starts the SDK. `Engine::from_release` points to the engine that your app ships.
- `download_model` puts a model in the model store.
- `transcribe_file` returns a job id. The result comes as events, and the job ends with `job_done` or `job_failed`.
- `start_session`, `session_audio`, and `stop_session` run a live session.

## Next

- To use Sayso when it runs, change `Mode::Local` to `Mode::Auto`. The first connection shows a question to the user. [How to ask for capabilities](ask-for-capabilities.md) and [How to react to mode changes](react-to-mode-changes.md) cover what your app must handle.
- To capture the microphone, use `Source::Mic`. See [How to run a live session](run-a-live-session.md) and [How to prepare a macOS app](prepare-a-macos-app.md).
- To label the speakers, see [How to identify speakers](identify-speakers.md).
- For every message and field, see the [reference](reference.md).
