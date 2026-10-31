# How to run a live session

Use a live session to transcribe audio while it comes in. The audio can come from the microphone, from the sound of other apps, or from your own code.

You need a connected `Speech` and a speech model on disk. In Sayso mode, your app needs the capability `session.live`, and also `audio.mic` for the microphone and `audio.system` for the system audio. See [How to ask for capabilities](ask-for-capabilities.md). On macOS, read [How to prepare a macOS app](prepare-a-macos-app.md) first.

## Choose the sources

`SessionStart::sources` lists the sources:

| Source | Audio from | Needs |
|---|---|---|
| `Source::Mic` | The microphone that the user chose in Sayso (Sayso mode), or the default microphone (local mode). | The capability `audio.mic` in Sayso mode. The microphone permission of the system for your app in local mode. |
| `Source::System` | The sound that the computer plays. | The capability `audio.system` in Sayso mode. In local mode, the permission of the system for your app. |
| `Source::Client` | Your code, with `session_audio`. | The capability `session.live` only. |

Each source is a separate stream. Every `partial` and `segment` event names its source.

## Start a session from the microphone and the system audio

`start_session` returns the id of the session and `excludes`. `excludes` lists what the capture of the system audio leaves out: `sayso`, `app`, or both. When it is empty, the capture records the mix of the system, and the sound of your own app can be in the text. It is empty on Windows and Linux.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn start_call(speech: &Speech, folder: &Path) -> Result<u64, Error> {
    let request = SessionStart { sources: vec![Source::Mic, Source::System], record_to: Some(folder.into()), ..Default::default() };
    let started = speech.start_session(request)?;
    if started.excludes.is_empty() {
        eprintln!("The capture records the mix of the system, with the sound of this app.");
    }
    Ok(started.session)
}
```

On macOS, a first `start_session` with `Source::System` can wait up to 120 seconds for the answer of the user to a question of the system. Call it on a thread that can wait. Other calls of `Speech` go on while it waits.

If the system refuses a permission, `start_session` returns the error code `permission`.

## Read the events and stop the session

A session ends in one of two ways. Your app calls `stop_session`, which returns the full text after the last events. Or the user stops the session in Sayso, or the connection ends, and you get one `session_ended` event. Only the first of the two counts. Handle both.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn label(source: &Option<Source>) -> String {
    source.as_ref().map_or_else(String::new, Source::to_string)
}

/// Print the events of the session. True when the session ended without a stop of this app.
fn show_event(event: &Event, session: u64) -> bool {
    match event {
        Event::Message(protocol::Event::Partial(partial)) if partial.session == session => {
            println!("{}: {} {}", label(&partial.source), partial.committed, partial.tentative);
        }
        Event::Message(protocol::Event::Segment(found)) if found.session == Some(session) => {
            println!("{}: {}", label(&found.source), found.segment.text);
        }
        Event::Message(protocol::Event::SourceGap { session: id, source, ms }) if *id == session => {
            eprintln!("{source} lost {ms} ms of audio");
        }
        Event::Message(protocol::Event::SourceEnded { session: id, source, reason }) if *id == session => {
            eprintln!("{source} ended: {reason}");
        }
        Event::Message(protocol::Event::SessionEnded { session: id, .. }) if *id == session => return true,
        _ => {}
    }
    false
}

fn follow_session(speech: &Speech, session: u64, stopped: &AtomicBool) {
    let events = speech.events();
    loop {
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(event) if show_event(&event, session) => break,
            Ok(_) => {}
            Err(_) if stopped.load(Ordering::SeqCst) => break,
            Err(_) => {}
        }
    }
}

fn record_for(speech: &Speech, folder: &Path, how_long: Duration) -> Result<String, Error> {
    let session = start_call(speech, folder)?;
    let stopped = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| follow_session(speech, session, &stopped));
        std::thread::sleep(how_long);
        let text = speech.stop_session(session);
        stopped.store(true, Ordering::SeqCst);
        text
    })
}
```

The events of a session are:

- `partial { session, source, committed, tentative }`. The text of one source so far. `committed` does not change any more. `tentative` can still change. A model without the role `streaming` sends no `partial`.
- `segment { session, source, start_ms, end_ms, text, words? }`. A part of the text that will not change. Times are from the start of the session.
- `source_gap { session, source, ms }`. A source lost audio, for example when the output device changed. The session goes on, and the recording has `ms` of silence there.
- `source_ended { session, source, reason }`. One source ended and the others go on. `reason` is `permission` (the user took the permission back) or `failed` (the capture did not come back). The last `segment` of the source comes before this event.
- `session_ended { session, text, code? }`. The session ended without a `stop_session` of your app. `code` is set when the connection ended.

See the [reference](reference.md#events) for every field.

## Send your own audio

For `Source::Client`, send 16 kHz mono samples of type `i16` with `session_audio`. Chunks of 1600 samples (100 ms) are a good size. `session_audio` does not wait for a reply. An error of the audio itself arrives as `Event::Error`.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn send_audio(speech: &Speech, session: u64, samples: &[i16]) -> Result<(), Error> {
    for chunk in samples.chunks(1_600) {
        speech.session_audio(session, chunk)?;
    }
    Ok(())
}
```

Send some silence at the end, so the voice detection can end the last sentence. The [tutorial](tutorial.md) does this.

## Keep the audio

Set `record_to` in `SessionStart` to a folder. The SDK writes one 16 kHz mono WAV file for each source there: `mic.wav`, `system.wav`, and `client.wav`. An existing file with that name is replaced.

## Know the limits

- One live session runs at a time. A second `start_session` returns `busy`. In Sayso mode the limit is for all apps together. In local mode it is for your app.
- In a session with `mic` and `system`, the SDK drops a microphone segment that repeats the system audio of the same time. This stops the text from showing twice when the user has no headphones. It does not filter `partial` events. A microphone segment can arrive later than the system segments, because it waits for the system source to pass its time.
- The engine must have a free stream. If it has none, `start_session` returns `too_many_streams`.
