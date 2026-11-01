//! The plain client of the Host API: one connection to Sayso, with no local
//! mode. Most apps use [`crate::Speech`]. Use this when you need the messages
//! of the Host API as they are, as `examples/sayso-ext.rs` and the tests of the host do.
//!
//! ```no_run
//! // Starts Sayso first when it does not run, and waits up to 10 s for it.
//! let client = sayso_sdk::client::Client::connect_or_start()?;
//! println!("Sayso {}", client.hello().host_version);
//! let reply = client.request("list_models", sayso_sdk::Fields::new());
//! # Ok::<(), sayso_sdk::client::Error>(())
//! ```

pub use crate::start::{APP_ID, Launcher, launchers, start_sayso};

use crate::protocol::{self, ErrorReply, Fields, HelloReply, PROTOCOL, Reply as WireReply};
use crate::wire::{Incoming, Wire};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long [`Client::connect_or_start`] waits for Sayso after it starts it.
pub const START_WAIT: Duration = Duration::from_secs(10);

/// The name and version of this SDK, in `hello`.
pub const SDK: &str = concat!("sayso-sdk ", env!("CARGO_PKG_VERSION"));

/// Something that the client gets from the host without asking.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// An event of the Host API. `id` is the id of the request that caused
    /// it, when a request caused it.
    Host { id: Option<String>, event: protocol::Event },
    /// An `error` without an `id`: a `session_audio` sent with
    /// [`Client::send`] failed, or the host could not read a line.
    Error(ErrorReply),
    /// Sayso quit or restarted (`host_closing`), or the connection broke.
    /// Requests fail until [`Client::reconnect`] connects again.
    HostGone,
}

/// The `ok` reply to a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub fields: Fields,
}

impl Reply {
    /// Read the fields as `T`, for example [`protocol::ListModelsReply`].
    pub fn parse<T: DeserializeOwned>(&self) -> Result<T, Error> {
        protocol::parse_ok(&self.fields).map_err(|e| Error::Protocol(format!("unexpected reply: {e}")))
    }
}

