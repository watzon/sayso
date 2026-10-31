//! [`Speech`]: one API, two modes, and the changes between them (`docs/sdk.md`,
//! "Mode changes in `Mode::Auto`").
//!
//! The rules of this file:
//!
//! - **One lock.** `State` is behind one lock. A request starts under it, and a change of mode
//!   happens under it. Nobody waits for the engine, for the socket, or for the user with it.
//!   The sinks of local work take it, so never call the queue or the sessions, and never drop
//!   a [`LocalRun`], while you hold it.
//! - **One end for each piece of work.** `State::jobs` and `State::sessions` are the table of
//!   open work. Each end removes its entry, and only the end that removed it goes to the app.
//! - **Generations.** Each connection and each local run has a number, and a message of an
//!   older one is dropped. Also the reader of a connection that is not current drops all.

use crate::local::{self, LocalRun};
use crate::protocol::{self, Capability, ErrorCode, ErrorReply, Fields, HelloReply, PROTOCOL, Reply};
use crate::transport::{self, SocketId};
use crate::wire::{Incoming, Wire};
use crate::{ActiveMode, Error, Event, Features, Mode, Options, client};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use parking_lot::{Condvar, Mutex};
use sayso_speech::{EventSink, ModelResolver};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

/// The id of `hello`. The other requests have ids of the form `r<number>`.
const HELLO: &str = "hello";

/// How often a change that waits for the end of work looks again, also without a wake-up.
const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_millis(100);

/// The app uses speech through this. See the docs of the crate. Dropping it ends its
/// work and closes its connection or stops its engine.
pub struct Speech {
    shared: Arc<Shared>,
    watcher: Mutex<Option<JoinHandle<()>>>,
}

pub(crate) struct Shared {
    pub options: Options,
    socket: Result<PathBuf, String>,
    state: Mutex<State>,
    /// Told when work ends, the mode changes, or the SDK closes.
    changed: Condvar,
    events_out: Sender<Event>,
    events: Receiver<Event>,
    next_generation: AtomicU64,
    /// One local start at a time. It takes seconds, so it is not the one lock.
    local_start: Mutex<()>,
    /// The connection whose `hello` runs, so a close can end the wait for the user.
    connecting: Mutex<Option<Arc<HostConnection>>>,
    closing: AtomicBool,
    /// [`Features::system_audio`] of local mode, for a change of mode under the lock.
    local_system_audio: AtomicBool,
}

struct State {
    route: Route,
    /// The requests that have no reply yet.
    in_flight: usize,
    /// The requests of the connection that wait for the host.
    pending: HashMap<String, Pending>,
    jobs: HashMap<u64, Place>,
    sessions: HashMap<u64, SessionWork>,
    /// From the numbers of the host to the numbers of the app.
    host_jobs: HashMap<u64, u64>,
    host_sessions: HashMap<u64, u64>,
    next_job: u64,
    next_session: u64,
    next_request: u64,
    /// The socket at the last try, and the time of that try.
    tried: (Option<SocketId>, Instant),
}

enum Route {
    Sayso(Arc<HostConnection>),
    Local { generation: u64, run: Option<Arc<LocalRun>> },
    /// [`Mode::Sayso`] after the connection ended.
    Gone,
}

/// Where a job or a session runs, and its number there.
#[derive(Clone, Copy)]
enum Place {
    Host(u64),
    /// None until the queue or the sessions gave a number.
    Local(Option<u64>),
}

struct SessionWork {
    place: Place,
    /// The id of the `session_stop` that waits. [`LOCAL_STOP`] for a stop in local mode.
    stop: Option<String>,
}

const LOCAL_STOP: &str = "local";

struct Pending {
    what: Awaited,
    reply: Sender<Result<Fields, ErrorReply>>,
}

/// What the reader does with the `ok` of a request, before the next message.
enum Awaited {
    Plain,
    Job,
    Session,
    Stop(u64),
}

struct HostConnection {
    wire: Wire,
    generation: u64,
    socket: Option<SocketId>,
    /// The reader ended. Set before the reader takes the lock.
    dead: Arc<AtomicBool>,
    hello: OnceLock<HelloReply>,
}

impl HostConnection {
    fn hello(&self) -> Option<&HelloReply> {
        self.hello.get()
    }
}

impl Drop for HostConnection {
    fn drop(&mut self) {
        self.wire.close();
    }
}

impl State {
    /// `docs/sdk.md`, "Active work". The approval question is active work by itself: the
    /// watcher finishes `hello` before it looks here.
    fn active(&self) -> bool {
        self.in_flight > 0 || !self.jobs.is_empty() || !self.sessions.is_empty()
    }
}

fn error(code: ErrorCode, message: impl Into<String>) -> ErrorReply {
    ErrorReply { code: Some(code), message: message.into(), capability: None, reason: None }
}

