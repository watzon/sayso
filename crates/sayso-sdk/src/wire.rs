//! One connection to the host: the socket, the writer, and a thread that reads
//! each message in order. [`crate::client::Client`] and [`crate::Speech`] both use it.

use crate::transport;
use parking_lot::Mutex;
use sayso_protocol::{Decoded, Fields, HostMessage, Message, PROTOCOL, Reply};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;

/// What the reader thread gets, in the order of the socket.
pub(crate) enum Incoming {
    Reply { id: Option<String>, reply: Reply },
    Event { id: Option<String>, event: sayso_protocol::Event },
    /// The last one. `host_closing` says that Sayso said so first. Else the connection broke.
    Ended { host_closing: bool },
}

pub(crate) struct Wire {
    writer: Mutex<Box<dyn Write + Send>>,
    closer: Arc<dyn Fn() + Send + Sync>,
}

impl Wire {
    /// Connect, and call `on` for each message on a thread of its own.
    pub fn open(path: &Path, on: impl FnMut(Incoming) + Send + 'static) -> io::Result<Wire> {
        let stream = transport::connect(path)?;
        let reader = stream.reader;
        std::thread::Builder::new().name("sayso-sdk-read".into()).spawn(move || read_loop(reader, on))?;
        Ok(Wire { writer: Mutex::new(stream.writer), closer: stream.closer })
    }

    pub fn write(&self, kind: &str, mut fields: Fields) -> io::Result<()> {
        fields.insert("v".into(), PROTOCOL.into());
        fields.insert("type".into(), kind.into());
        let mut line = Value::Object(fields).to_string();
        line.push('\n');
        let mut writer = self.writer.lock();
        writer.write_all(line.as_bytes()).and_then(|()| writer.flush())
    }

    /// Stop both directions. The reader ends with [`Incoming::Ended`].
    pub fn close(&self) {
        (self.closer)();
    }
}

fn read_loop(reader: Box<dyn io::Read + Send>, mut on: impl FnMut(Incoming)) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        match sayso_protocol::decode::<HostMessage>(line.trim_end()) {
            Ok(Decoded::Known(Message { id, body: HostMessage::Reply(reply) })) => on(Incoming::Reply { id, reply }),
            Ok(Decoded::Known(Message { body: HostMessage::Event(sayso_protocol::Event::HostClosing), .. })) => {
                on(Incoming::Ended { host_closing: true });
                return;
            }
            Ok(Decoded::Known(Message { id, body: HostMessage::Event(event) })) => on(Incoming::Event { id, event }),
            // A client ignores a message type that it does not know.
            Ok(Decoded::Unknown { .. }) | Err(_) => {}
        }
    }
    on(Incoming::Ended { host_closing: false });
}
