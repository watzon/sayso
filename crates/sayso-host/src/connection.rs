//! One connection: a thread that reads requests, a thread that writes replies
//! and events, and a worker thread for each request that takes time.
//!
//! The reader answers `hello`, `bye`, and the requests that fail at once. A
//! request that waits (for the engine, or for the user) runs on a worker with
//! a [`Responder`], so the reader goes on and the app can send another
//! request on the same connection.

use crate::approval::{self, HelloStep};
use crate::identity::{Caller, Peer};
use crate::responder::{Notifier, Outgoing, Responder};
use crate::sandbox::{Attested, FileAccess};
use crate::transport::Conn;
use crate::Shared;
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::Mutex;
use sayso_protocol::{Capability, DecodeError, Decoded, ErrorCode, ErrorReply, Event, Hello, Kind, Reply, Request, Subscribe, SubscribeReply, decode, encode};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// A longer line is not a request. Audio comes in small parts.
const MAX_LINE: u64 = 16 * 1024 * 1024;

/// The line before the identity of the writer is known. A `hello` is small.
const MAX_FIRST_LINE: u64 = 64 * 1024;

/// A program that says nothing for this long loses its connection. It has no
/// identity yet, so nobody can ask it to stop.
const FIRST_LINE_WAIT: Duration = Duration::from_secs(5);

/// More connections than any real set of apps. Each one holds file handles of
/// Sayso, and a program in a sandbox can open them without any approval.
pub(crate) const MAX_CONNECTIONS: usize = 64;

/// The event after each insert of a dictation.
pub(crate) const DICTATION_DONE: &str = "dictation_done";

/// The events that an app can subscribe to, and the capability that
/// each one needs.
const EVENTS: [(&str, Capability); 1] = [(DICTATION_DONE, Capability::DictationObserve)];

/// A connection in the registry of the host.
pub(crate) struct Entry {
    /// The key of the app, when the program has an identity.
    pub app_id: Option<String>,
    outgoing: Sender<Outgoing>,
    writer: Option<JoinHandle<()>>,
    /// The events of `subscribe`.
    events: Vec<&'static str>,
}

impl Entry {
    fn new(outgoing: Sender<Outgoing>, writer: JoinHandle<()>) -> Entry {
        Entry { app_id: None, outgoing, writer: Some(writer), events: Vec::new() }
    }

    /// Send `host_closing`, then close. Gives the writer thread, to wait for it.
    pub fn close_with_host_closing(&mut self) -> Option<JoinHandle<()>> {
        let _ = self.outgoing.send(Outgoing::Line(encode(None, &Event::HostClosing)));
        let _ = self.outgoing.send(Outgoing::Close);
        self.writer.take()
    }
}

pub(crate) fn serve(shared: &Arc<Shared>, number: u64, conn: Conn) {
    let Conn { reader, writer, closer, peer, attested } = conn;
    let (outgoing, messages) = unbounded();
    // False when the reader ends. Work for this connection stops then.
    let open = Arc::new(AtomicBool::new(true));
    let notifier = Notifier::new(outgoing.clone(), open.clone());
    let spawned = {
        let closer = closer.clone();
        std::thread::Builder::new().name("sayso-host-write".into()).spawn(move || write_loop(writer, &messages, &*closer))
    };
    let writer = match spawned {
        Ok(writer) => writer,
        Err(e) => {
            log::warn!("host: cannot start a connection thread: {e}");
            closer();
            return;
        }
    };
    {
        let mut connections = shared.connections.lock();
        if shared.closing.load(Ordering::SeqCst) {
            drop(connections);
            let mut entry = Entry::new(outgoing, writer);
            if let Some(writer) = entry.close_with_host_closing() {
                let _ = writer.join();
            }
            return;
        }
        connections.insert(number, Entry::new(outgoing.clone(), writer));
    }

    let mut connection = Connection { shared, number, caller: None, writer: None, attested, files: FileAccess::Same, jobs_with_file: Arc::default(), notifier, jobs: Mutex::new(HashSet::new()), open: open.clone(), asking: Arc::new(AtomicBool::new(false)) };
    let first_line = Arc::new(AtomicBool::new(false));
    {
        let (first_line, closer) = (first_line.clone(), closer.clone());
        let _ = std::thread::Builder::new().name("sayso-host-first-line".into()).spawn(move || {
            std::thread::sleep(FIRST_LINE_WAIT);
            if !first_line.load(Ordering::SeqCst) {
                log::debug!("host: a program said nothing, and its connection closes");
                closer();
            }
        });
    }
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        let limit = if connection.caller.is_some() { MAX_LINE } else { MAX_FIRST_LINE };
        match (&mut reader).take(limit + 1).read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                log::debug!("host: a connection failed: {e}");
                break;
            }
        }
        first_line.store(true, Ordering::SeqCst);
        if line.len() as u64 > limit {
            let _ = outgoing.send(Outgoing::Line(encode(None, &Reply::error(Some(ErrorCode::BadRequest), "the line is longer than 16 MiB"))));
            break;
        }
        // Strict: the bridge reads the line as JSON too, and both must read the same thing.
        let Ok(text) = std::str::from_utf8(&line) else {
            let _ = outgoing.send(Outgoing::Line(encode(None, &Reply::error(Some(ErrorCode::BadRequest), "the line is not UTF-8"))));
            break;
        };
        let text = text.trim_end_matches(['\n', '\r']);
        if text.trim().is_empty() {
            continue;
        }
        // After the read: the system knows the writer of the line only now.
        if !connection.check_writer(peer()) {
            break;
        }
        if connection.handle(text) {
            break;
        }
    }

    open.store(false, Ordering::SeqCst);
    // The jobs and the live session of an app end with its connection.
    if let Some(jobs) = &shared.jobs {
        jobs.cancel_connection(number);
    }
    if let Some(live) = &shared.live {
        live.connection_closed(number);
    }
    let _ = outgoing.send(Outgoing::Close);
    let entry = shared.connections.lock().remove(&number);
    // Wait until the last reply is out. `Host::close` waits for the writers
    // that it took.
    if let Some(writer) = entry.and_then(|mut entry| entry.writer.take()) {
        let _ = writer.join();
    }
    if let Some(caller) = &connection.caller {
        log::info!("host: {} disconnected", caller.name);
    }
}