fn fields_of(body: &impl Serialize) -> Fields {
    match serde_json::to_value(body) {
        Ok(serde_json::Value::Object(fields)) => fields,
        _ => Fields::new(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(fields: &Fields, kind: &str) -> Result<T, ErrorReply> {
    protocol::parse_ok(fields).map_err(|e| ErrorReply { code: None, message: format!("Sayso sent an unexpected reply to {kind}: {e}"), capability: None, reason: None })
}

/// The event with the numbers of the app. None when a number has no entry: the event
/// belongs to work that ended, or to work of an older connection.
fn retarget(event: protocol::Event, job: impl Fn(u64) -> Option<u64>, session: impl Fn(u64) -> Option<u64>) -> Option<protocol::Event> {
    use protocol::Event as E;
    Some(match event {
        E::JobProgress { job: id, fraction } => E::JobProgress { job: job(id)?, fraction },
        E::JobDone { job: id, text } => E::JobDone { job: job(id)?, text },
        E::JobFailed { job: id, message, code } => E::JobFailed { job: job(id)?, message, code },
        E::Segment(mut segment) => {
            segment.job = match segment.job {
                Some(id) => Some(job(id)?),
                None => None,
            };
            segment.session = match segment.session {
                Some(id) => Some(session(id)?),
                None => None,
            };
            E::Segment(segment)
        }
        E::Speakers(mut speakers) => {
            speakers.job = match speakers.job {
                Some(id) => Some(job(id)?),
                None => None,
            };
            speakers.session = match speakers.session {
                Some(id) => Some(session(id)?),
                None => None,
            };
            E::Speakers(speakers)
        }
        E::Partial(mut partial) => {
            partial.session = session(partial.session)?;
            E::Partial(partial)
        }
        E::SourceGap { session: id, source, ms } => E::SourceGap { session: session(id)?, source, ms },
        E::SourceEnded { session: id, source, reason } => E::SourceEnded { session: session(id)?, source, reason },
        E::SessionEnded { session: id, text, code } => E::SessionEnded { session: session(id)?, text, code },
        other => other,
    })
}

impl Shared {
    fn features(&self, route: &Route) -> Features {
        match route {
            Route::Sayso(connection) => {
                let hello = connection.hello();
                Features {
                    enhance: true,
                    subscribe: true,
                    download_question: true,
                    system_audio: hello.is_some_and(|hello| hello.capabilities.contains(&Capability::AudioSystem)),
                    capabilities: hello.map(|hello| hello.capabilities.clone()).unwrap_or_default(),
                    denied: hello.map(|hello| hello.denied.clone()).unwrap_or_default(),
                    host: hello.map(|hello| hello.features.clone()).unwrap_or_default(),
                    engine: None,
                }
            }
            Route::Local { run, .. } => Features {
                enhance: false,
                subscribe: false,
                download_question: false,
                system_audio: self.local_system_audio.load(Ordering::SeqCst),
                capabilities: vec![Capability::ModelsRead, Capability::ModelsDownload, Capability::TranscribeFile, Capability::SessionLive, Capability::AudioMic, Capability::AudioSystem],
                denied: Vec::new(),
                host: Vec::new(),
                engine: run.as_ref().map(|run| run.engine_features()),
            },
            Route::Gone => Features { enhance: false, subscribe: false, download_question: false, system_audio: false, capabilities: Vec::new(), denied: Vec::new(), host: Vec::new(), engine: None },
        }
    }

    /// Look at the permission of the system again. Without the lock: it asks a service of the system.
    fn refresh_system_audio(&self) {
        let usable = self.options.capture.is_some() || local::real_system_audio_usable();
        self.local_system_audio.store(usable, Ordering::SeqCst);
    }

    fn send(&self, event: Event) {
        let _ = self.events_out.send(event);
    }

    /// Open a connection and finish `hello`. Nothing changes for the app yet. Waits for the
    /// answer of the user when Sayso asks a question, and holds no lock while it waits.
    fn open_host(self: &Arc<Self>) -> Result<Arc<HostConnection>, ErrorReply> {
        let socket = self.socket.as_ref().map_err(|e| error(ErrorCode::HostNotRunning, e.clone()))?;
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        let id = transport::socket_id(socket);
        let dead = Arc::new(AtomicBool::new(false));
        let asking = Arc::new(AtomicBool::new(false));
        let (hello_out, hello_in) = bounded(1);
        let mut hello_out = Some(hello_out);
        let wire = {
            let shared = self.clone();
            let dead = dead.clone();
            let asking = asking.clone();
            Wire::open(socket, move |incoming| match incoming {
                Incoming::Event { event: protocol::Event::ApprovalWaiting, .. } => asking.store(true, Ordering::SeqCst),
                Incoming::Reply { id: Some(id), reply } if id == HELLO => {
                    if let Some(out) = hello_out.take() {
                        let _ = out.send(reply);
                    }
                }
                Incoming::Ended { host_closing } => {
                    dead.store(true, Ordering::SeqCst);
                    hello_out = None;
                    shared.host_ended(generation, host_closing);
                }
                other => shared.host_message(generation, other),
            })
        }
        .map_err(|e| {
            if transport::not_running(&e) {
                error(ErrorCode::HostNotRunning, format!("Sayso does not run: {e}"))
            } else {
                error(ErrorCode::ConnectionLost, format!("cannot connect to Sayso: {e}"))
            }
        })?;
        let connection = Arc::new(HostConnection { wire, generation, socket: id, dead, hello: OnceLock::new() });
        *self.connecting.lock() = Some(connection.clone());
        let hello = self.finish_hello(&connection, &hello_in, &asking);
        *self.connecting.lock() = None;
        let hello = hello?;
        let _ = connection.hello.set(hello);
        Ok(connection)
    }

    /// A host that says nothing is stuck, so its `hello` has a limit. A host that says
    /// `approval_waiting` waits for a person, and a person has no limit.
    fn finish_hello(&self, connection: &HostConnection, reply: &Receiver<Reply>, asking: &AtomicBool) -> Result<HelloReply, ErrorReply> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(error(ErrorCode::ConnectionLost, "the SDK closes"));
        }
        let app = client::AppHello { name: Some(self.options.app.name.clone()), capabilities: self.options.capabilities.clone() };
        let mut fields = app.fields();
        fields.insert("id".into(), HELLO.into());
        connection.wire.write(HELLO, fields).map_err(|e| error(ErrorCode::ConnectionLost, format!("cannot talk to Sayso: {e}")))?;
        let limit = self.options.timing.hello_limit;
        let answer = match reply.recv_timeout(limit) {
            Ok(answer) => Some(answer),
            Err(RecvTimeoutError::Disconnected) => None,
            Err(RecvTimeoutError::Timeout) if asking.load(Ordering::SeqCst) => reply.recv().ok(),
            Err(RecvTimeoutError::Timeout) => return Err(error(ErrorCode::HostNotRunning, format!("Sayso did not answer hello in {} s", limit.as_secs_f32()))),
        };
        let hello: HelloReply = match answer {
            Some(Reply::Ok(fields)) => parse(&fields, HELLO)?,
            Some(Reply::Error(e)) => return Err(e),
            None => return Err(error(ErrorCode::ConnectionLost, "the connection to Sayso ended during hello")),
        };
        if hello.protocol != PROTOCOL {
            return Err(ErrorReply { code: None, message: format!("Sayso speaks protocol {}, and this SDK speaks {PROTOCOL}", hello.protocol), capability: None, reason: None });
        }
        Ok(hello)
    }

    /// A reply or an event of the connection `generation`, on its reader thread.
    fn host_message(&self, generation: u64, incoming: Incoming) {
        let mut state = self.state.lock();
        if !matches!(&state.route, Route::Sayso(connection) if connection.generation == generation) {
            return;
        }
        let state = &mut *state;
        match incoming {
            Incoming::Reply { id, reply } => self.host_reply(state, id, reply),
            Incoming::Event { event, .. } => {
                let job = |job| state.host_jobs.get(&job).copied().filter(|job| state.jobs.contains_key(job));
                let session = |session| state.host_sessions.get(&session).copied().filter(|session| state.sessions.contains_key(session));
                if let Some(event) = retarget(event, job, session) {
                    self.work_event(state, event);
                }
            }
            Incoming::Ended { .. } => {}
        }
    }

    fn host_reply(&self, state: &mut State, id: Option<String>, reply: Reply) {
        let Some(pending) = id.as_ref().and_then(|id| state.pending.remove(id)) else {
            // A late reply to a stop that a `session_ended` answered has an id, and nobody waits.
            if let (None, Reply::Error(error)) = (id, reply) {
                self.send(Event::Error(error));
            }
            return;
        };
        let result = match reply {
            Reply::Ok(fields) => Ok(fields),
            Reply::Error(error) => Err(error),
        };
        let result = match pending.what {
            Awaited::Plain => result,
            Awaited::Job => result.and_then(|mut fields| {
                let host: protocol::JobReply = parse(&fields, "transcribe_file")?;
                state.next_job += 1;
                let job = state.next_job;
                state.jobs.insert(job, Place::Host(host.job));
                state.host_jobs.insert(host.job, job);
                fields.insert("job".into(), job.into());
                Ok(fields)
            }),
            Awaited::Session => result.and_then(|mut fields| {
                let host: protocol::SessionReply = parse(&fields, "session_start")?;
                state.next_session += 1;
                let session = state.next_session;
                state.sessions.insert(session, SessionWork { place: Place::Host(host.session), stop: None });
                state.host_sessions.insert(host.session, session);
                fields.insert("session".into(), session.into());
                Ok(fields)
            }),
            // Each reply to a stop is the end of the session: a stop that fails frees it too.
            Awaited::Stop(session) => {
                state.sessions.remove(&session);
                result
            }
        };
        let _ = pending.reply.send(result);
        self.changed.notify_all();
    }

    /// The event of a piece of work. Its first end removes it from the table, and only
    /// that end goes to the app.
    fn work_event(&self, state: &mut State, event: protocol::Event) {
        use protocol::Event as E;
        match &event {
            E::JobDone { job, .. } | E::JobFailed { job, .. } => {
                if state.jobs.remove(job).is_none() {
                    return;
                }
                self.changed.notify_all();
            }
            E::SessionEnded { session, text, .. } => {
                let Some(work) = state.sessions.remove(session) else { return };
                self.changed.notify_all();
                // A stop waits: it gets this end as its reply.
                if let Some(pending) = work.stop.and_then(|stop| state.pending.remove(&stop)) {
                    let _ = pending.reply.send(Ok(fields_of(&protocol::TextReply { text: text.clone() })));
                    return;
                }
            }
            _ => {}
        }
        self.send(Event::Message(event));
    }

    /// The connection `generation` ended. One step under the lock (`docs/sdk.md`, "The change
    /// after `host_closing` or a lost connection"): end the open work, close the connection,
    /// change the route, and tell the app. A request that starts now waits for the lock, and
    /// so runs in the new mode.
    fn host_ended(&self, generation: u64, host_closing: bool) {
        let mut state = self.state.lock();
        let connection = match &state.route {
            Route::Sayso(connection) if connection.generation == generation => connection.clone(),
            _ => return,
        };
        let state = &mut *state;
        let (code, why) = if host_closing { (ErrorCode::HostClosing, "Sayso quit") } else { (ErrorCode::ConnectionLost, "The connection to Sayso was lost") };
        for (_, pending) in state.pending.drain() {
            if let Awaited::Stop(session) = pending.what {
                // The reply below is the end of this session.
                state.sessions.remove(&session);
            }
            let _ = pending.reply.send(Err(error(code.clone(), format!("{why}. It can have run this request or not."))));
        }
        for (session, _) in state.sessions.drain() {
            self.send(Event::Message(protocol::Event::SessionEnded { session, text: String::new(), code: Some(code.clone()) }));
        }
        for (job, _) in state.jobs.drain() {
            self.send(Event::Message(protocol::Event::JobFailed { job, message: format!("{why} during this job."), code: Some(code.clone()) }));
        }
        state.host_jobs.clear();
        state.host_sessions.clear();
        connection.wire.close();
        if self.options.mode == Mode::Auto {
            state.tried = (connection.socket, Instant::now());
            state.route = Route::Local { generation: self.next_generation.fetch_add(1, Ordering::SeqCst), run: None };
            let features = self.features(&state.route);
            self.send(Event::ModeChanged { mode: ActiveMode::Local, features });
        } else {
            state.route = Route::Gone;
            self.send(Event::Disconnected { code });
        }
        self.changed.notify_all();
    }

    /// An event of the local work `work` of the run `generation`, from a thread of `sayso-speech`.
    fn local_event(&self, generation: u64, job: Option<u64>, session: Option<u64>, event: protocol::Event) {
        let mut state = self.state.lock();
        if !matches!(&state.route, Route::Local { generation: current, .. } if *current == generation) {
            return;
        }
        // Nothing after the end of the work, as in Sayso mode.
        let job = job.filter(|job| state.jobs.contains_key(job));
        let session = session.filter(|session| state.sessions.contains_key(session));
        if let Some(event) = retarget(event, |_| job, |_| session) {
            self.work_event(&mut state, event);
        }
    }

    /// Start a request: it runs in the mode that this gives. The guard ends it.
    fn begin(&self) -> Result<(InFlight<'_>, Target), ErrorReply> {
        let mut state = self.state.lock();
        if self.closing.load(Ordering::SeqCst) {
            return Err(error(ErrorCode::ConnectionLost, "the SDK closes"));
        }
        let target = match &state.route {
            Route::Sayso(connection) => Target::Host(connection.clone()),
            Route::Local { generation, .. } => Target::Local(*generation),
            Route::Gone => return Err(error(ErrorCode::HostNotRunning, "the connection to Sayso ended; call reconnect")),
        };
        state.in_flight += 1;
        Ok((InFlight { shared: self }, target))
    }

    /// Send a request to the host, and wait for its reply. Each request gets one reply: the
    /// reply of the host, or the error of the end of the connection. It never goes out twice.
    fn host_call(&self, connection: &Arc<HostConnection>, kind: &str, mut fields: Fields, what: Awaited) -> Result<Fields, ErrorReply> {
        let (reply_out, reply) = bounded(1);
        let id = {
            let mut state = self.state.lock();
            if !matches!(&state.route, Route::Sayso(current) if current.generation == connection.generation) {
                return Err(error(ErrorCode::ConnectionLost, "the connection to Sayso was lost before this request"));
            }
            state.next_request += 1;
            let id = format!("r{}", state.next_request);
            if let Awaited::Stop(session) = &what {
                match state.sessions.get_mut(session) {
                    None => return Err(no_session(*session)),
                    Some(work) if work.stop.is_some() => return Err(error(ErrorCode::BadRequest, "the live session stops already")),
                    Some(work) => work.stop = Some(id.clone()),
                }
            }
            state.pending.insert(id.clone(), Pending { what, reply: reply_out });
            id
        };
        fields.insert("id".into(), id.into());
        if let Err(e) = connection.wire.write(kind, fields) {
            log::debug!("sdk: a write to Sayso failed: {e}");
            // The reader can be later than this write. The change is the same step.
            self.host_ended(connection.generation, false);
        }
        reply.recv().unwrap_or_else(|_| Err(error(ErrorCode::ConnectionLost, "the connection to Sayso was lost")))
    }

    /// The run of the engine of the app. Starts the engine at the first request that needs it,
    /// and again after the engine ended and its work ended.
    fn local_run(self: &Arc<Self>, generation: u64) -> Result<Arc<LocalRun>, ErrorReply> {
        let _one = self.local_start.lock();
        let ended = {
            let mut state = self.state.lock();
            let busy = !state.jobs.is_empty() || !state.sessions.is_empty();
            let Route::Local { run, generation: current } = &mut state.route else {
                return Err(error(ErrorCode::ConnectionLost, "the mode changed during this request"));
            };
            if *current != generation {
                return Err(error(ErrorCode::ConnectionLost, "the mode changed during this request"));
            }
            match run {
                // The work of a run whose engine ended fails by itself, and then the engine starts again.
                Some(run) if run.is_running() || busy => return Ok(run.clone()),
                run => run.take(),
            }
        };
        // Outside the lock: the queue of the old run can send events.
        drop(ended);
        let engine = self.options.engine.as_ref().ok_or_else(|| error(ErrorCode::Unsupported, "local mode needs an engine; set Options::engine"))?;
        let shared = self.clone();
        let downloads = Arc::new(move |event| shared.local_event(generation, None, None, event));
        let run = LocalRun::start(engine, &self.options.app.name, &self.options.timing, self.options.capture.as_ref(), downloads)?;
        let mut state = self.state.lock();
        if let Route::Local { run: slot, generation: current } = &mut state.route
            && *current == generation
        {
            *slot = Some(run.clone());
        }
        Ok(run)
    }

    fn local_sink(self: &Arc<Self>, generation: u64, job: Option<u64>, session: Option<u64>) -> Arc<dyn EventSink> {
        Arc::new(LocalSink { shared: self.clone(), generation, job, session })
    }

    fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Some(connection) = self.connecting.lock().take() {
            connection.wire.close();
        }
        let old = {
            let mut state = self.state.lock();
            self.changed.notify_all();
            for (_, pending) in state.pending.drain() {
                let _ = pending.reply.send(Err(error(ErrorCode::ConnectionLost, "the SDK closes")));
            }
            std::mem::replace(&mut state.route, Route::Gone)
        };
        // Outside the lock. A connection closes, and an engine stops.
        drop(old);
    }

    /// `docs/sdk.md`, "To Sayso": look for the socket while in local mode, finish `hello`, and
    /// change only when no work is active. After a failed `hello`, try again after the retry
    /// time or when the socket changes.
    fn watch(self: &Arc<Self>) {
        let Ok(socket) = self.socket.clone() else { return };
        let timing = self.options.timing;
        loop {
            {
                let mut state = self.state.lock();
                if self.closing.load(Ordering::SeqCst) {
                    return;
                }
                if !matches!(state.route, Route::Local { .. }) {
                    self.changed.wait(&mut state);
                    continue;
                }
                self.changed.wait_for(&mut state, timing.watch);
                if self.closing.load(Ordering::SeqCst) {
                    return;
                }
                if !matches!(state.route, Route::Local { .. }) {
                    continue;
                }
            }
            let now = transport::socket_id(&socket);
            if now.is_none() {
                self.state.lock().tried.0 = None;
                continue;
            }
            let (last, at) = self.state.lock().tried;
            if now == last && at.elapsed() < timing.retry {
                continue;
            }
            let opened = self.open_host();
            // On Windows a pipe cannot say that it is new, so only the retry time helps there.
            self.state.lock().tried = (now, Instant::now());
            match opened {
                Ok(connection) => self.change_to_sayso(connection),
                Err(e) => log::debug!("sdk: no change to Sayso: {e}"),
            }
        }
    }

    fn change_to_sayso(&self, connection: Arc<HostConnection>) {
        let old = {
            let mut state = self.state.lock();
            loop {
                if self.closing.load(Ordering::SeqCst) || connection.dead.load(Ordering::SeqCst) {
                    return;
                }
                if !state.active() {
                    break;
                }
                self.changed.wait_for(&mut state, LOOK_AGAIN);
            }
            let old = std::mem::replace(&mut state.route, Route::Sayso(connection));
            let features = self.features(&state.route);
            self.send(Event::ModeChanged { mode: ActiveMode::Sayso, features });
            self.changed.notify_all();
            old
        };
        // Then the engine stops, so the model is in memory once.
        drop(old);
    }
}

