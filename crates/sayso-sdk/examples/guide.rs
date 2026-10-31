//! The code of the how-to guides in `docs/sdk-guide/`. It compiles with the other examples and
//! does not run. `scripts/check-sdk-guide.py` checks that each guide shows code from this file
//! line for line.

#![allow(dead_code)]

use sayso_sdk::protocol::{self, Capability, ErrorCode, Role, Segment, SessionStart, Source, TranscribeFile, Turn};
use sayso_sdk::{ActiveMode, App, Engine, Error, Event, Mode, ModelChoices, Options, Speech};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn main() {
    eprintln!("This example only compiles the code of docs/sdk-guide/. It does not run.");
}

fn lost(message: &str) -> Error {
    Error { code: Some(ErrorCode::ConnectionLost), message: message.into(), capability: None, reason: None }
}

// --- Transcribe a file ---

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

fn cancel(speech: &Speech, job: u64) -> Result<(), Error> {
    speech.cancel_job(job)
}

// --- A live session from the microphone and the system audio ---

fn start_call(speech: &Speech, folder: &Path) -> Result<u64, Error> {
    let request = SessionStart { sources: vec![Source::Mic, Source::System], record_to: Some(folder.into()), ..Default::default() };
    let started = speech.start_session(request)?;
    if started.excludes.is_empty() {
        eprintln!("The capture records the mix of the system, with the sound of this app.");
    }
    Ok(started.session)
}

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

fn send_audio(speech: &Speech, session: u64, samples: &[i16]) -> Result<(), Error> {
    for chunk in samples.chunks(1_600) {
        speech.session_audio(session, chunk)?;
    }
    Ok(())
}

// --- Speakers ---

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

// --- Choose a model and download it ---

fn show_models(speech: &Speech) -> Result<(), Error> {
    for model in speech.list_models(Some(Role::Final))? {
        let chosen: Vec<String> = model.choice.iter().map(|role| role.to_string()).collect();
        println!("{} ({}): {}, {} MB, chosen for: {}", model.id, model.name, model.state, model.size_bytes / 1_000_000, chosen.join(", "));
    }
    Ok(())
}

fn download(speech: &Speech, model: &str) -> Result<(), Error> {
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let events = speech.events();
            while !done.load(Ordering::SeqCst) {
                if let Ok(Event::Message(protocol::Event::DownloadProgress(progress))) = events.recv_timeout(Duration::from_millis(100)) {
                    eprintln!("{}: {:.0} %", progress.model, progress.fraction * 100.0);
                }
            }
        });
        let result = speech.download_model(model, "To transcribe your recordings on this computer");
        done.store(true, Ordering::SeqCst);
        result
    })
}

fn options_with_models(engine: Engine) -> Options {
    let models = ModelChoices { final_model: Some("whisper-tiny".into()), vad_model: Some("basic".into()), language: Some("en".into()), ..Default::default() };
    Options::new(Mode::Local, App::new("Notetaker")).engine(engine).models(models)
}

// --- Capabilities, `not_approved`, and `busy` ---

fn connect_to_sayso() -> Result<Speech, Error> {
    let options = Options::new(Mode::Sayso { start: false }, App::new("Notetaker")).capabilities([Capability::TranscribeFile, Capability::SessionLive, Capability::AudioMic]);
    Speech::connect(options)
}

fn explain(error: &Error) -> String {
    match &error.code {
        Some(ErrorCode::NotApproved) => "The user did not allow this app in Sayso. Tell them where to change it: Settings > Connected apps.".into(),
        Some(ErrorCode::Capability) => format!("The user did not grant {}.", error.capability.as_ref().map_or("a capability".to_string(), |capability| capability.to_string())),
        Some(ErrorCode::Busy) => "Sayso is busy. Try again later.".into(),
        Some(ErrorCode::HostNotRunning) => "Sayso does not run.".into(),
        _ => error.to_string(),
    }
}

fn can_record_the_system(speech: &Speech) -> bool {
    let features = speech.features();
    features.system_audio && !features.denied.contains(&Capability::AudioSystem)
}

fn start_when_free(speech: &Speech, request: SessionStart) -> Result<u64, Error> {
    for _ in 0..3 {
        match speech.start_session(request.clone()) {
            Err(error) if error.code == Some(ErrorCode::Busy) => std::thread::sleep(Duration::from_secs(5)),
            other => return other.map(|started| started.session),
        }
    }
    Err(Error { code: Some(ErrorCode::Busy), message: "another live session still runs".into(), capability: None, reason: None })
}

// --- React to a change of mode ---

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

// --- Ship the engine in your app ---

fn engine_in_resources(resources: &Path) -> Engine {
    Engine::from_release(resources.join("engine"))
}

fn engine_program(resources: &Path) -> std::io::Result<PathBuf> {
    sayso_engine_sdk::find_engine(resources.join("engine"))
}

fn engine_with_own_folders(program: PathBuf, data: &Path) -> Engine {
    Engine::from_program(program).store(data.join("models").join("v1")).models_dir(data.join("models-old"))
}

// --- Handle errors and the end of each piece of work ---

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

fn stop_and_get_text(speech: &Speech, session: u64) -> Result<String, Error> {
    match speech.stop_session(session) {
        Err(error) if error.code == Some(ErrorCode::ConnectionLost) || error.code == Some(ErrorCode::HostClosing) => {
            eprintln!("The session ended with the connection: {error}");
            Err(error)
        }
        other => other,
    }
}
