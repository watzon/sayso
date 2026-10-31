# How to react to mode changes

With `Mode::Auto`, the SDK uses Sayso when Sayso runs, and your own engine when it does not. It changes by itself. Your app learns about a change from the event `Event::ModeChanged`. This is an event of the SDK, and not a message of the Host API.

## When the mode changes

- **Sayso starts.** The SDK watches for the socket of Sayso while it runs in local mode. When Sayso starts and the user approves your app, the SDK changes to Sayso mode. It does this only when the app has no active work. A change never moves work. A long job that runs in local mode ends in local mode, and the next job goes to Sayso. After the change, the SDK stops its own engine, so the model is in memory once.
- **Sayso quits.** Sayso ends the work of your app first. Each job ends with `job_failed`, and each live session stops. The SDK ends any work that is still open with the code `host_closing`: `job_failed` for a job and `session_ended` for a live session. Then the SDK changes to local mode.
- **Sayso crashes.** Each request without a reply returns the code `connection_lost`. Each job ends with `job_failed` and each live session with `session_ended`, both with that code. Then the SDK changes to local mode. The SDK never sends a request again by itself, because it cannot know if Sayso already did the work.

The first request after the event runs in the new mode.

## What your app does

Read `Event::ModeChanged { mode, features }`. Then:

1. Read `features`. The mode decides which parts of the API work (see below). Show or hide the buttons of those parts.
2. In Sayso mode, `subscribe` again if you use `dictation_done`. A subscription belongs to one connection.
3. Do not reuse old ids. Ids of jobs and sessions come from the SDK, and no id comes back after a change.
4. If work ended with `connection_lost` or `host_closing`, decide with the user whether to start it again. The SDK does not.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn watch_mode(speech: &Speech) {
    for event in speech.events() {
        match event {
            Event::ModeChanged { mode: ActiveMode::Sayso, features } => {
                eprintln!("Sayso runs now. Enhance: {}", features.enhance);
                if features.subscribe
                    && let Err(error) = speech.subscribe(&["dictation_done"])
                {
                    eprintln!("{}", explain(&error));
                }
            }
            Event::ModeChanged { mode: ActiveMode::Local, .. } => eprintln!("Sayso quit. The app uses its own engine."),
            Event::Disconnected { code } => eprintln!("The connection to Sayso ended: {code}"),
            Event::Message(protocol::Event::DictationDone(dictation)) => println!("Dictated: {}", dictation.text),
            _ => {}
        }
    }
}
```

## What differs between the modes

| Part | Sayso mode | Local mode |
|---|---|---|
| `enhance`, `subscribe`, `dictation_done` | Work, if the user granted them. | Return `unsupported`. |
| Download question | The user can say no (`declined`). | None. The download starts at once. |
| Model choice | The choice of the user in Sayso. | `Options::models`, then the first suitable model on disk. |
| Queues | One live session and one job at a time, for all apps. | The same limits, for your app only. |
| Priority | Dictation, then live sessions, then jobs. | Live sessions, then jobs. |
| Engine features | The engine of Sayso. | The engine that your app ships. They can differ. |

## Mode::Sayso does not change

With `Mode::Sayso`, a lost connection gives `Event::Disconnected { code }` and every request returns `host_not_running`. The SDK does not connect again by itself. Call `speech.reconnect()` when your app decides to. It sends `Event::ModeChanged` when the connection is back. Work does not survive the end of a connection.

## Test it

Start your app with `Mode::Auto` and no Sayso running. `speech.mode()` is `Local`. Start Sayso. When your app has no work, you get `Event::ModeChanged { mode: ActiveMode::Sayso, .. }`. Quit Sayso, and you get `ModeChanged` to `Local`.

We did not run this check by hand with a real Sayso. The repository tests it with a real host in a child process: `crates/sayso-host/tests/sdk_modes.rs`.
