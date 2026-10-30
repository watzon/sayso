//! NDJSON protocol v1: one JSON object per line. Every message has `"v":1`.
//!
//! The spec is `docs/engine-protocol.md`. The other engine is the Swift engine
//! (`native/macos/SaysoEngine`), and the client is `sayso-engine-client`.

use anyhow::{Context, Result, anyhow};
use serde_json::{Map, Value, json};
use std::io::Write;
use std::sync::Mutex;

pub const PROTOCOL_VERSION: u64 = 1;

/// The optional parts of the protocol that this engine has, sent in `hello`
/// (`docs/engine-protocol.md`, "Features"). It accepts `prepare` but does
/// nothing with it, so it does not list `prepare`.
pub const FEATURES: &[&str] = &["catalog", "concurrent_streams", "detected_language", "diarization", "keep_preview", "segments", "store_v1", "unload", "vad"];

/// An error with a stable `code` for the `error` reply, for example
/// `too_many_streams`.
#[derive(Debug)]
pub struct Coded {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for Coded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Coded {}

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
    /// Stdout then points at stderr, so a stray print from a native library
    /// cannot corrupt the stream. The Swift engine does the same.
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
        #[cfg(windows)]
        if let Some(file) = windows_protocol_stdout() {
            return Output::new(Box::new(file));
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

    /// An `error` reply for `error`: with its `code` when it is a [`Coded`] error.
    pub fn failure(&self, error: &anyhow::Error, id: Option<&str>) {
        let mut fields = json!({ "message": format!("{error:#}") });
        if let Some(coded) = error.downcast_ref::<Coded>() {
            fields["code"] = json!(coded.code);
        }
        self.send("error", id, fields);
    }

    pub fn log(&self, level: &str, message: &str) {
        self.send("log", None, json!({ "level": level, "message": message }));
    }
}

/// Windows has two layers: the C runtime's descriptor 1 (`printf`) and the
/// Win32 standard output handle (Rust's `println!`, `WriteFile`). Both go to
/// stderr. whisper.cpp links the same C runtime DLL as Rust, so it shares
/// descriptor 1. sherpa-onnx and ONNX Runtime have their own C runtime; they
/// are delay-loaded (`build.rs`), so it starts after this and takes stderr.
#[cfg(windows)]
fn windows_protocol_stdout() -> Option<std::fs::File> {
    use std::os::raw::{c_int, c_void};
    use std::os::windows::io::FromRawHandle;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    unsafe extern "C" {
        fn _dup(fd: c_int) -> c_int;
        fn _dup2(src: c_int, dst: c_int) -> c_int;
        fn _get_osfhandle(fd: c_int) -> isize;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
    }
    // SAFETY: C runtime and Win32 calls on the standard handles. `_dup` makes a
    // new descriptor with its own duplicated OS handle. The returned File owns
    // that handle; the descriptor is never used or closed again.
    unsafe {
        let protocol = _dup(1);
        if protocol < 0 {
            return None;
        }
        let handle = _get_osfhandle(protocol);
        if handle == -1 || handle == 0 {
            return None;
        }
        let stderr = GetStdHandle(STD_ERROR_HANDLE);
        if !stderr.is_null() && stderr as isize != -1 {
            _dup2(2, 1);
            SetStdHandle(STD_OUTPUT_HANDLE, stderr);
        }
        Some(std::fs::File::from_raw_handle(handle as *mut c_void))
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

    /// A JSON value by key that can be missing. `null` counts as missing.
    pub fn optional_value(&self, key: &str) -> Option<&Value> {
        self.fields.get(key).filter(|v| !v.is_null())
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