/// A request without a reply yet.
struct InFlight<'a> {
    shared: &'a Shared,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.shared.state.lock().in_flight -= 1;
        self.shared.changed.notify_all();
    }
}

enum Target {
    Host(Arc<HostConnection>),
    Local(u64),
}

struct LocalSink {
    shared: Arc<Shared>,
    generation: u64,
    job: Option<u64>,
    session: Option<u64>,
}

impl EventSink for LocalSink {
    fn event(&self, event: protocol::Event) {
        self.shared.local_event(self.generation, self.job, self.session, event);
    }
}

fn no_session(session: u64) -> ErrorReply {
    error(ErrorCode::BadRequest, format!("no live session {session} of this app runs"))
}

fn unsupported_locally(what: &str) -> ErrorReply {
    error(ErrorCode::Unsupported, format!("{what} is a feature of Sayso, and this app runs in local mode"))
}

impl Speech {
    /// Connect in the mode of `options` (`docs/sdk.md`, "API"):
    ///
    /// - [`Mode::Sayso`] fails with `host_not_running` when Sayso does not run, and with
    ///   `not_approved` when the user refused the app.
    /// - [`Mode::Local`] never talks to Sayso. It fails when the engine is not there.
    /// - [`Mode::Auto`] uses Sayso when it can, else local mode.
    ///
    /// When Sayso asks the user about this app, this waits for the answer. A Sayso that
    /// does not answer in [`Timing::hello_limit`](crate::Timing::hello_limit) and does not say that it asks
    /// counts as a Sayso that does not run.
    pub fn connect(options: Options) -> Result<Speech, Error> {
        let (events_out, events) = unbounded();
        let mode = options.mode;
        if mode != (Mode::Sayso { start: false }) && mode != (Mode::Sayso { start: true }) {
            let Some(engine) = &options.engine else {
                return Err(error(ErrorCode::BadRequest, "local mode needs an engine; set Options::engine"));
            };
            if mode == Mode::Local {
                engine.program().map_err(|e| error(ErrorCode::Unsupported, e.to_string()))?;
            }
        }
        let socket = match &options.socket {
            Some(socket) => Ok(socket.clone()),
            None => protocol::socket::client_socket_path().map_err(|e| e.to_string()),
        };
        let tried = socket.as_ref().ok().and_then(|socket| transport::socket_id(socket));
        let shared = Arc::new(Shared {
            options,
            socket,
            state: Mutex::new(State {
                route: Route::Gone,
                in_flight: 0,
                pending: HashMap::new(),
                jobs: HashMap::new(),
                sessions: HashMap::new(),
                host_jobs: HashMap::new(),
                host_sessions: HashMap::new(),
                next_job: 0,
                next_session: 0,
                next_request: 0,
                tried: (tried, Instant::now()),
            }),
            changed: Condvar::new(),
            events_out,
            events,
            next_generation: AtomicU64::new(1),
            local_start: Mutex::new(()),
            connecting: Mutex::new(None),
            closing: AtomicBool::new(false),
            local_system_audio: AtomicBool::new(false),
        });
        shared.refresh_system_audio();
        let speech = Speech { shared: shared.clone(), watcher: Mutex::new(None) };
        let local = || Route::Local { generation: shared.next_generation.fetch_add(1, Ordering::SeqCst), run: None };
        let route = match mode {
            Mode::Local => local(),
            Mode::Auto => match shared.open_host() {
                Ok(connection) => Route::Sayso(connection),
                Err(e) => {
                    log::debug!("sdk: local mode: {e}");
                    local()
                }
            },
            Mode::Sayso { start } => Route::Sayso(connect_to_sayso(&shared, start)?),
        };
        shared.install(route);
        if mode == Mode::Auto {
            let watcher = shared.clone();
            let handle = std::thread::Builder::new().name("sayso-sdk-watch".into()).spawn(move || watcher.watch()).map_err(|e| ErrorReply { code: None, message: format!("cannot start a thread: {e}"), capability: None, reason: None })?;
            *speech.watcher.lock() = Some(handle);
        }
        Ok(speech)
    }

