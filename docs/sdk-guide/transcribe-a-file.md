# How to transcribe a file

Use a job to transcribe an audio file. A job runs in the background and sends its result as events.

You need a connected `Speech` (see the [tutorial](tutorial.md)) and a speech model on disk (see [How to choose a model and download it](choose-and-download-a-model.md)). In Sayso mode, your app also needs the capability `transcribe.file` (see [How to ask for capabilities](ask-for-capabilities.md)).

## Transcribe the file

1. Give the SDK an absolute path. A relative path gets `bad_request`.
2. Call `transcribe_file`. It returns the id of the job at once.
3. Read `speech.events()` until the job ends. Keep the events with `job == your id`.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
struct Transcript {
    text: String,
    segments: Vec<Segment>,
}

fn transcribe_file(speech: &Speech, path: PathBuf) -> Result<Transcript, Error> {
    let request = TranscribeFile { path, language: Some("en".into()), ..Default::default() };
    let job = speech.transcribe_file(request)?;
    let mut segments = Vec::new();
    for event in speech.events() {
        match event {
            Event::Message(protocol::Event::Segment(found)) if found.job == Some(job) => segments.push(found.segment),
            Event::Message(protocol::Event::JobDone { job: id, text }) if id == job => return Ok(Transcript { text, segments }),
            Event::Message(protocol::Event::JobFailed { job: id, message, code }) if id == job => {
                return Err(Error { code, message, capability: None, reason: None });
            }
            _ => {}
        }
    }
    Err(lost("the events of the SDK ended"))
}
```

The job ends with exactly one `job_done` or one `job_failed`. No event of the job comes after that. The text in `job_done` is the full transcript. Each `segment` event has `start_ms` and `end_ms` in milliseconds from the start of the file. It has `words` with a time for each word when the model has the timestamp level `word`.

## Change how the file is transcribed

All fields of `TranscribeFile` except `path` are optional:

- `model`: the id of a speech model. Without it, the SDK takes the model that you chose in `Options::models`. In Sayso mode, the host takes the choice of the user instead.
- `language`: a code such as `en`. The default is `auto`.
- `vad_model`: a voice detection model, or `basic` for Basic VAD. See [reference](reference.md#transcribe_file).
- `diarize`, `diarization_model`, and `speakers`: see [How to identify speakers](identify-speakers.md).

## Show progress

`job_progress { job, fraction }` goes from 0 to 1. A job that waits behind another job sends `fraction: 0` until it starts. With speakers, transcription is the first 90 % and diarization the last 10 %.

## Cancel a job

Call `cancel_job` with the id. The job ends with `job_failed` and the message `The job was cancelled.` It has no `code`. A cancel for a job that ended already returns `Ok`, so a cancel that crosses the end of the job is not an error. A job id that the app never got returns `bad_request`.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn cancel(speech: &Speech, job: u64) -> Result<(), Error> {
    speech.cancel_job(job)
}
```

## Know the limits

- The SDK reads WAV, FLAC, MP3, AAC, and M4A files. For another format, convert the file first, or send the audio as a live session with the source `client`. A file that the SDK cannot read ends the job with `job_failed`. The message names the formats.
- One job runs at a time. Other jobs wait in a queue, first in, first out. In Sayso mode the queue is shared by all apps. In local mode it is only yours.
- One thread must read `speech.events()`. All clones of the receiver take from the same channel, so two readers each see only some of the events. If your app has more than one piece of work, read the events in one place and send them on.
