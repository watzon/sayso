//! A small extension for the command line. It shows how to use the SDK, and
//! developers and the tests of Sayso use it to try the Host API.
//!
//! ```text
//! cargo run -p sayso-extension --example sayso-ext -- hello
//! ```
//!
//! The program can run from any folder. At the first connection, Sayso asks
//! the user whether this program may use it (`docs/sdk.md`, "The question"),
//! and this program waits for the answer.
//!
//! Each message from Sayso is one JSON line on stdout, as it is on the
//! socket: the events of a request, then its reply. A reply `error` ends the
//! program with the exit code 1. Wrong arguments end it with 2.

use sayso_extension::protocol::{self, Event as HostEvent, Reply as WireReply, pcm};
use sayso_extension::{AppHello, Client, Error, Event, Fields, Reply};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: sayso-ext [--start] [--app-name N] [--capability C]... <command> [options]

Commands:
  hello
  list-models [--role final|streaming|vad|diarization]
  download <model> <reason>
  transcribe <file> [--model M] [--vad-model M] [--language L] [--diarize] [--diarization-model M] [--speakers N]
  session --source client --file <wav> [--speed X] [session options]
  session --source mic --seconds N [session options]
  enhance <text> [--style S | --prompt P] [--provider P]
  watch

Session options: --model M, --preview-model M, --vad-model M, --language L, --diarize, --record-to <folder>.
--speed 1 sends the file in real time (the default), 4 sends it four times faster, and 0 as fast as possible.
--start starts Sayso when it does not run, and waits up to 10 s for it.
--app-name gives the name that the app claims in hello. --capability asks for a capability in hello.";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let start = take_flag(&mut args, "--start");
    let mut app = AppHello::default();
    while let Some(name) = take_value(&mut args, "--app-name") {
        app.name = Some(name);
    }
    while let Some(capability) = take_value(&mut args, "--capability") {
        app.capabilities.push(protocol::Capability::parse(&capability));
    }
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!("{USAGE}");
        return;
    }
    let command = args.remove(0);
    let connected = protocol::socket::host_socket_path().map_err(Error::from).and_then(|socket| if start { Client::connect_or_start_as(&socket, &app, sayso_extension::START_WAIT, sayso_extension::start_sayso) } else { Client::connect_as(&socket, &app) });
    let client = match connected {
        Ok(client) => client,
        // No reply of a command is on stdout yet.
        Err(e) => fail_with(&e.to_string()),
    };
    let result = match command.as_str() {
        "hello" => {
            print_ok(Reply { fields: fields(serde_json::to_value(client.hello()).unwrap_or_default()) });
            Ok(())
        }
        "list-models" => list_models(&client, args),
        "download" => download(&client, args),
        "transcribe" => transcribe(&client, args),
        "session" => session(&client, args),
        "enhance" => enhance(&client, args),
        "watch" => watch(&client),
        other => usage(&format!("unknown command {other}")),
    };
    if let Err(e) = result {
        fail(&e);
    }
    let _ = client.close();
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn list_models(client: &Client, mut args: Vec<String>) -> Result<(), Error> {
    let mut body = Fields::new();
    if let Some(role) = take_value(&mut args, "--role") {
        body.insert("role".into(), role.into());
    }
    no_more(&args);
    call(client, "list_models", body).map(print_ok)
}

fn download(client: &Client, args: Vec<String>) -> Result<(), Error> {
    let [model, reason] = args.as_slice() else { usage("download needs <model> <reason>") };
    call(client, "download_model", fields(json!({"model": model, "reason": reason}))).map(print_ok)
}

fn transcribe(client: &Client, mut args: Vec<String>) -> Result<(), Error> {
    let mut body = Fields::new();
    for (flag, key) in [("--model", "model"), ("--vad-model", "vad_model"), ("--language", "language"), ("--diarization-model", "diarization_model")] {
        if let Some(value) = take_value(&mut args, flag) {
            body.insert(key.into(), value.into());
        }
    }
    if let Some(speakers) = take_value(&mut args, "--speakers") {
        let speakers: u32 = speakers.parse().unwrap_or_else(|_| usage("--speakers needs a number"));
        body.insert("speakers".into(), speakers.into());
    }
    if take_flag(&mut args, "--diarize") {
        body.insert("diarize".into(), true.into());
    }
    let [file] = args.as_slice() else { usage("transcribe needs one <file>") };
    body.insert("path".into(), absolute(Path::new(file)).display().to_string().into());
    let reply = call(client, "transcribe_file", body)?;
    let job: protocol::JobReply = reply.parse()?;
    print_ok(reply);
    // The events of the job have no id. The job ends with job_done or job_failed.
    let events = client.events();
    loop {
        match events.recv() {
            Ok(Event::Host { id, event }) => {
                print_event(id.as_deref(), &event);
                match event {
                    HostEvent::JobDone { job: done, .. } if done == job.job => return Ok(()),
                    HostEvent::JobFailed { job: failed, message, code } if failed == job.job => {
                        return Err(Error::Host(protocol::ErrorReply { code, message, capability: None, reason: None }));
                    }
                    _ => {}
                }
            }
            Ok(other) => print_other(&other),
            Err(_) => return Err(Error::Gone),
        }
    }
}