    /// The mode that a request uses now.
    pub fn mode(&self) -> ActiveMode {
        match self.shared.state.lock().route {
            Route::Local { .. } => ActiveMode::Local,
            Route::Sayso(_) | Route::Gone => ActiveMode::Sayso,
        }
    }

    /// What the mode that the app has now can do. [`Event::ModeChanged`] says when that changes.
    pub fn features(&self) -> Features {
        self.shared.refresh_system_audio();
        self.shared.features(&self.shared.state.lock().route)
    }

    /// The events of work, and the changes of the mode. Read them, or they wait in the channel.
    pub fn events(&self) -> Receiver<Event> {
        self.shared.events.clone()
    }

    /// The local models of this computer with their state. With `role`, only those that have it.
    pub fn list_models(&self, role: Option<protocol::Role>) -> Result<Vec<protocol::ModelInfo>, Error> {
        let (_request, target) = self.shared.begin()?;
        match target {
            Target::Host(connection) => {
                let fields = self.shared.host_call(&connection, "list_models", fields_of(&protocol::ListModels { role }), Awaited::Plain)?;
                Ok(parse::<protocol::ListModelsReply>(&fields, "list_models")?.models)
            }
            Target::Local(generation) => {
                let run = self.shared.local_run(generation)?;
                Ok(run.models(&self.shared.options.models)?.list(role.as_ref()))
            }
        }
    }

