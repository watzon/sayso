//! NDJSON protocol v1: one JSON object per line. Every message has `"v":1`.
//!
//! The reference is the Swift engine (`native/macos/SaysoEngine`) and the
//! client in `sayso-engine-client`.

use anyhow::{Context, Result, anyhow};
use serde_json::{Map, Value, json};
use std::io::Write;
use std::sync::Mutex;

pub const PROTOCOL_VERSION: u64 = 1;

/// Writes protocol lines. Thread safe. One write call per line, so lines from
/// different threads never mix.
pub struct Output {
    sink: Mutex<Box<dyn Write + Send>>,
}

impl Output {
    pub fn new(sink: Box<dyn Write + Send>) -> Self {
        Output {
            sink: Mutex::new(sink),
        }
    }

    /// The real stdout, kept for the protocol only.
    ///
    /// On Unix, fd 1 then points at stderr, so a stray print from a native
    /// library cannot corrupt the stream. The Swift engine does the same.
    pub fn protocol_stdout() -> Self {
        #[cfg(unix)]
        {
            use std::os::fd::FromRawFd;
            // SAFETY: dup and dup2 on the standard descriptors. The duplicate is a new
            // descriptor that only this File owns.
            unsafe {
                let fd = libc::dup(libc::STDOUT_FILENO);
                if fd >= 0 && libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) >= 0 {
                    return Output::new(Box::new(std::fs::File::from_raw_fd(fd)));
                }
            }
        }
        Output::new(Box::new(std::io::stdout()))
    }

    /// Send one message. `fields` must be a JSON object (or null for no fields).
    pub fn send(&self, kind: &str, id: Option<&str>, fields: Value) {
        let mut object = match fields {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        object.insert("v".into(), json!(PROTOCOL_VERSION));
        object.insert("type".into(), json!(kind));
        if let Some(id) = id {
            object.insert("id".into(), json!(id));
        }
        let mut line = Value::Object(object).to_string();
        line.push('\n');
        let mut sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        // A write error means the parent closed the pipe. The stdin loop then ends the process.
        let _ = sink.write_all(line.as_bytes()).and_then(|()| sink.flush());
    }

    pub fn ok(&self, id: Option<&str>, fields: Value) {
        self.send("ok", id, fields);
    }

    pub fn error(&self, message: &str, id: Option<&str>) {
        self.send("error", id, json!({ "message": message }));
    }

    pub fn log(&self, level: &str, message: &str) {
        self.send("log", None, json!({ "level": level, "message": message }));
    }
}

/// One parsed request line.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: Option<String>,
    pub kind: String,
    fields: Map<String, Value>,
}

impl Request {
    pub fn parse(line: &str) -> Result<Request> {
        let value: Value = serde_json::from_str(line).map_err(|_| anyhow!("invalid JSON line"))?;
        let Value::Object(fields) = value else {
            return Err(anyhow!("invalid JSON line"));
        };
        let kind = fields
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing type"))?
            .to_string();
        let id = fields.get("id").and_then(Value::as_str).map(str::to_string);
        Ok(Request { id, kind, fields })
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn string(&self, key: &str) -> Result<&str> {
        self.optional_string(key)
            .ok_or_else(|| anyhow!("missing string field {key}"))
    }

    pub fn optional_string(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    pub fn u64(&self, key: &str) -> Result<u64> {
        self.fields
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("missing integer field {key}"))
    }

    pub fn optional_u64(&self, key: &str) -> Option<u64> {
        self.fields.get(key).and_then(Value::as_u64)
    }

    /// A JSON value by key, for the `engine` object and the `models` array.
    pub fn value(&self, key: &str) -> Result<&Value> {
        self.fields
            .get(key)
            .with_context(|| format!("missing field {key}"))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;

    /// A sink that keeps everything written to it.
    #[derive(Clone, Default)]
    pub struct Captured(pub Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        /// Every message written so far, parsed.
        pub fn messages(&self) -> Vec<Value> {
            let bytes = self.0.lock().unwrap().clone();
            String::from_utf8(bytes)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }

        pub fn clear(&self) {
            self.0.lock().unwrap().clear();
        }
    }

    #[test]
    fn messages_have_version_type_and_id() {
        let captured = Captured::default();
        let out = Output::new(Box::new(captured.clone()));
        out.ok(Some("r1"), json!({"session": 3}));
        out.error("boom", None);
        out.log("info", "hi");
        let messages = captured.messages();
        assert_eq!(
            messages[0],
            json!({"v": 1, "type": "ok", "id": "r1", "session": 3})
        );
        assert_eq!(
            messages[1],
            json!({"v": 1, "type": "error", "message": "boom"})
        );
        assert_eq!(
            messages[2],
            json!({"v": 1, "type": "log", "level": "info", "message": "hi"})
        );
    }

    #[test]
    fn request_parsing() {
        let request =
            Request::parse(r#"{"v":1,"type":"load","id":"r2","model":"m","session":7}"#).unwrap();
        assert_eq!(request.kind, "load");
        assert_eq!(request.id(), Some("r2"));
        assert_eq!(request.string("model").unwrap(), "m");
        assert_eq!(request.u64("session").unwrap(), 7);
        assert!(request.string("path").is_err());
        assert_eq!(request.optional_u64("size_bytes"), None);

        assert_eq!(
            Request::parse("nope").unwrap_err().to_string(),
            "invalid JSON line"
        );
        assert_eq!(
            Request::parse(r#"{"v":1}"#).unwrap_err().to_string(),
            "missing type"
        );
        assert!(Request::parse("[1]").is_err());
    }
}
