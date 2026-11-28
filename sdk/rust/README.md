# sayso-sdk

Use the speech features of [Sayso](https://github.com/watzon/sayso) from your own app: transcribe files, run live sessions, list and download models. MIT.

The app calls one API. When Sayso runs, the SDK uses Sayso: its engine, its loaded models, its microphone, and the choices of its user. When Sayso does not run, the SDK starts the engine that the app ships and does the work in the process of the app. The calls block and the events come on a channel, so you need no async runtime.

This is one crate. It talks to Sayso and to the engines only over the wire protocols in [`sdk/protocol`](../protocol), and it never links code of the app.

## Features

| Feature | Adds | Dependencies |
|---|---|---|
| `local` | `Mode::Local` and `Mode::Auto`, the engine of the app (`Engine`, `ModelChoices`), jobs and live sessions (`speech`), `embedded` | `symphonia` (WAV, FLAC, MP3, AAC, M4A) |
| `capture` | the microphone and the system audio (`capture`, `permission`) | `cpal`, and the Core Audio, PulseAudio, and WASAPI bindings of each system |
| `cleanup` | the cleanup models: catalog, prompts, the runtime client, the download (`cleanup`) | `ureq`, `sha2`, `whatlang`, `regex` |
| `deepgram` | `Deepgram` and `DeepgramFinal` (needs `local`) | `ureq` |

All four are on by default. With `default-features = false` the crate is a small client of Sayso (`Mode::Sayso`, the raw `client::Client`, the `protocol` types, the `engine` client, and the path rules), with no audio and no HTTP dependency.

| Module | What it is |
|---|---|
| `protocol` | the messages of the Host API and of the engine protocol, the names, the socket path rules, and the PCM helpers |
| `client` | the raw client of the Host API |
| `engine` | start an engine program and talk to it with the engine protocol; `find_engine` |
| `audio` | the resampler |
| `speech`, `capture`, `embedded`, `deepgram` | the parts of local mode (features above) |
| `cleanup` | `cleanup::models` and `cleanup::client` |
| `paths`, `start`, `error` | where the data and the models are, how to start Sayso, the error type |

## Host features

`hello.features` of Sayso lists the optional parts of the Host API that this Sayso serves. The SDK checks the list before it sends a request that needs a part: a `session_start` with the source `mic`, `system`, `record_to`, or `silence_limit_ms`, an `enhance`, and a `subscribe` to `dictation_done`. A Sayso that lacks the part gives the app `unsupported` and gets no request. An empty list comes from a Sayso older than the list, and then the SDK sends the request and reads the reply. `Features::host` has the list.

## Example

```rust,no_run
use sayso_sdk::protocol::{self, Capability, TranscribeFile};
use sayso_sdk::{App, Engine, Event, Mode, Options, Speech};

let options = Options::new(Mode::Auto, App::new("Notetaker"))
    .capabilities([Capability::TranscribeFile])
    .engine(Engine::from_release("resources/engine"));
let speech = Speech::connect(options)?;
let job = speech.transcribe_file(TranscribeFile { path: "/tmp/talk.wav".into(), ..Default::default() })?;
for event in speech.events() {
    match event {
        Event::Message(protocol::Event::JobDone { job: done, text }) if done == job => {
            println!("{text}");
            break;
        }
        Event::Message(protocol::Event::JobFailed { job: failed, message, .. }) if failed == job => {
            eprintln!("{message}");
            break;
        }
        _ => {}
    }
}
# Ok::<(), sayso_sdk::Error>(())
```

`examples/transcribe.rs` is a program that you can run. With the feature `local` off, `Mode::Local` and `Mode::Auto` do not exist.

## Modes

| Mode | What it does |
|---|---|
| `Mode::Sayso { start }` | Uses Sayso. Fails with `host_not_running` when it does not run, or `not_approved` when the user refused the app. With `start`, it starts Sayso first. |
| `Mode::Local` | Uses the engine of the app. Never talks to Sayso. |
| `Mode::Auto` | Uses Sayso when it can, else the engine of the app. It changes when Sayso starts or quits, and sends `Event::ModeChanged`. |

`Mode::Auto` changes only when the app has no active work: no request without a reply, no job, no live session. A change never moves work. When Sayso starts during a job in local mode, the job ends in local mode and the next job goes to Sayso. When Sayso crashes, a request without a reply gets `connection_lost`, each job gets `job_failed` with that code, and each live session ends with that code. The SDK never sends a request again by itself, because it cannot know if Sayso did it.

The first connection to Sayso can show a question to the user, and `Speech::connect` waits for the answer. Call it on a thread that can wait. A Sayso that does not answer `hello` in 10 s and does not say that it asks the user counts as a Sayso that does not run.

## What differs between modes

The messages and the end rules are the same. A job ends with one `job_done` or `job_failed`. A live session ends with the reply of `stop_session`, or else with one `session_ended`. No event of a piece of work comes after its end. These things differ:

- **Downloads.** In Sayso mode the user can refuse a download (`declined`). In local mode a download starts at once, so your app asks its own user first.
- **Models.** A request without a model gets the choice of the user in Sayso mode. In local mode it gets `Options::models`, else the first suitable model on disk.
- **Queues.** In Sayso mode the limits (one live session, one job at a time) are for all apps together. In local mode they are for this app only.
- **Priority.** Sayso mode puts dictation first, then live sessions, then jobs. Local mode has no dictation, and still puts live sessions before jobs.
- **Features.** `enhance`, `subscribe`, and the download question are in Sayso mode only. They give `unsupported` in local mode. `Speech::features` tells the app what it has now.

## What the app ships for local mode

- The release file of the engine, unpacked in a folder of the app. Use `Engine::from_release`. The SDK does not use the engine inside an installed Sayso, because an update of Sayso would then change the engine of your app.

The models go to the model store of the user, which Sayso also uses, so a model is on disk once. An engine without the feature `store_v1` uses a folder of the app.

### On macOS

In local mode the permissions belong to your app, not to Sayso:

- `NSMicrophoneUsageDescription` in the `Info.plist` of the app, for the source `mic`.
- `NSAudioCaptureUsageDescription` in the `Info.plist` of the app, for the source `system`. Without this text the system does not ask in the normal way, and macOS can end a program that asks.
- A line in the `build.rs` of your app, so it starts on macOS 14.0 and 14.1:

  ```rust
  fn main() {
      if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
          println!("cargo:rustc-link-arg-bins=-Wl,-weak_framework,CoreAudio");
      }
  }
  ```

  The audio library `cpal` imports functions that came with macOS 14.2. Without this line, the program does not start on the older versions. With it, the program starts, and the source `system` gives `unsupported` there. The SDK cannot do this for you, because a library cannot set a link flag for the program of another package.

## Live sessions with the source `system`

- `start_session` gives `excludes`: what the capture really leaves out. In local mode on macOS it is `["app"]`, and in Sayso mode `["sayso", "app"]`. It is empty on Windows and Linux, where the capture records the mix. So sound of your own app can be in the text there.
- The first `start_session` with `system` on macOS can wait until the user answers a question of the system (up to 120 s). Call it on a thread that can wait. Other calls of `Speech` go on while it waits.
- A refused permission gives the code `permission`. `Speech::features` tells you `system_audio` without a question to the user.
- `source_gap {session, source, ms}` says that a source lost audio, for example at a change of the output device. The session goes on.
- `source_ended {session, source, reason}` says that one source ended: the user took the permission back, or the capture failed. The other sources go on, and the session still ends with the reply of `stop_session`.
- `part_failed {session, source, start_ms, end_ms, message, code?}` says that the final pass of one part failed, for example because of a wrong key. The session has no segment for that audio, and it goes on.
- In a session with `mic` and `system`, the SDK drops a microphone segment that repeats the system audio of the same time.

## Tests

`cargo test` runs the unit tests, the wire corpus of `../protocol/testdata`, and the tests against the Go programs of the repository: `cmd/sayso-testhost` (the real Host API on WAV files), `sdk/go/cmd/sayso-fake-engine`, and `sdk/go/cmd/sayso-fake-cleanup`. The tests build them with `go build`, or take them from `SAYSO_TESTHOST`, `SAYSO_FAKE_ENGINE`, and `SAYSO_FAKE_CLEANUP`. Without Go, or outside the repository, these tests skip with a note, unless `SAYSO_TEST_REQUIRE_FAKES=1` is set: then they fail. `SAYSO_SCENARIO_REPORT=<file>` collects the ids of the scenarios that passed.

## License

The crate is MIT, and so are its dependencies (`deny.toml` lists the licenses that are allowed). It starts the engine as a separate program and never links it. Check the license of the engine that you ship.