    /// Download a model. In Sayso mode the user answers a question first, and can refuse
    /// (`declined`). `reason` is for that question. In local mode the download starts at once:
    /// the app asks its own user. `model_state` and `download_progress` come on [`Speech::events`].
    pub fn download_model(&self, model: &str, reason: &str) -> Result<(), Error> {
        let (_request, target) = self.shared.begin()?;
        match target {
            Target::Host(connection) => {
                let request = protocol::DownloadModel { model: model.to_string(), reason: reason.to_string() };
                self.shared.host_call(&connection, "download_model", fields_of(&request), Awaited::Plain).map(|_| ())
            }
            Target::Local(generation) => self.shared.local_run(generation)?.download(model, &self.shared.options.models),
        }
    }

    /// Start a job. It gives the id of the job, and the job ends with one `job_done` or
    /// `job_failed` on [`Speech::events`].
    pub fn transcribe_file(&self, request: protocol::TranscribeFile) -> Result<u64, Error> {
        let (_request, target) = self.shared.begin()?;
        match target {
            Target::Host(connection) => {
                let fields = self.shared.host_call(&connection, "transcribe_file", fields_of(&request), Awaited::Job)?;
                Ok(parse::<protocol::JobReply>(&fields, "transcribe_file")?.job)
            }
            Target::Local(generation) => {
                let run = self.shared.local_run(generation)?;
                let spec = run.models(&self.shared.options.models)?.job(&request).map_err(refusal)?;
                let job = {
                    let mut state = self.shared.state.lock();
                    state.next_job += 1;
                    let job = state.next_job;
                    state.jobs.insert(job, Place::Local(None));
                    job
                };
                match run.submit(spec, self.shared.local_sink(generation, Some(job), None)) {
                    Some(local) => {
                        // A job that ended already has no entry.
                        if let Some(place) = self.shared.state.lock().jobs.get_mut(&job) {
                            *place = Place::Local(Some(local));
                        }
                        Ok(job)
                    }
                    None => {
                        self.shared.state.lock().jobs.remove(&job);
                        Err(ErrorReply { code: None, message: "the queue of jobs closed".into(), capability: None, reason: None })
                    }
                }
            }
        }
    }

