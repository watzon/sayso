# How to ask for capabilities and handle `not_approved` and `busy`

In Sayso mode, the user controls what your app can do. Your app asks for capabilities. Sayso shows a question the first time the app connects. This page shows how to ask, and how to handle each answer.

Local mode has no question. The app can do everything that the SDK offers, and the system asks for the microphone and the system audio on its own.

## Know the capabilities

| Capability | Allows | Needed by |
|---|---|---|
| `models.read` | List models. Always granted, with no question. | `list_models` |
| `models.download` | Ask the user to download a model. | `download_model` |
| `transcribe.file` | Jobs. | `transcribe_file` |
| `session.live` | Live sessions. | `session_start`, `session_audio` |
| `audio.mic` | Live sessions from the microphone. | `session_start` with `mic` |
| `audio.system` | Live sessions from the system audio. | `session_start` with `system` |
| `ai.enhance` | The AI providers and styles of the user. | `enhance` |
| `dictation.observe` | Each dictation after it is inserted. | `subscribe` |

`cancel_job` and `stop_session` need no capability. An app can always end its own work.

## Ask when you connect

List the capabilities that your app uses in `Options::capabilities`. Ask for what you need, and for nothing more. The user sees the list.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn connect_to_sayso() -> Result<Speech, Error> {
    let options = Options::new(Mode::Sayso { start: false }, App::new("Notetaker")).capabilities([Capability::TranscribeFile, Capability::SessionLive, Capability::AudioMic]);
    Speech::connect(options)
}
```

`Speech::connect` waits for the answer of the user, so call it on a thread that can wait. Sayso says at once that it asks. A Sayso that says nothing and does not reply in 10 s (`Timing::hello_limit`) counts as a Sayso that does not run. The answer is **Allow** for all the listed capabilities, or **Don't allow**. Sayso remembers it. If your app asks for a new capability later, Sayso asks only about the new one. Until the user answers, your app keeps its old grants.

## Check what you have

`speech.features()` tells what the mode has now. In Sayso mode, `capabilities` lists what the user granted, and `denied` lists what the user refused. Sayso can also put a capability in `denied` when its version does not know the capability.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn can_record_the_system(speech: &Speech) -> bool {
    let features = speech.features();
    features.system_audio && !features.denied.contains(&Capability::AudioSystem)
}
```

Use this to hide a button, and not to replace the error check. The user can take a grant back at any time in Settings. Then the next request of that kind fails.

## Handle `not_approved`

`Speech::connect` returns `not_approved` in `Mode::Sayso` when the user chose **Don't allow**, or closed the question. It also returns it when Sayso cannot trust the identity of your program, for example when the signature of a macOS app is not valid. In that case Sayso asks no question.

Do not connect again in a loop. Sayso remembers the answer and replies `not_approved` at once, with no question. Tell the user that the app needs the permission, and where to change it: **Settings, Connected apps** in Sayso.

With `Mode::Auto`, `connect` does not fail on `not_approved`. The SDK uses local mode instead. Check `speech.mode()` if your app must tell the user.

## Handle `capability`

If your app calls a request that needs a capability that the user did not grant, the reply is the error code `capability`. The error has the field `capability` with the name of the missing capability. Show the user a message that names it:

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

## Handle `busy`

`busy` comes from two places:

- **`Speech::connect`**: Sayso has too many apps or connections that wait for an approval question. A fifth identity, or a fifth connection of the same identity, gets `busy` at once. After the user closes a question that nobody answered, the same app gets `busy` for 30 seconds. Wait and try again later. Do not retry in a tight loop.
- **`start_session`**: another live session runs. Only one runs at a time. In Sayso mode that is across all apps. Ask the user to stop the other, or wait.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn start_when_free(speech: &Speech, request: SessionStart) -> Result<u64, Error> {
    for _ in 0..3 {
        match speech.start_session(request.clone()) {
            Err(error) if error.code == Some(ErrorCode::Busy) => std::thread::sleep(Duration::from_secs(5)),
            other => return other.map(|started| started.session),
        }
    }
    Err(Error { code: Some(ErrorCode::Busy), message: "another live session still runs".into(), capability: None, reason: None })
}
```

## Handle `declined`

`download_model` returns `declined` when the user said no to the download question. This is an answer and not a failure. Do not ask again at once.