fn write_loop(mut writer: Box<dyn Write + Send>, messages: &Receiver<Outgoing>, closer: &dyn Fn()) {
    for message in messages.iter() {
        match message {
            Outgoing::Line(mut line) => {
                line.push('\n');
                if let Err(e) = writer.write_all(line.as_bytes()).and_then(|()| writer.flush()) {
                    log::debug!("host: cannot write to a connection: {e}");
                    break;
                }
            }
            Outgoing::Close => break,
        }
    }
    closer();
}

struct Connection<'a> {
    shared: &'a Arc<Shared>,
    /// The number of the connection in the host.
    number: u64,
    /// The app, from its first line. None before it, and when the system gives
    /// no identity that the host can trust: the app gets `not_approved` with
    /// no question.
    caller: Option<Caller>,
    /// The process that wrote the first line.
    writer: Option<Peer>,
    /// What `sayso-bridge` says about the app, until the first line.
    attested: Option<Attested>,
    /// How the host opens a file that the app names.
    files: FileAccess,
    /// See `job_requests::MAX_JOBS_WITH_FILE`.
    jobs_with_file: Arc<AtomicUsize>,
    notifier: Notifier,
    /// The jobs that this connection started.
    jobs: Mutex<HashSet<u64>>,
    /// False when the reader ends.
    open: Arc<AtomicBool>,
    /// `hello` of this connection waits for the user.
    asking: Arc<AtomicBool>,
}

