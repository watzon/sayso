# sayso-sdk

Use the speech features of [Sayso](https://github.com/watzon/sayso) from your own app: transcribe files, run live sessions, list and download models. MIT.

The app calls one API. When Sayso runs, the SDK uses Sayso: its engine, its loaded models, its microphone, and the choices of its user. When Sayso does not run, the SDK starts the engine that the app ships and does the work in the process of the app. The calls block and the events come on a channel, so you need no async runtime.

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

`examples/transcribe.rs` is a program that you can run.

## Modes

| Mode | What it does |
|---|---|
| `Mode::Sayso { start }` | Uses Sayso. Fails with `host_not_running` when it does not run, or `not_approved` when the user refused the app. With `start`, it starts Sayso first. |
| `Mode::Local` | Uses the engine of the app. Never talks to Sayso. |
| `Mode::Auto` | Uses Sayso when it can, else the engine of the app. It changes when Sayso starts or quits, and sends `Event::ModeChanged`. |

`Mode::Auto` changes only when the app has no active work: no request without a reply, no job, no live session. A change never moves work. When Sayso starts during a job in local mode, the job ends in local mode and the next job goes to Sayso. When Sayso crashes, a request without a reply gets `connection_lost`, each job gets `job_failed` with that code, and each live session ends with that code. The SDK never sends a request again by itself, because it cannot know if Sayso did it.

The first connection to Sayso can show a question to the user, and `Speech::connect` waits for the answer. Call it on a thread that can wait.

## What differs between modes

The messages and the end rules are the same. A job ends with one `job_done` or `job_failed`. A live session ends with the reply of `stop_session`, or else with one `session_ended`. These things differ:

- **Downloads.** In Sayso mode the user can refuse a download (`declined`). In local mode a download starts at once, so your app asks its own user first.
- **Models.** A request without a model gets the choice of the user in Sayso mode. In local mode it gets `Options::models`, else the first suitable model on disk.
- **Queues.** In Sayso mode the limits (one live session, one job at a time) are for all apps together. In local mode they are for this app only.
- **Priority.** Sayso mode puts dictation first, then live sessions, then jobs. Local mode has no dictation, and still puts live sessions before jobs.
- **Features.** `enhance`, `subscribe`, and the download question are in Sayso mode only. They give `unsupported` in local mode. `Speech::features` tells the app what it has now.

## What the app ships for local mode

- The release file of the engine, unpacked in a folder of the app. Use `Engine::from_release`. The SDK does not use the engine inside an installed Sayso, because an update of Sayso would then change the engine of your app.
- On macOS, the text `NSMicrophoneUsageDescription` in the `Info.plist` of the app, for the source `mic`. The system asks the user for your app.

The models go to the model store of the user, which Sayso also uses, so a model is on disk once. An engine without the feature `store_v1` uses a folder of the app. The source `system` of a live session gives `unsupported` for now.

## License

The crate is MIT. It starts the engine as a separate program and never links it. Check the license of the engine that you ship.
