# How to identify speakers

Use diarization to learn who speaks when. The SDK adds `speakers` events to a job or a live session. You match them with the segments.

You need a diarization model on disk. List the models with the role `diarization` and download one (see [How to choose a model and download it](choose-and-download-a-model.md)). The model `speaker-diarization` is 21 MB.

## Ask for speakers in a job

Set `diarize: Some(true)` in `TranscribeFile`. If you know the number of speakers, set `speakers`. Without it, the engine finds the number. `speakers: Some(0)` returns `bad_request`.

If no diarization model is on disk, the request returns `model_not_downloaded`. To pick a model, set `diarization_model`. A `diarization_model` without `diarize` also turns diarization on. `diarize: Some(false)` turns it off.

The job sends one `speakers { job, turns }` event after the last `segment`. Each turn has `start_ms`, `end_ms`, and `speaker`. A speaker is a label such as `"1"` or `"2"`. The label is unique in one job. It is not a name. Your app names the speakers.

A segment has no speaker. Give each segment the turn that overlaps it most:

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn transcribe_with_speakers(speech: &Speech, path: PathBuf) -> Result<Vec<(String, String)>, Error> {
    let request = TranscribeFile { path, diarize: Some(true), speakers: Some(2), ..Default::default() };
    let job = speech.transcribe_file(request)?;
    let mut segments = Vec::new();
    let mut turns = Vec::new();
    for event in speech.events() {
        match event {
            Event::Message(protocol::Event::Segment(found)) if found.job == Some(job) => segments.push(found.segment),
            Event::Message(protocol::Event::Speakers(found)) if found.job == Some(job) => turns.extend(found.turns),
            Event::Message(protocol::Event::JobDone { job: id, .. }) if id == job => break,
            Event::Message(protocol::Event::JobFailed { job: id, message, code }) if id == job => {
                return Err(Error { code, message, capability: None, reason: None });
            }
            _ => {}
        }
    }
    Ok(segments.iter().map(|segment| (speaker_of(segment, &turns).unwrap_or("?").to_string(), segment.text.clone())).collect())
}

fn overlap(segment: &Segment, turn: &Turn) -> u64 {
    segment.end_ms.min(turn.end_ms).saturating_sub(segment.start_ms.max(turn.start_ms))
}

fn speaker_of<'a>(segment: &Segment, turns: &'a [Turn]) -> Option<&'a str> {
    let best = turns.iter().max_by_key(|turn| overlap(segment, turn))?;
    (overlap(segment, best) > 0).then_some(best.speaker.as_str())
}
```

On our test file with a male and a female voice, this gave:

```text
1: Good Morning!
1: Did you finish the report for the meeting today?
2: Yes, I did, I sent it to everyone an hour ago, so please read it before we start.
```

## Ask for speakers in a live session

Set `diarize: Some(true)` in `SessionStart`. Diarization runs on each source alone, when the session stops. So you get one `speakers { session, source, turns }` event for each source, after the last `segment` and before `stop_session` returns. A label is unique in one source of one session. In a call, `mic` is usually one person, and `system` is everyone else. Use the pair of source and label as the key.

## Know the limits

- Diarization needs the whole recording, so a live session shows speakers only at the end.
- Diarization of a long file takes time at the end of the job. Progress stays below 100 % until it is done.