impl Connection<'_> {
    fn responder(&self, id: Option<&str>) -> Responder {
        Responder::new(id.map(str::to_owned), self.notifier.clone())
    }

    /// The first line tells who the app is. A later line from another process
    /// says false: the connection closes (`docs/sdk.md`, "Security").
    fn check_writer(&mut self, peer: Result<Peer, String>) -> bool {
        let Some(writer) = &self.writer else {
            self.identify(peer);
            return true;
        };
        match peer {
            Ok(peer) if writer.same_process(&peer) => true,
            Ok(_) => {
                log::warn!("host: another process wrote to the connection of {}, so it closes", self.caller.as_ref().map_or("?", |caller| &caller.name));
                false
            }
            Err(e) => {
                log::warn!("host: a connection closes: {e}");
                false
            }
        }
    }

    fn identify(&mut self, peer: Result<Peer, String>) {
        let attested = self.attested.take();
        let caller = peer.and_then(|peer| {
            let caller = match attested {
                Some(attested) => {
                    self.files = attested.files;
                    attested.caller?
                }
                None => {
                    let caller = self.shared.identify.identify(&peer)?;
                    self.files = FileAccess::of(&caller, &peer)?;
                    caller
                }
            };
            self.writer = Some(peer);
            Ok(caller)
        });
        let caller = match caller {
            Ok(caller) => caller,
            Err(e) => {
                log::warn!("host: a program has no identity: {e}");
                return;
            }
        };
        log::info!("host: {} connected", caller.name);
        if let Some(entry) = self.shared.connections.lock().get_mut(&self.number) {
            entry.app_id = Some(caller.identity.id());
        }
        self.caller = Some(caller);
    }

    /// Answer one line, and say true when the connection closes after it.
    fn handle(&self, line: &str) -> bool {
        let decoded = decode::<Request>(line);
        let Some(caller) = &self.caller else {
            let id = match &decoded {
                Ok(Decoded::Known(message)) => message.id.clone(),
                Ok(Decoded::Unknown { id, .. }) | Err(DecodeError::BadFields { id, .. }) => id.clone(),
                Err(DecodeError::NotAMessage(_)) => None,
            };
            self.responder(id.as_deref()).reply(approval::not_approved());
            return true;
        };
        let message = match decoded {
            Err(DecodeError::NotAMessage(message)) => {
                self.responder(None).reply(Reply::error(Some(ErrorCode::BadRequest), message));
                return false;
            }
            Err(DecodeError::BadFields { id, kind, message }) => {
                self.responder(id.as_deref()).reply(Reply::error(Some(ErrorCode::BadRequest), format!("bad {kind} request: {message}")));
                return false;
            }
            Ok(Decoded::Unknown { id, kind, .. }) => {
                self.responder(id.as_deref()).reply(Reply::error(None, format!("unknown request type {kind}")));
                return false;
            }
            Ok(Decoded::Known(message)) => message,
        };
        let responder = self.responder(message.id.as_deref());
        if matches!(message.body, Request::Bye) {
            responder.reply(Reply::ok());
            return true;
        }
        let request = match message.body {
            Request::Hello(hello) => return self.hello(caller, hello, responder),
            request => request,
        };
        // Read on each request, so a grant that the user takes back stops the next one.
        let apps = self.shared.apps.current();
        let Some(record) = apps.find(&caller.identity).filter(|record| record.allowed) else {
            if self.asking.load(Ordering::SeqCst) {
                responder.reply(Reply::error(Some(ErrorCode::BadRequest), "wait for the reply to hello: the user has not answered yet"));
                return false;
            }
            log::info!("host: {} is not approved", caller.name);
            responder.reply(approval::not_approved());
            return true;
        };
        let granted = record.capabilities();
        if let Some(missing) = request.capabilities().into_iter().find(|capability| !granted.contains(capability)) {
            responder.reply(capability_error(&missing));
            return false;
        }
        match request {
            Request::ListModels(request) => self.spawn("list-models", responder, move |shared, _, responder| crate::models::list_models(shared, &request, responder)),
            Request::DownloadModel(request) => {
                self.spawn("download", responder, move |shared, app_id, responder| crate::download::download_model(shared, app_id, &request, responder));
            }
            Request::TranscribeFile(mut request) => {
                let file = match self.files.open(&request.path) {
                    Ok(file) => file,
                    Err(message) => {
                        responder.reply(Reply::error(Some(ErrorCode::BadRequest), message));
                        return false;
                    }
                };
                if let Some(file) = &file {
                    request.path = file.path.clone();
                }
                crate::job_requests::transcribe_file(self.shared, self.number, &self.jobs, &self.jobs_with_file, &request, file, responder);
            }
            Request::CancelJob(request) => crate::job_requests::cancel_job(self.shared, self.number, &self.jobs, &request, responder),
            Request::SessionStart(request) if request.record_to.is_some() && self.files.has_another_view() => {
                responder.reply(Reply::error(Some(ErrorCode::Unsupported), "record_to does not work across a Flatpak sandbox, because Sayso and the app see other files. Start the session without it, and write the audio in the app"));
            }
            Request::SessionStart(request) => {
                let number = self.number;
                let pid = self.writer.as_ref().and_then(Peer::pid);
                self.spawn("live-start", responder, move |shared, app_id, responder| crate::live_requests::session_start(shared, number, app_id, pid, &request, responder));
            }
            Request::SessionAudio(request) => crate::live_requests::session_audio(self.shared, self.number, &request, responder),
            Request::SessionStop(request) => {
                let number = self.number;
                self.spawn("live-stop", responder, move |shared, _, responder| crate::live_requests::session_stop(shared, number, &request, responder));
            }
            Request::Enhance(request) => self.spawn("enhance", responder, move |shared, app_id, responder| crate::enhance::enhance(shared, app_id, &request, responder)),
            Request::Subscribe(request) => self.subscribe(&request, responder),
            request => responder.reply(Reply::error(Some(ErrorCode::Unsupported), format!("this version of Sayso cannot run {} yet", request.kind()))),
        }
        false
    }

    /// `hello`: reply at once when the record of the app has the answers.
    /// Else ask on a worker, and hold the reply until the user answered.
    /// Says true when the connection closes after it.
    fn hello(&self, caller: &Caller, hello: Hello, responder: Responder) -> bool {
        if self.asking.load(Ordering::SeqCst) {
            responder.reply(Reply::error(Some(ErrorCode::BadRequest), "an earlier hello waits for the user"));
            return false;
        }
        match approval::hello_step(self.shared, caller, &hello) {
            HelloStep::Reply(reply) => responder.reply(Reply::ok_with(&reply)),
            HelloStep::Refuse => {
                log::info!("host: {} is not approved", caller.name);
                responder.reply(approval::not_approved());
                return true;
            }
            HelloStep::Ask(question) => {
                // The client can now tell this wait from a stuck host.
                responder.event(&Event::ApprovalWaiting);
                self.asking.store(true, Ordering::SeqCst);
                let shared = self.shared.clone();
                let caller = caller.clone();
                let open = self.open.clone();
                let asking = self.asking.clone();
                // A thread that cannot start drops the responder, which sends the error.
                let spawned = std::thread::Builder::new().name("sayso-host-approval".into()).spawn(move || {
                    approval::ask_and_reply(&shared, &caller, &hello, question, &open, &asking, responder);
                });
                if let Err(e) = spawned {
                    self.asking.store(false, Ordering::SeqCst);
                    log::warn!("host: cannot start a thread for the approval question: {e}");
                }
            }
        }
        false
    }

    /// `subscribe`: the events of this connection from now on are the known
    /// names in the request. Unknown names are left out, and an empty list
    /// stops all events. The reply lists the events.
    fn subscribe(&self, request: &Subscribe, responder: Responder) {
        let events: Vec<&'static str> = EVENTS.iter().map(|(name, _)| *name).filter(|name| request.events.iter().any(|wanted| wanted == name)).collect();
        // The reply goes out under the lock, so no event of the
        // subscription comes before it.
        let mut connections = self.shared.connections.lock();
        if let Some(entry) = connections.get_mut(&self.number) {
            entry.events = events.clone();
        }
        responder.reply(Reply::ok_with(&SubscribeReply { events: events.iter().map(|name| name.to_string()).collect() }));
    }

    /// Run `work` on a thread of its own. `work` gets the host, the key of the
    /// app, and the responder, and must reply.
    fn spawn(&self, what: &str, responder: Responder, work: impl FnOnce(&Shared, &str, Responder) + Send + 'static) {
        let Some(app_id) = self.caller.as_ref().map(|caller| caller.identity.id()) else { return };
        let shared = self.shared.clone();
        // The responder moves into the closure. A thread that cannot start
        // drops the closure, and the dropped responder sends the error.
        let spawned = std::thread::Builder::new().name(format!("sayso-host-{what}")).spawn(move || work(&shared, &app_id, responder));
        if let Err(e) = spawned {
            log::warn!("host: cannot start a thread for {what}: {e}");
        }
    }
}