fn session(client: &Client, mut args: Vec<String>) -> Result<(), Error> {
    let source = take_value(&mut args, "--source").unwrap_or_else(|| usage("session needs --source client or --source mic"));
    let mut body = Fields::new();
    for (flag, key) in [("--model", "model"), ("--preview-model", "preview_model"), ("--vad-model", "vad_model"), ("--language", "language")] {
        if let Some(value) = take_value(&mut args, flag) {
            body.insert(key.into(), value.into());
        }
    }
    if let Some(folder) = take_value(&mut args, "--record-to") {
        body.insert("record_to".into(), absolute(Path::new(&folder)).display().to_string().into());
    }
    if take_flag(&mut args, "--diarize") {
        body.insert("diarize".into(), true.into());
    }
    body.insert("sources".into(), json!([source]));
    let feed: Box<Feed> = match source.as_str() {
        "client" => {
            let file = take_value(&mut args, "--file").unwrap_or_else(|| usage("session --source client needs --file <wav>"));
            let speed: f64 = take_value(&mut args, "--speed").map_or(1.0, |s| s.parse().unwrap_or_else(|_| usage("--speed needs a number")));
            let samples = read_wav(Path::new(&file)).unwrap_or_else(|e| fail_with(&e));
            Box::new(move |client: &Client, session: u64| send_audio(client, session, &samples, speed))
        }
        "mic" => {
            let seconds: f64 = take_value(&mut args, "--seconds").and_then(|s| s.parse().ok()).unwrap_or_else(|| usage("session --source mic needs --seconds N"));
            Box::new(move |_: &Client, _: u64| {
                std::thread::sleep(Duration::from_secs_f64(seconds));
                Ok(())
            })
        }
        other => usage(&format!("unknown source {other}")),
    };
    no_more(&args);
    let reply = call(client, "session_start", body)?;
    let session: protocol::SessionReply = reply.parse()?;
    print_ok(reply);
    // The events of the session (partial, segment) come while the audio goes out.
    let events = client.events();
    let fed = std::thread::scope(|scope| {
        let feeder = scope.spawn(|| feed(client, session.session));
        while !feeder.is_finished() {
            print_waiting(&events, Duration::from_millis(50));
        }
        feeder.join().unwrap_or(Err(Error::Gone))
    });
    fed?;
    call(client, "session_stop", fields(json!({"session": session.session}))).map(print_ok)
}

/// What sends the audio of a session, until the session can stop.
type Feed = dyn FnOnce(&Client, u64) -> Result<(), Error> + Send;

