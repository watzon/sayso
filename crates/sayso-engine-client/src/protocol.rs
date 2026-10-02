//! NDJSON protocol v1: message building, parsing, and audio encoding.
//!
//! One JSON object per line. Every message has `"v":1`. Requests carry an `"id"`
//! when they expect a reply. See `NOTES.md` for the full reference.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sayso_core::models::ModelId;
use sayso_core::stt::ModelStatus;
use serde_json::{Map, Value, json};

pub(crate) const PROTOCOL_VERSION: u64 = 1;

/// Build one request line without the trailing newline.
pub(crate) fn request_line(kind: &str, id: Option<&str>, fields: Value) -> String {
    let mut object = match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    object.insert("v".into(), json!(PROTOCOL_VERSION));
    object.insert("type".into(), json!(kind));
    if let Some(id) = id {
        object.insert("id".into(), json!(id));
    }
    Value::Object(object).to_string()
}

/// One message from the engine.
#[derive(Debug, Clone)]
pub(crate) struct Message {
    pub kind: String,
    pub id: Option<String>,
    pub body: Value,
}

impl Message {
    pub(crate) fn parse(line: &str) -> Result<Message, String> {
        let body: Value = serde_json::from_str(line).map_err(|e| format!("bad JSON: {e}"))?;
        let kind = body
            .get("type")
            .and_then(Value::as_str)
            .ok_or("message has no type")?
            .to_string();
        let id = body.get("id").and_then(Value::as_str).map(str::to_string);
        Ok(Message { kind, id, body })
    }

    pub(crate) fn str(&self, key: &str) -> Option<&str> {
        self.body.get(key).and_then(Value::as_str)
    }

    pub(crate) fn u64(&self, key: &str) -> Option<u64> {
        self.body.get(key).and_then(Value::as_u64)
    }

    pub(crate) fn f64(&self, key: &str) -> Option<f64> {
        self.body.get(key).and_then(Value::as_f64)
    }

    /// A reply ends a request. `model_state`, `download_progress`, `partial`, and
    /// `log` are events that can arrive while a request still runs.
    pub(crate) fn is_reply(&self) -> bool {
        matches!(self.kind.as_str(), "hello" | "ok" | "final" | "error")
    }
}

/// The model status that a `model_state` or `download_progress` message describes.
pub(crate) fn model_status(message: &Message) -> Option<(ModelId, ModelStatus)> {
    let model = ModelId::new(message.str("model")?);
    let state = if message.kind == "download_progress" {
        "downloading"
    } else {
        message.str("state")?
    };
    let status = match state {
        "not_downloaded" => ModelStatus::NotDownloaded,
        "downloading" => ModelStatus::Downloading {
            fraction: message.f64("fraction").unwrap_or(0.0) as f32,
            bytes_done: message.u64("bytes_done").unwrap_or(0),
            bytes_total: message.u64("bytes_total").unwrap_or(0),
        },
        "optimizing" => ModelStatus::Optimizing,
        "downloaded" => ModelStatus::Downloaded,
        "ready" => ModelStatus::Ready,
        "failed" => ModelStatus::Failed {
            message: message
                .str("message")
                .unwrap_or("unknown error")
                .to_string(),
        },
        _ => return None,
    };
    Some((model, status))
}

/// Base64 of little-endian 16-bit PCM, the `pcm` field of `stream_audio`.
pub(crate) fn pcm_base64(pcm: &[i16]) -> String {
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line_has_version_type_and_id() {
        let line = request_line("load", Some("r1"), json!({"model": "m"}));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["type"], "load");
        assert_eq!(value["id"], "r1");
        assert_eq!(value["model"], "m");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn parse_rejects_garbage_and_missing_type() {
        assert!(Message::parse("not json").is_err());
        assert!(Message::parse(r#"{"v":1}"#).is_err());
        let message = Message::parse(r#"{"v":1,"type":"ok","id":"r2"}"#).unwrap();
        assert!(message.is_reply());
        assert_eq!(message.id.as_deref(), Some("r2"));
    }

    #[test]
    fn model_state_maps_to_status() {
        let parse = |line| model_status(&Message::parse(line).unwrap());
        assert_eq!(
            parse(r#"{"v":1,"type":"model_state","model":"a","state":"optimizing"}"#),
            Some((ModelId::new("a"), ModelStatus::Optimizing))
        );
        assert_eq!(
            parse(r#"{"v":1,"type":"model_state","model":"a","state":"failed","message":"boom"}"#),
            Some((
                ModelId::new("a"),
                ModelStatus::Failed {
                    message: "boom".into()
                }
            ))
        );
        assert_eq!(
            parse(
                r#"{"v":1,"type":"download_progress","model":"a","fraction":0.25,"bytes_done":5,"bytes_total":20}"#
            ),
            Some((
                ModelId::new("a"),
                ModelStatus::Downloading {
                    fraction: 0.25,
                    bytes_done: 5,
                    bytes_total: 20
                }
            ))
        );
        assert_eq!(
            parse(r#"{"v":1,"type":"model_state","model":"a","state":"nope"}"#),
            None
        );
    }

    #[test]
    fn pcm_round_trips_through_base64() {
        let encoded = pcm_base64(&[1, -2]);
        assert_eq!(STANDARD.decode(encoded).unwrap(), vec![1, 0, 0xFE, 0xFF]);
    }
}