    /// Cancel a job. It ends with `job_failed`. A job of this app that ended already gets
    /// `Ok` too, so a cancel that crosses the end of the job is not an error.
    pub fn cancel_job(&self, job: u64) -> Result<(), Error> {
        let (_request, target) = self.shared.begin()?;
        let place = {
            let state = self.shared.state.lock();
            if job == 0 || job > state.next_job {
                return Err(error(ErrorCode::BadRequest, format!("this app has no job {job}")));
            }
            state.jobs.get(&job).copied()
        };
        match (target, place) {
            (Target::Host(connection), Some(Place::Host(host))) => self.shared.host_call(&connection, "cancel_job", fields_of(&protocol::CancelJob { job: host }), Awaited::Plain).map(|_| ()),
            (Target::Local(generation), Some(Place::Local(Some(local)))) => {
                self.shared.local_run(generation)?.cancel(local);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Start a live session. It gives the id of the session, and what the capture of the
    /// source `system` leaves out. It ends with the reply of [`Speech::stop_session`], or else
    /// with the event `session_ended`. A session with the source `client` gets its audio from
    /// [`Speech::session_audio`]. A refused capture gives the code `permission`.
    ///
    /// With the source `system` in local mode, this can wait for the answer of the user to a
    /// question of macOS, so call it on a thread that can wait. While it waits, the session is
    /// active work: other requests run, and [`Mode::Auto`] does not change mode.
    pub fn start_session(&self, request: protocol::SessionStart) -> Result<protocol::SessionReply, Error> {
        let (_request, target) = self.shared.begin()?;
        match target {
            Target::Host(connection) => {
                let fields = self.shared.host_call(&connection, "session_start", fields_of(&request), Awaited::Session)?;
                parse(&fields, "session_start")
            }
            Target::Local(generation) => {
                let run = self.shared.local_run(generation)?;
                let spec = run.models(&self.shared.options.models)?.session(&request).map_err(refusal)?;
                let session = {
                    let mut state = self.shared.state.lock();
                    state.next_session += 1;
                    let session = state.next_session;
                    state.sessions.insert(session, SessionWork { place: Place::Local(None), stop: None });
                    session
                };
                match run.start_session(&self.shared.options.app.name, spec, self.shared.local_sink(generation, None, Some(session))) {
                    Ok(started) => {
                        if let Some(work) = self.shared.state.lock().sessions.get_mut(&session) {
                            work.place = Place::Local(Some(started.session));
                        }
                        Ok(protocol::SessionReply { session, excludes: started.excludes })
                    }
                    Err(e) => {
                        self.shared.state.lock().sessions.remove(&session);
                        self.shared.changed.notify_all();
                        Err(ErrorReply { code: e.code, message: e.message, capability: None, reason: None })
                    }
                }
            }
        }
    }

    /// Audio for the source `client`: 16 kHz, mono. An error of the audio itself comes as
    /// [`Event::Error`], because the call does not wait for a reply.
    pub fn session_audio(&self, session: u64, samples: &[i16]) -> Result<(), Error> {
        let pcm = protocol::pcm::encode_i16(samples);
        let (route, place) = {
            let state = self.shared.state.lock();
            let Some(work) = state.sessions.get(&session) else { return Err(no_session(session)) };
            match (&state.route, work.place) {
                (Route::Sayso(connection), Place::Host(host)) => (Target::Host(connection.clone()), host),
                (Route::Local { generation, .. }, Place::Local(Some(local))) => (Target::Local(*generation), local),
                _ => return Err(no_session(session)),
            }
        };
        match route {
            Target::Host(connection) => {
                let mut fields = Fields::new();
                fields.insert("session".into(), place.into());
                fields.insert("pcm".into(), pcm.into());
                if let Err(e) = connection.wire.write("session_audio", fields) {
                    self.shared.host_ended(connection.generation, false);
                    return Err(error(ErrorCode::ConnectionLost, format!("cannot talk to Sayso: {e}")));
                }
            }
            Target::Local(generation) => {
                let run = self.shared.local_run(generation)?;
                if let Err(e) = run.audio(place, &pcm) {
                    self.shared.send(Event::Error(ErrorReply { code: e.code, message: e.message, capability: None, reason: None }));
                }
            }
        }
        Ok(())
    }

    /// Stop a live session and give its text, after its last events.
    pub fn stop_session(&self, session: u64) -> Result<String, Error> {
        let (_request, target) = self.shared.begin()?;
        let place = {
            let mut state = self.shared.state.lock();
            let Some(work) = state.sessions.get_mut(&session) else { return Err(no_session(session)) };
            if matches!(work.place, Place::Local(_)) {
                if work.stop.is_some() {
                    return Err(error(ErrorCode::BadRequest, "the live session stops already"));
                }
                work.stop = Some(LOCAL_STOP.to_string());
            }
            work.place
        };
        match (target, place) {
            (Target::Host(connection), Place::Host(host)) => {
                let fields = self.shared.host_call(&connection, "session_stop", fields_of(&protocol::SessionStop { session: host }), Awaited::Stop(session))?;
                Ok(parse::<protocol::TextReply>(&fields, "session_stop")?.text)
            }
            (Target::Local(generation), Place::Local(Some(local))) => {
                let stopped = self.shared.local_run(generation).and_then(|run| run.stop_session(local).map_err(|e| ErrorReply { code: e.code, message: e.message, capability: None, reason: None }));
                self.shared.state.lock().sessions.remove(&session);
                self.shared.changed.notify_all();
                stopped
            }
            _ => Err(no_session(session)),
        }
    }

    /// Run a style of the user on text. In Sayso mode only.
    pub fn enhance(&self, request: protocol::Enhance) -> Result<protocol::EnhanceReply, Error> {
        let (_request, target) = self.shared.begin()?;
        match target {
            Target::Host(connection) => {
                let fields = self.shared.host_call(&connection, "enhance", fields_of(&request), Awaited::Plain)?;
                parse(&fields, "enhance")
            }
            Target::Local(_) => Err(unsupported_locally("enhance")),
        }
    }

    /// Get events such as `dictation_done`. It gives the events that the host will send. In
    /// Sayso mode only, and for this connection only: subscribe again after a change of mode.
    pub fn subscribe(&self, events: &[&str]) -> Result<Vec<String>, Error> {
        let (_request, target) = self.shared.begin()?;
        match target {
            Target::Host(connection) => {
                let request = protocol::Subscribe { events: events.iter().map(|event| event.to_string()).collect() };
                let fields = self.shared.host_call(&connection, "subscribe", fields_of(&request), Awaited::Plain)?;
                Ok(parse::<protocol::SubscribeReply>(&fields, "subscribe")?.events)
            }
            Target::Local(_) => Err(unsupported_locally("subscribe")),
        }
    }

    /// [`Mode::Sayso`]: connect again after [`Event::Disconnected`]. Work does not survive
    /// the end of a connection. In the other modes the SDK does this by itself, and this does nothing.
    pub fn reconnect(&self) -> Result<(), Error> {
        let Mode::Sayso { start } = self.shared.options.mode else { return Ok(()) };
        if !matches!(self.shared.state.lock().route, Route::Gone) {
            return Ok(());
        }
        let connection = connect_to_sayso(&self.shared, start)?;
        let mut state = self.shared.state.lock();
        if !matches!(state.route, Route::Gone) {
            // Another call of this function won, and that connection is the route.
            return Ok(());
        }
        if connection.dead.load(Ordering::SeqCst) {
            return Err(error(ErrorCode::ConnectionLost, "the connection to Sayso ended just after hello"));
        }
        state.route = Route::Sayso(connection);
        let features = self.shared.features(&state.route);
        self.shared.send(Event::ModeChanged { mode: ActiveMode::Sayso, features });
        Ok(())
    }
}

impl Shared {
    fn install(&self, route: Route) {
        // Before the first request, so no other thread looks. A connection that ended in
        // between counts as ended.
        let lost = matches!(&route, Route::Sayso(connection) if connection.dead.load(Ordering::SeqCst));
        let generation = match &route {
            Route::Sayso(connection) => Some(connection.generation),
            _ => None,
        };
        self.state.lock().route = route;
        if let (true, Some(generation)) = (lost, generation) {
            self.host_ended(generation, false);
        }
    }
}

fn refusal(refusal: sayso_speech::Refusal) -> ErrorReply {
    error(refusal.code, refusal.message)
}

/// `hello` with Sayso, and with `start` a start of Sayso when it does not run.
fn connect_to_sayso(shared: &Arc<Shared>, start: bool) -> Result<Arc<HostConnection>, ErrorReply> {
    match shared.open_host() {
        Err(e) if start && e.code == Some(ErrorCode::HostNotRunning) => {}
        other => return other,
    }
    (shared.options.start)().map_err(|e| error(ErrorCode::HostNotRunning, format!("cannot start Sayso: {e}")))?;
    let wait = shared.options.timing.start_wait;
    let deadline = Instant::now() + wait;
    loop {
        match shared.open_host() {
            Err(e) if e.code == Some(ErrorCode::HostNotRunning) && Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
            Err(e) if e.code == Some(ErrorCode::HostNotRunning) => return Err(error(ErrorCode::HostNotRunning, format!("Sayso did not start in {} s: {}", wait.as_secs(), e.message))),
            other => return other,
        }
    }
}

impl Drop for Speech {
    fn drop(&mut self) {
        self.shared.close();
        if let Some(watcher) = self.watcher.lock().take() {
            let _ = watcher.join();
        }
    }
}
