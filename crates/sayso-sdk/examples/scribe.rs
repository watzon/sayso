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