#[derive(Debug)]
pub enum Error {
    /// The socket failed, or Sayso does not run.
    Io(io::Error),
    /// The host replied `error`.
    Host(ErrorReply),
    /// The connection is gone. Call [`Client::reconnect`].
    Gone,
    /// The host sent something that this SDK cannot use.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "cannot talk to Sayso: {e}"),
            Error::Host(e) => write!(f, "Sayso replied: {e}"),
            Error::Gone => f.write_str("the connection to Sayso is gone"),
            Error::Protocol(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Host(e) => Some(e),
            Error::Gone | Error::Protocol(_) => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

/// What an app tells Sayso in `hello`, for the approval question
/// (`docs/sdk.md`, "The question").
#[derive(Debug, Clone, Default)]
pub struct AppHello {
    /// A name that the app gives. Sayso shows it only as a claim of the app.
    pub name: Option<String>,
    /// What the app wants to use. The question lists them.
    pub capabilities: Vec<protocol::Capability>,
}

impl AppHello {
    /// The fields of the `hello` request.
    pub(crate) fn fields(&self) -> Fields {
        let mut fields = Fields::new();
        fields.insert("sdk".into(), SDK.into());
        fields.insert("protocol".into(), PROTOCOL.into());
        fields.insert("features".into(), Value::Array(vec![]));
        if let Some(name) = &self.name {
            fields.insert("app".into(), serde_json::json!({ "name": name }));
        }
        if !self.capabilities.is_empty() {
            fields.insert("capabilities".into(), self.capabilities.iter().map(|capability| Value::from(capability.as_str())).collect());
        }
        fields
    }
}

/// A connection to Sayso.
pub struct Client {
    path: PathBuf,
    app: AppHello,
    connection: Connection,
    hello: HelloReply,
    next_id: AtomicU64,
    events_out: Sender<Event>,
    events: Receiver<Event>,
}

impl Client {
    /// Connect to the socket of `sayso_protocol::socket` (`SAYSO_HOST_SOCKET`,
    /// or the default path of this system) and send `hello`.
    pub fn connect() -> Result<Client, Error> {
        Client::connect_to(protocol::socket::client_socket_path()?)
    }

    /// Connect as [`Client::connect`] does. When Sayso does not run, start it
    /// ([`start_sayso`]) and wait up to [`START_WAIT`] for its socket.
    /// `connect` never starts Sayso.
    pub fn connect_or_start() -> Result<Client, Error> {
        Client::connect_or_start_with(protocol::socket::client_socket_path()?, START_WAIT, start_sayso)
    }

    /// [`Client::connect_or_start`] with the socket at `path`, a time to
    /// wait, and a way to start Sayso.
    pub fn connect_or_start_with(path: impl AsRef<Path>, wait: Duration, start: impl FnOnce() -> io::Result<()>) -> Result<Client, Error> {
        Client::connect_or_start_as(path, &AppHello::default(), wait, start)
    }

    /// [`Client::connect_or_start_with`] with what the app tells Sayso.
    pub fn connect_or_start_as(path: impl AsRef<Path>, app: &AppHello, wait: Duration, start: impl FnOnce() -> io::Result<()>) -> Result<Client, Error> {
        let path = path.as_ref();
        match Client::connect_as(path, app) {
            Err(Error::Io(e)) if crate::transport::not_running(&e) => {}
            other => return other,
        }
        start()?;
        let deadline = Instant::now() + wait;
        loop {
            match Client::connect_as(path, app) {
                Err(Error::Io(e)) if crate::transport::not_running(&e) && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                Err(Error::Io(e)) if crate::transport::not_running(&e) => {
                    return Err(Error::Io(io::Error::new(io::ErrorKind::TimedOut, format!("Sayso did not start in {} s: {e}", wait.as_secs()))));
                }
                other => return other,
            }
        }
    }

    /// Connect to the socket (the pipe on Windows) at `path` and send `hello`.
    pub fn connect_to(path: impl AsRef<Path>) -> Result<Client, Error> {
        Client::connect_as(path, &AppHello::default())
    }

    /// [`Client::connect_to`] with what the app tells Sayso. When Sayso has no
    /// answer yet, this waits until the user answers the approval question.
    pub fn connect_as(path: impl AsRef<Path>, app: &AppHello) -> Result<Client, Error> {
        let (events_out, events) = unbounded();
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path, events_out.clone())?;
        let next_id = AtomicU64::new(1);
        let hello = send_hello(&connection, &next_id, app)?;
        Ok(Client { path, app: app.clone(), connection, hello, next_id, events_out, events })
    }

    /// The reply of the host to `hello`: its version, the granted
    /// capabilities, and the capabilities that the user refused.
    pub fn hello(&self) -> &HelloReply {
        &self.hello
    }

    /// The events of the host. The channel stays the same after
    /// [`Client::reconnect`].
    pub fn events(&self) -> Receiver<Event> {
        self.events.clone()
    }

    /// Send the request `kind` with `fields`, and wait for its reply.
    pub fn request(&self, kind: &str, fields: Fields) -> Result<Reply, Error> {
        self.connection.request(&self.next_id, kind, fields)
    }

    /// Send the request `kind` with `fields` and no `id`, and do not wait.
    /// The host sends no `ok` for it; an `error` comes as [`Event::Error`].
    /// For `session_audio`, which goes many times each second.
    pub fn send(&self, kind: &str, fields: Fields) -> Result<(), Error> {
        self.connection.send(kind, fields)
    }

    /// Connect again after [`Event::HostGone`], and send `hello` again.
    /// Jobs and live sessions do not survive a restart of Sayso.
    pub fn reconnect(&mut self) -> Result<&HelloReply, Error> {
        self.connection.close();
        self.connection = Connection::open(&self.path, self.events_out.clone())?;
        self.hello = send_hello(&self.connection, &self.next_id, &self.app)?;
        Ok(&self.hello)
    }

    /// Send `bye` and close the connection. No [`Event::HostGone`] follows.
    pub fn close(self) -> Result<(), Error> {
        self.request("bye", Fields::new()).map(|_| ())
    }
}

fn send_hello(connection: &Connection, next_id: &AtomicU64, app: &AppHello) -> Result<HelloReply, Error> {
    let hello: HelloReply = connection.request(next_id, "hello", app.fields())?.parse()?;
    if hello.protocol != PROTOCOL {
        return Err(Error::Protocol(format!("Sayso speaks protocol {}, and this SDK speaks {PROTOCOL}", hello.protocol)));
    }
    Ok(hello)
}

impl Drop for Client {
    fn drop(&mut self) {
        self.connection.close();
    }
}

type Waiter = Sender<Result<Reply, Error>>;

/// The requests that wait for a reply, and the state of the connection.
#[derive(Default)]
struct Pending {
    waiters: HashMap<String, Waiter>,
    gone: bool,
    /// The app sent `bye`: the end of the connection is no event.
    said_bye: bool,
}

struct Connection {
    wire: Wire,
    pending: Arc<Mutex<Pending>>,
}

impl Connection {
    fn open(path: &Path, events: Sender<Event>) -> Result<Connection, Error> {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let reader_pending = pending.clone();
        let wire = Wire::open(path, move |incoming| on_incoming(incoming, &reader_pending, &events))?;
        Ok(Connection { wire, pending })
    }

