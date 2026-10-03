//! NDJSON protocol v1: request parsing and the protocol writer.
//!
//! One JSON object per line. Every message has `"v":1`. See
//! `crates/sayso-engine-client/NOTES.md` for the full reference.

use sayso_core::models::EngineKind;
use serde_json::{Map, Value, json};
use std::io::Write;
use std::sync::{Arc, Mutex};

pub const PROTOCOL_VERSION: u64 = 1;
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A failure that goes back to the client as `error {message}`.
pub type Result<T> = std::result::Result<T, String>;

/// One parsed request line.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: Option<String>,
    pub kind: String,
    body: Map<String, Value>,
}

impl Request {
    pub fn parse(line: &str) -> Result<Request> {
        let value: Value =
            serde_json::from_str(line).map_err(|_| "invalid JSON line".to_string())?;
        let Value::Object(body) = value else {
            return Err("a request must be a JSON object".into());
        };
        let kind = body
            .get("type")
            .and_then(Value::as_str)
            .ok_or("missing type")?
            .to_string();
        let id = body.get("id").and_then(Value::as_str).map(str::to_string);
        Ok(Request { id, kind, body })
    }

    pub fn str(&self, key: &str) -> Result<&str> {
        self.body
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("missing string field {key}"))
    }

    pub fn opt_str(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(Value::as_str)
    }

    pub fn u64(&self, key: &str) -> Result<u64> {
        self.body
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("missing integer field {key}"))
    }

    pub fn opt_u64(&self, key: &str) -> Option<u64> {
        self.body.get(key).and_then(Value::as_u64)
    }

    /// A list of strings. Missing or malformed entries are skipped.
    pub fn strings(&self, key: &str) -> Vec<String> {
        self.body
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `engine` object, as the `EngineKind` of `sayso-core`.
    pub fn engine(&self) -> Result<EngineKind> {
        let value = self
            .body
            .get("engine")
            .ok_or("missing object field engine")?;
        parse_engine(value)
    }

    pub fn opt_engine(&self) -> Option<Result<EngineKind>> {
        self.body.get("engine").map(parse_engine)
    }

    /// The `models` array of `list_models`. Each entry keeps its own engine
    /// result, so one bad entry does not fail the whole list.
    pub fn models(&self) -> Result<Vec<(String, Result<EngineKind>)>> {
        let list = self
            .body
            .get("models")
            .and_then(Value::as_array)
            .ok_or("missing models array")?;
        list.iter()
            .map(|entry| {
                let id = entry
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("models[].id is missing")?;
                let engine = match entry.get("engine") {
                    Some(engine) => parse_engine(engine),
                    None => Err("models[].engine is missing".to_string()),
                };
                Ok((id.to_string(), engine))
            })
            .collect()
    }
}

/// Parse an `EngineKind`. An unknown `kind` gets a short message.
pub fn parse_engine(value: &Value) -> Result<EngineKind> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("engine.kind is missing")?;
    serde_json::from_value::<EngineKind>(value.clone()).map_err(|e| {
        if e.to_string().contains("unknown variant") {
            format!("unknown engine kind {kind}")
        } else {
            format!("bad engine {kind}: {e}")
        }
    })
}

