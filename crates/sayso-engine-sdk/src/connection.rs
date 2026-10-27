//! The engine process, its pipes, and the matching of replies to requests.

use crate::{EngineOptions, Error, Event, Stderr};
use crossbeam_channel::{Receiver, Sender, bounded, never, select, unbounded};
use sayso_protocol::engine::{self, FromEngine, Reply, Request};
use sayso_protocol::{Decoded, Message, decode, encode};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long a drop waits for the engine to exit after stdin closes.
const DROP_GRACE: Duration = Duration::from_secs(2);

/// A call that waits for its reply.
struct Waiter {
    reply: Sender<Result<Reply, Error>>,
    /// The events with the id of the request, for the callback of the call.
    events: Option<Sender<engine::Event>>,
}

#[derive(Default)]
struct Pending {
    waiters: HashMap<String, Waiter>,
    /// The stdout of the engine is closed.
    gone: bool,
    /// The end is on purpose (a shutdown or a drop), so it is no event.
    closing: bool,
}

pub(crate) struct Connection {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Arc<Mutex<Pending>>,
    next_id: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic in another thread does not make the data wrong here.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Connection {
    /// Start the process and the threads that read its stdout and stderr.
    pub(crate) fn spawn(options: &EngineOptions) -> Result<(Connection, Receiver<Event>), Error> {
        let mut command = Command::new(&options.program);
        command.args(&options.args).env("SAYSO_MODELS_DIR", &options.models_dir);
        for (key, value) in &options.env {
            command.env(key, value);
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        command.stderr(match &options.stderr {
            Stderr::Inherit => Stdio::inherit(),
            Stderr::Null => Stdio::null(),
            Stderr::Lines(_) => Stdio::piped(),
        });
        // An app with a window should not flash a console window (CREATE_NO_WINDOW).
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
        let mut child = command.spawn().map_err(|e| Error::Io(io::Error::new(e.kind(), format!("cannot start {}: {e}", options.program.display()))))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (events_out, events) = unbounded();
        let pending = Arc::new(Mutex::new(Pending::default()));
        let reader_pending = pending.clone();
        std::thread::Builder::new().name("sayso-engine-sdk".into()).spawn(move || read_loop(stdout, &reader_pending, &events_out))?;
        if let (Stderr::Lines(log), Some(stderr)) = (&options.stderr, child.stderr.take()) {
            let log = log.clone();
            std::thread::Builder::new().name("sayso-engine-sdk-stderr".into()).spawn(move || {
                for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
                    log(String::from_utf8_lossy(&line).trim_end_matches('\r'));
                }
            })?;
        }
        Ok((Connection { child: Mutex::new(child), stdin: Mutex::new(Some(stdin)), pending, next_id: AtomicU64::new(1) }, events))
    }

    /// Send `request` with a new id, and wait for the reply. `on_event` gets
    /// the events that the request caused, in the order of arrival, also when
    /// they come before the reply.
    pub(crate) fn call(&self, request: &Request, name: &'static str, timeout: Duration, mut on_event: Option<&mut dyn FnMut(&engine::Event)>) -> Result<Reply, Error> {
        let id = format!("r{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (reply_out, reply) = bounded(1);
        let (events_out, events) = if on_event.is_some() {
            let (out, events) = unbounded();
            (Some(out), events)
        } else {
            (None, never())
        };
        {
            let mut pending = lock(&self.pending);
            if pending.gone {
                return Err(Error::Exited);
            }
            pending.waiters.insert(id.clone(), Waiter { reply: reply_out, events: events_out });
        }
        if let Err(e) = self.write(&encode(Some(&id), request)) {
            lock(&self.pending).waiters.remove(&id);
            return Err(e);
        }
        let deadline = Instant::now() + timeout;
        let mut events = events;
        loop {
            select! {
                recv(events) -> event => match event {
                    Ok(event) => {
                        if let Some(on_event) = on_event.as_mut() {
                            on_event(&event);
                        }
                    }
                    // The reader answered the call and dropped the sender: the reply is next.
                    Err(_) => events = never(),
                },
                recv(reply) -> result => {
                    // The events came before the reply, so they are in the channel.
                    if let Some(on_event) = on_event.as_mut() {
                        while let Ok(event) = events.try_recv() {
                            on_event(&event);
                        }
                    }
                    return result.unwrap_or(Err(Error::Exited));
                }
                default(deadline.saturating_duration_since(Instant::now())) => {
                    lock(&self.pending).waiters.remove(&id);
                    return Err(Error::Timeout { request: name, after: timeout });
                }
            }
        }
    }

    /// Send a request that has no `id` and gets no reply (`stream_audio`).
    pub(crate) fn send(&self, request: &Request) -> Result<(), Error> {
        if lock(&self.pending).gone {
            return Err(Error::Exited);
        }
        self.write(&encode(None, request))
    }

    fn write(&self, line: &str) -> Result<(), Error> {
        let mut stdin = lock(&self.stdin);
        let Some(stdin) = stdin.as_mut() else { return Err(Error::Exited) };
        let mut line = line.to_owned();
        line.push('\n');
        stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush()).map_err(|e| if e.kind() == io::ErrorKind::BrokenPipe { Error::Exited } else { Error::Io(e) })
    }

    pub(crate) fn process_id(&self) -> u32 {
        lock(&self.child).id()
    }

    pub(crate) fn is_running(&self) -> bool {
        !lock(&self.pending).gone
    }

    /// The end that follows is on purpose: it sends no [`Event::Exited`].
    pub(crate) fn mark_closing(&self) {
        lock(&self.pending).closing = true;
    }

    /// Close stdin, wait up to `grace` for the engine to exit, and kill it
    /// when it does not. Calling it again is harmless.
    pub(crate) fn finish(&self, grace: Duration) -> io::Result<ExitStatus> {
        self.mark_closing();
        drop(lock(&self.stdin).take());
        let mut child = lock(&self.child);
        let deadline = Instant::now() + grace;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                return child.wait();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.finish(DROP_GRACE);
    }
}

/// Read the stdout of the engine until it closes.
fn read_loop(stdout: impl io::Read, pending: &Mutex<Pending>, events_out: &Sender<Event>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&line);
        match decode::<FromEngine>(text.trim_end()) {
            Ok(Decoded::Known(Message { id, body: FromEngine::Reply(reply) })) => {
                // A reply without the id of a request (for a line that the
                // engine could not read) has nobody to go to.
                let waiter = id.and_then(|id| lock(pending).waiters.remove(&id));
                if let Some(waiter) = waiter {
                    let _ = waiter.reply.send(match reply {
                        Reply::Error(error) => Err(Error::Engine(error)),
                        reply => Ok(reply),
                    });
                }
            }
            Ok(Decoded::Known(Message { id, body: FromEngine::Event(event) })) => {
                let sink = id.as_ref().and_then(|id| lock(pending).waiters.get(id).and_then(|w| w.events.clone()));
                if let Some(sink) = sink {
                    let _ = sink.send(event.clone());
                }
                let _ = events_out.send(Event::Engine { id, event });
            }
            // A known reply with bad fields fails its call. Other bad lines have nobody to go to.
            Err(sayso_protocol::DecodeError::BadFields { id: Some(id), kind, message }) => {
                if let Some(waiter) = lock(pending).waiters.remove(&id) {
                    let _ = waiter.reply.send(Err(Error::Protocol(format!("the engine sent a bad {kind}: {message}"))));
                }
            }
            // A client ignores a type that it does not know.
            Ok(Decoded::Unknown { .. }) | Err(_) => {}
        }
    }
    let closing = {
        let mut pending = lock(pending);
        pending.gone = true;
        pending.waiters.clear();
        pending.closing
    };
    if !closing {
        let _ = events_out.send(Event::Exited);
    }
}