    fn request(&self, next_id: &AtomicU64, kind: &str, mut fields: Fields) -> Result<Reply, Error> {
        let id = format!("r{}", next_id.fetch_add(1, Ordering::SeqCst));
        let id = id.as_str();
        let (waiter, reply) = bounded(1);
        {
            let mut pending = self.pending.lock();
            if pending.gone {
                return Err(Error::Gone);
            }
            pending.waiters.insert(id.to_string(), waiter);
            if kind == "bye" {
                pending.said_bye = true;
            }
        }
        fields.insert("id".into(), id.into());
        if let Err(e) = self.wire.write(kind, fields) {
            self.pending.lock().waiters.remove(id);
            // The host closed the connection before the reader saw it. A
            // Windows pipe fails the write; a Unix socket takes it.
            return Err(if crate::transport::closed(&e) { Error::Gone } else { Error::Io(e) });
        }
        reply.recv().unwrap_or(Err(Error::Gone))
    }

    fn send(&self, kind: &str, fields: Fields) -> Result<(), Error> {
        if self.pending.lock().gone {
            return Err(Error::Gone);
        }
        self.wire.write(kind, fields).map_err(Error::Io)
    }

    fn close(&self) {
        self.wire.close();
    }
}

fn on_incoming(incoming: Incoming, pending: &Mutex<Pending>, events: &Sender<Event>) {
    match incoming {
        Incoming::Reply { id, reply } => {
            let waiter = id.and_then(|id| pending.lock().waiters.remove(&id));
            match (waiter, reply) {
                (Some(waiter), reply) => {
                    let _ = waiter.send(match reply {
                        WireReply::Ok(fields) => Ok(Reply { fields }),
                        WireReply::Error(error) => Err(Error::Host(error)),
                    });
                }
                // An error without the id of a request goes to the events.
                (None, WireReply::Error(error)) => {
                    let _ = events.send(Event::Error(error));
                }
                (None, WireReply::Ok(_)) => {}
            }
        }
        Incoming::Event { id, event } => {
            let _ = events.send(Event::Host { id, event });
        }
        // The end: each waiting request fails, and the app hears it once.
        Incoming::Ended { .. } => {
            let said_bye = {
                let mut pending = pending.lock();
                pending.gone = true;
                pending.waiters.clear();
                pending.said_bye
            };
            if !said_bye {
                let _ = events.send(Event::HostGone);
            }
        }
    }
}