/// The `kind` tag of an engine, for messages.
pub fn engine_kind_name(engine: &EngineKind) -> String {
    serde_json::to_value(engine)
        .ok()
        .and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

/// Writes protocol lines. Cheap to clone, thread safe. Each message is one
/// `write_all` of a full line under a lock, so lines never interleave.
#[derive(Clone)]
pub struct Output {
    sink: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Output {
    pub fn new(sink: Box<dyn Write + Send>) -> Output {
        Output {
            sink: Arc::new(Mutex::new(sink)),
        }
    }

    /// Send one message. `fields` must be a JSON object (anything else is ignored).
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
        // A closed pipe means the parent is gone. The stdin loop ends the process.
        let _ = sink.write_all(line.as_bytes()).and_then(|()| sink.flush());
    }

    pub fn ok(&self, id: Option<&str>, fields: Value) {
        self.send("ok", id, fields);
    }

    pub fn error(&self, id: Option<&str>, message: &str) {
        self.send("error", id, json!({ "message": message }));
    }

    pub fn log(&self, level: &str, message: &str) {
        self.send("log", None, json!({ "level": level, "message": message }));
    }

    pub fn model_state(&self, id: Option<&str>, model: &str, state: &str) {
        self.send("model_state", id, json!({ "model": model, "state": state }));
    }
}

/// An in-memory sink for tests: every line the engine wrote.
#[derive(Clone, Default)]
pub struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    pub fn output(&self) -> Output {
        Output::new(Box::new(self.clone()))
    }

    pub fn messages(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is JSON"))
            .collect()
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_type_id_and_fields() {
        let r = Request::parse(r#"{"v":1,"type":"load","id":"r1","model":"m","size_bytes":5}"#)
            .unwrap();
        assert_eq!(r.kind, "load");
        assert_eq!(r.id.as_deref(), Some("r1"));
        assert_eq!(r.str("model").unwrap(), "m");
        assert_eq!(r.opt_u64("size_bytes"), Some(5));
        assert!(r.str("path").unwrap_err().contains("path"));
        assert!(r.u64("session").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Request::parse("not json").unwrap_err(), "invalid JSON line");
        assert!(Request::parse("[1,2]").is_err());
        assert_eq!(Request::parse(r#"{"v":1}"#).unwrap_err(), "missing type");
    }

    #[test]
    fn engine_kinds_use_the_sayso_core_type() {
        let r = Request::parse(
            r#"{"type":"download","engine":{"kind":"onnx","family":"parakeet","repo":"istupakov/parakeet-tdt-0.6b-v3-onnx"}}"#,
        )
        .unwrap();
        assert_eq!(
            r.engine().unwrap(),
            EngineKind::Onnx {
                family: "parakeet".into(),
                repo: "istupakov/parakeet-tdt-0.6b-v3-onnx".into()
            }
        );
        let r = Request::parse(
            r#"{"type":"x","engine":{"kind":"whisper_cpp","file":"ggml-tiny.bin"}}"#,
        )
        .unwrap();
        assert_eq!(
            r.engine().unwrap(),
            EngineKind::WhisperCpp {
                file: "ggml-tiny.bin".into()
            }
        );
        let r = Request::parse(r#"{"type":"x","engine":{"kind":"warp_drive"}}"#).unwrap();
        assert_eq!(r.engine().unwrap_err(), "unknown engine kind warp_drive");
        let r = Request::parse(r#"{"type":"x","engine":{"kind":"whisper_cpp"}}"#).unwrap();
        assert!(
            r.engine()
                .unwrap_err()
                .starts_with("bad engine whisper_cpp")
        );
        let r = Request::parse(r#"{"type":"x","engine":{}}"#).unwrap();
        assert_eq!(r.engine().unwrap_err(), "engine.kind is missing");
    }

    #[test]
    fn models_keep_bad_entries_apart() {
        let r = Request::parse(
            r#"{"type":"list_models","models":[{"id":"a","engine":{"kind":"whisper_cpp","file":"f.bin"}},{"id":"b","engine":{"kind":"nope"}}]}"#,
        )
        .unwrap();
        let models = r.models().unwrap();
        assert_eq!(models.len(), 2);
        assert!(models[0].1.is_ok());
        assert!(models[1].1.is_err());
        let r = Request::parse(r#"{"type":"list_models","models":[{"engine":{}}]}"#).unwrap();
        assert!(r.models().is_err());
    }

    #[test]
    fn output_writes_one_versioned_line_per_message() {
        let captured = Captured::default();
        let out = captured.output();
        out.send("hello", Some("r1"), json!({"version": "x", "protocol": 1}));
        out.error(None, "boom");
        out.log("info", "hi");
        let messages = captured.messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["v"], 1);
        assert_eq!(messages[0]["type"], "hello");
        assert_eq!(messages[0]["id"], "r1");
        assert_eq!(messages[1]["type"], "error");
        assert!(messages[1].get("id").is_none());
        assert_eq!(messages[2]["level"], "info");
    }

    #[test]
    fn kind_names() {
        assert_eq!(engine_kind_name(&EngineKind::AppleSpeech), "apple_speech");
        assert_eq!(
            engine_kind_name(&EngineKind::WhisperCpp { file: "a".into() }),
            "whisper_cpp"
        );
    }
}