/// Send `samples` as `session_audio` in parts of 100 ms, at `speed` times real
/// time (0: as fast as possible). Without an `id`, so no reply waits.
fn send_audio(client: &Client, session: u64, samples: &[i16], speed: f64) -> Result<(), Error> {
    const PART: usize = 1_600;
    let started = Instant::now();
    for (i, part) in samples.chunks(PART).enumerate() {
        if speed > 0.0 {
            let due = started + Duration::from_secs_f64(i as f64 * 0.1 / speed);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        client.send("session_audio", fields(json!({"session": session, "pcm": pcm::encode_i16(part)})))?;
    }
    Ok(())
}

fn enhance(client: &Client, mut args: Vec<String>) -> Result<(), Error> {
    let mut body = Fields::new();
    for (flag, key) in [("--style", "style"), ("--prompt", "prompt"), ("--provider", "provider")] {
        if let Some(value) = take_value(&mut args, flag) {
            body.insert(key.into(), value.into());
        }
    }
    let [text] = args.as_slice() else { usage("enhance needs one <text>") };
    body.insert("text".into(), text.clone().into());
    call(client, "enhance", body).map(print_ok)
}

/// Print each `dictation_done` until Sayso closes.
fn watch(client: &Client) -> Result<(), Error> {
    print_ok(call(client, "subscribe", fields(json!({"events": ["dictation_done"]})))?);
    for event in client.events().iter() {
        match event {
            Event::Host { id, event } => print_event(id.as_deref(), &event),
            Event::HostGone => {
                print_json(&json!({"type": "host_gone"}));
                return Ok(());
            }
            other => print_other(&other),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Requests and output
// ---------------------------------------------------------------------------

/// Send a request and wait for its reply. The events that arrive meanwhile
/// are printed first, in the order of the socket.
fn call(client: &Client, kind: &str, body: Fields) -> Result<Reply, Error> {
    let events = client.events();
    std::thread::scope(|scope| {
        let request = scope.spawn(|| client.request(kind, body));
        while !request.is_finished() {
            print_waiting(&events, Duration::from_millis(20));
        }
        // The client puts the events of a request on the channel before its
        // reply. The reply comes on another channel, so an event without an
        // `id` that the host sent just after the reply (the first
        // `job_progress` of a job) can print before the reply line.
        for event in events.try_iter() {
            print_any(&event);
        }
        let reply = request.join().unwrap_or(Err(Error::Gone));
        if let Err(Error::Host(error)) = &reply {
            println!("{}", protocol::encode(None, &WireReply::Error(error.clone())));
        }
        reply
    })
}

fn print_waiting(events: &crossbeam_channel::Receiver<Event>, wait: Duration) {
    if let Ok(event) = events.recv_timeout(wait) {
        print_any(&event);
    }
}

fn print_any(event: &Event) {
    match event {
        Event::Host { id, event } => print_event(id.as_deref(), event),
        other => print_other(other),
    }
}

fn print_event(id: Option<&str>, event: &HostEvent) {
    println!("{}", protocol::encode(id, event));
}

fn print_other(event: &Event) {
    match event {
        Event::Error(error) => println!("{}", protocol::encode(None, &WireReply::Error(error.clone()))),
        Event::HostGone => print_json(&json!({"type": "host_gone"})),
        Event::Host { .. } => {}
    }
}

fn print_ok(reply: Reply) {
    println!("{}", protocol::encode(None, &WireReply::Ok(reply.fields)));
}

fn print_json(value: &Value) {
    println!("{value}");
}

fn fields(value: Value) -> Fields {
    match value {
        Value::Object(fields) => fields,
        _ => Fields::new(),
    }
}

fn fail(error: &Error) -> ! {
    // A reply `error` is already on stdout.
    if !matches!(error, Error::Host(_)) {
        eprintln!("sayso-ext: {error}");
    }
    std::process::exit(1)
}

fn fail_with(message: &str) -> ! {
    eprintln!("sayso-ext: {message}");
    std::process::exit(1)
}

fn usage(message: &str) -> ! {
    eprintln!("sayso-ext: {message}\n\n{USAGE}");
    std::process::exit(2)
}

// ---------------------------------------------------------------------------
// Arguments and files
// ---------------------------------------------------------------------------

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let found = args.iter().position(|arg| arg == flag);
    if let Some(i) = found {
        args.remove(i);
    }
    found.is_some()
}

fn take_value(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|arg| arg == flag)?;
    if i + 1 >= args.len() {
        usage(&format!("{flag} needs a value"));
    }
    args.remove(i);
    Some(args.remove(i))
}

fn no_more(args: &[String]) {
    if let Some(extra) = args.first() {
        usage(&format!("unexpected argument {extra}"));
    }
}

/// Sayso needs absolute paths.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The samples of a WAV file with 16 kHz mono 16-bit PCM, the format of
/// `session_audio`.
fn read_wav(path: &Path) -> Result<Vec<i16>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let wrong = || format!("{} is not a 16 kHz mono 16-bit WAV file. Convert it first, for example with: ffmpeg -i <file> -ar 16000 -ac 1 -c:a pcm_s16le out.wav", path.display());
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(wrong());
    }
    let mut at = 12;
    let mut format_ok = false;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let format = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                format_ok = format == 1 && channels == 1 && rate == 16_000 && bits == 16;
            }
            b"data" if format_ok => return Ok(body.as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect()),
            _ => {}
        }
        // Chunks have an even length.
        at += 8 + len + (len & 1);
    }
    Err(wrong())
}
