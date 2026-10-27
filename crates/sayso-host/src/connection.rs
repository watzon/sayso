//! One connection: a thread that reads requests, and a thread that writes
//! replies and events.

use crate::identity::find_record;
use crate::transport::Conn;
use crate::{Peer, Shared};
use crossbeam_channel::{Receiver, Sender, unbounded};
use sayso_protocol::{Capability, DecodeError, Decoded, ErrorCode, ErrorReply, Event, HelloReply, Kind, PROTOCOL, Reply, Request, decode, encode};
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

/// The optional parts of the Host API that this host serves.
const FEATURES: [&str; 0] = [];

/// A longer line is not a request. Audio comes in small parts.
const MAX_LINE: u64 = 16 * 1024 * 1024;

/// A connection in the registry of the host.
pub(crate) struct Entry {
    /// None until the host knows the extension, and for a client that is not
    /// an extension.
    pub extension_id: Option<String>,
    outgoing: Sender<Outgoing>,
    writer: Option<JoinHandle<()>>,
}

impl Entry {
    /// Send `host_closing`, then close. Gives the writer thread, to wait for it.
    pub fn close_with_host_closing(&mut self) -> Option<JoinHandle<()>> {
        let _ = self.outgoing.send(Outgoing::Line(encode(None, &Event::HostClosing)));
        let _ = self.outgoing.send(Outgoing::Close);
        self.writer.take()
    }
}

enum Outgoing {
    /// One message, without the `\n`.
    Line(String),
    /// Stop writing and close both directions.
    Close,
}

pub(crate) fn serve(shared: &Arc<Shared>, number: u64, conn: Conn) {
    let Conn { reader, writer, closer, peer } = conn;
    let (outgoing, messages) = unbounded();
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
            let mut entry = Entry { extension_id: None, outgoing, writer: Some(writer) };
            if let Some(writer) = entry.close_with_host_closing() {
                let _ = writer.join();
            }
            return;
        }
        connections.insert(number, Entry { extension_id: None, outgoing: outgoing.clone(), writer: Some(writer) });
    }

    let extension = identify(shared, &peer);
    drop(peer);
    if let Some(id) = &extension {
        log::info!("host: {id} connected");
        if let Some(entry) = shared.connections.lock().get_mut(&number) {
            entry.extension_id = Some(id.clone());
        }
    }
    let connection = Connection { shared, extension };
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE + 1).read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                log::debug!("host: a connection failed: {e}");
                break;
            }
        }
        if line.len() as u64 > MAX_LINE {
            let _ = outgoing.send(Outgoing::Line(encode(None, &Reply::error(Some(ErrorCode::BadRequest), "the line is longer than 16 MiB"))));
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        if text.trim().is_empty() {
            continue;
        }
        let (reply, close) = connection.handle(text);
        let _ = outgoing.send(Outgoing::Line(reply));
        if close {
            break;
        }
    }

    let _ = outgoing.send(Outgoing::Close);
    let entry = shared.connections.lock().remove(&number);
    // Wait until the last reply is out. `Host::close` waits for the writers
    // that it took.
    if let Some(writer) = entry.and_then(|mut entry| entry.writer.take()) {
        let _ = writer.join();
    }
    if let Some(id) = &connection.extension {
        log::info!("host: {id} disconnected");
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

/// The id of the extension at the other end, from its executable and
/// `state.toml`.
fn identify(shared: &Shared, peer: &Peer) -> Option<String> {
    let executable = match shared.identify.executable(peer) {
        Ok(executable) => executable,
        Err(e) => {
            log::warn!("host: cannot find the program of a client: {e}");
            return None;
        }
    };
    let state = shared.state.current();
    match find_record(&state, &executable, |team_id, bundle_id| shared.identify.signed_by(peer, team_id, bundle_id)) {
        Some(record) => Some(record.id.clone()),
        None => {
            log::warn!("host: {} is not an installed extension", executable.display());
            None
        }
    }
}

struct Connection<'a> {
    shared: &'a Shared,
    extension: Option<String>,
}

impl Connection<'_> {
    /// The reply to one line, and true when the connection closes after it.
    fn handle(&self, line: &str) -> (String, bool) {
        let decoded = decode::<Request>(line);
        let Some(extension_id) = &self.extension else {
            let id = match &decoded {
                Ok(Decoded::Known(message)) => message.id.clone(),
                Ok(Decoded::Unknown { id, .. }) | Err(DecodeError::BadFields { id, .. }) => id.clone(),
                Err(DecodeError::NotAMessage(_)) => None,
            };
            return (not_an_extension(id.as_deref()), true);
        };
        let message = match decoded {
            Err(DecodeError::NotAMessage(message)) => return (encode(None, &Reply::error(Some(ErrorCode::BadRequest), message)), false),
            Err(DecodeError::BadFields { id, kind, message }) => {
                return (encode(id.as_deref(), &Reply::error(Some(ErrorCode::BadRequest), format!("bad {kind} request: {message}"))), false);
            }
            Ok(Decoded::Unknown { id, kind, .. }) => return (encode(id.as_deref(), &Reply::error(None, format!("unknown request type {kind}"))), false),
            Ok(Decoded::Known(message)) => message,
        };
        let id = message.id.as_deref();
        if matches!(message.body, Request::Bye) {
            return (encode(id, &Reply::ok()), true);
        }
        // Read on each request, so a grant that the user takes back stops the next one.
        let state = self.shared.state.current();
        let Some(record) = state.record(extension_id) else {
            log::info!("host: {extension_id} is no longer installed");
            return (not_an_extension(id), true);
        };
        let granted = record.capabilities();
        let reply = match &message.body {
            Request::Hello(_) => Reply::ok_with(&HelloReply {
                host_version: self.shared.host_version.clone(),
                protocol: PROTOCOL,
                features: FEATURES.iter().map(|feature| feature.to_string()).collect(),
                extension_id: extension_id.clone(),
                capabilities: granted,
            }),
            request => match request.capabilities().into_iter().find(|capability| !granted.contains(capability)) {
                Some(missing) => capability_error(&missing),
                None => Reply::error(Some(ErrorCode::Unsupported), format!("this version of Sayso cannot run {} yet", request.kind())),
            },
        };
        (encode(id, &reply), false)
    }
}

fn not_an_extension(id: Option<&str>) -> String {
    encode(id, &Reply::error(Some(ErrorCode::NotAnExtension), "this program is not an installed extension of Sayso"))
}

fn capability_error(missing: &Capability) -> Reply {
    Reply::Error(ErrorReply {
        code: Some(ErrorCode::Capability),
        message: format!("the user did not grant the capability {missing}"),
        capability: Some(missing.clone()),
        reason: None,
    })
}