/// Send `event` (the event `name`) to each connection that subscribed to it,
/// when its app still has the capability of the event. The grant is
/// read for each event, so a grant that the user takes back stops the next one.
pub(crate) fn send_to_subscribers(shared: &Shared, name: &str, event: &Event) {
    if shared.is_closing() {
        return;
    }
    let Some((_, capability)) = EVENTS.iter().find(|(known, _)| *known == name) else { return };
    // Without a subscriber, the app (a dictation) does not read `apps.toml`.
    if !shared.connections.lock().values().any(|entry| entry.events.contains(&name)) {
        return;
    }
    let apps = shared.apps.current();
    let line = encode(None, event);
    let connections = shared.connections.lock();
    for entry in connections.values().filter(|entry| entry.events.contains(&name)) {
        let granted = entry.app_id.as_deref().and_then(|id| apps.record(id)).is_some_and(|record| record.allowed && record.capabilities().contains(capability));
        if granted {
            let _ = entry.outgoing.send(Outgoing::Line(line.clone()));
        }
    }
}

fn capability_error(missing: &Capability) -> Reply {
    Reply::Error(ErrorReply {
        code: Some(ErrorCode::Capability),
        message: format!("the user did not grant the capability {missing}"),
        capability: Some(missing.clone()),
        reason: None,
    })
}
