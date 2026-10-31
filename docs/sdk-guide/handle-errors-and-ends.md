# How to handle errors and the end of each piece of work

Every piece of work in the SDK ends in a way that you can count on. This page lists the ends, the error codes, and what to do with each.

## Know the ends

| Work | How it ends |
|---|---|
| A request (`list_models`, `download_model`, `transcribe_file`, and the others) | One `Result`. Either `Ok` or one `Error`. |
| A job | One `job_done` or one `job_failed` event. No event of the job comes after it. |
| A live session | The `Ok` of `stop_session`, or else one `session_ended` event. The first of the two counts. |
| A download | The `Result` of `download_model`, after the last `model_state` event. |
| `session_audio` | `Ok(())` at once. An error of the audio itself arrives as `Event::Error`. |

These rules are the same in Sayso mode and in local mode. They also hold when the mode changes or the connection breaks. The SDK never sends a request again by itself.

## Wait for a job

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
enum JobEnd {
    Done(String),
    Failed(Error),
}

fn wait_for_job(speech: &Speech, job: u64) -> JobEnd {
    for event in speech.events() {
        match event {
            Event::Message(protocol::Event::JobDone { job: id, text }) if id == job => return JobEnd::Done(text),
            Event::Message(protocol::Event::JobFailed { job: id, message, code }) if id == job => {
                return JobEnd::Failed(Error { code, message, capability: None, reason: None });
            }
            _ => {}
        }
    }
    JobEnd::Failed(lost("the events of the SDK ended"))
}
```

`job_failed` has a `message` for people and an optional `code`. A job that you cancel ends with `job_failed`, no `code`, and the message `The job was cancelled.` A file that the SDK cannot open or read ends with `job_failed` and no `code`. The message says why.

## Stop a session

Call `stop_session` once. A second call for the same session returns `bad_request`, and so does a call for a session that ended. If a `session_ended` event came first, the session is over, and you have its `text` in the event.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn stop_and_get_text(speech: &Speech, session: u64) -> Result<String, Error> {
    match speech.stop_session(session) {
        Err(error) if error.code == Some(ErrorCode::ConnectionLost) || error.code == Some(ErrorCode::HostClosing) => {
            eprintln!("The session ended with the connection: {error}");
            Err(error)
        }
        other => other,
    }
}
```

## Read the error codes

An `Error` has `code` (optional), `message` (English, for people), `capability` (with the code `capability`), and `reason` (a code of the engine, for example `download_failed`). Act on `code`. Show `message` to the user only as detail.

| Code | Meaning | What to do |
|---|---|---|
| `not_approved` | The user did not allow the app, or Sayso cannot trust its identity. | Stop. Tell the user where to change it. Do not retry. |
| `capability` | The user did not grant the capability in `capability`. | Disable that feature. Ask again only when the user acts. |
| `busy` | Another live session runs, or too many apps wait for an approval question. | Wait and try again later, with a limit. |
| `declined` | The user said no to a download. | Respect the answer. |
| `model_not_downloaded` | The model is not on disk. | Call `download_model`, then try again. |
| `unknown_model`, `model_retired` | The id is not in the catalog. | Choose another model from `list_models`. |
| `wrong_role` | The model cannot do this role. | Choose a model with the role. |
| `unsupported` | This mode, engine, or system cannot do it. | Disable the feature. See `Speech::features`. |
| `bad_request` | A field is missing or wrong, or the id names no work of your app. | Fix the code. |
| `too_many_streams` | The engine has no free stream. | Stop other work, then try again. |
| `permission` | The system refused the microphone or the system audio. | Tell the user to allow it in System Settings. |
| `unavailable`, `refused`, `timeout` | The AI provider of `enhance` failed. | Show the message. Try again later. |
| `host_not_running` | Sayso does not run (`Mode::Sayso` only). | Ask the user to start Sayso, or use `Mode::Auto`. |
| `connection_lost` | The connection to Sayso ended without a goodbye. The request may have run. | Check the state before you repeat a request. |
| `host_closing` | Sayso quit and ended this work. | Start the work again if the user wants it, in the new mode. |

`host_not_running`, `connection_lost`, and `host_closing` come only from the SDK. Sayso never sends them.

An `Error` without a `code` has a `message` only. It comes from a failed download or a file that could not be read. Show the message.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn explain(error: &Error) -> String {
    match &error.code {
        Some(ErrorCode::NotApproved) => "The user did not allow this app in Sayso. Tell them where to change it: Settings > Connected apps.".into(),
        Some(ErrorCode::Capability) => format!("The user did not grant {}.", error.capability.as_ref().map_or("a capability".to_string(), |capability| capability.to_string())),
        Some(ErrorCode::Busy) => "Sayso is busy. Try again later.".into(),
        Some(ErrorCode::HostNotRunning) => "Sayso does not run.".into(),
        _ => error.to_string(),
    }
}
```

## Read errors that belong to no request

`Event::Error` carries an error that no request caused. For example, the audio of `session_audio` was not valid. Log it, and show it to the user if the session cannot go on.

## Close the SDK

Drop the `Speech` to close it. This ends its work, closes the connection to Sayso, and stops the engine of the app. Stop your live sessions first, so you get their text.
