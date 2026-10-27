//! NDJSON protocol v1: message building, parsing, and audio encoding.
//!
//! One JSON object per line. Every message has `"v":1`. Requests carry an `"id"`
//! when they expect a reply. The spec is `docs/engine-protocol.md`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sayso_core::models::ModelId;
use sayso_core::stt::{Diarization, EngineReason, ModelStatus, Segment, SpeechPart, Turn, Word};
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

/// The `reason` code of a message. `None` when an older engine sends none, and
/// also for a code that this version does not know: the app then shows its
/// general sentence with the English `message`.
pub(crate) fn reason(message: &Message) -> Option<EngineReason> {
    message.str("reason").and_then(EngineReason::from_code)
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
            reason: reason(message),
        },
        _ => return None,
    };
    Some((model, status))
}

/// The `features` of a `hello` reply. Empty for an engine that sends none.
pub(crate) fn features(message: &Message) -> Vec<String> {
    message.body["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|feature| feature.as_str().map(str::to_string))
        .collect()
}

fn number(item: &Value, key: &str, what: &str) -> Result<u64, String> {
    item.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{what} has no integer {key}"))
}

fn text(item: &Value, what: &str) -> Result<String, String> {
    item.get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{what} has no text"))
}

/// The list in the field `key` of a reply, or an error when it is not there.
fn list<'a>(message: &'a Message, key: &str) -> Result<&'a [Value], String> {
    message.body[key]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("the reply has no list {key}"))
}

/// The `segments` of a `final` reply. A segment without `words` has none.
pub(crate) fn segments(message: &Message) -> Result<Vec<Segment>, String> {
    list(message, "segments")?
        .iter()
        .map(|item| {
            let words = item["words"].as_array().map_or(Ok(Vec::new()), |words| {
                words
                    .iter()
                    .map(|word| {
                        Ok(Word {
                            start_ms: number(word, "start_ms", "a word")?,
                            end_ms: number(word, "end_ms", "a word")?,
                            text: text(word, "a word")?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()
            })?;
            Ok(Segment {
                start_ms: number(item, "start_ms", "a segment")?,
                end_ms: number(item, "end_ms", "a segment")?,
                text: text(item, "a segment")?,
                words,
            })
        })
        .collect()
}

/// The `speech` parts of a `vad` reply.
pub(crate) fn speech(message: &Message) -> Result<Vec<SpeechPart>, String> {
    list(message, "speech")?
        .iter()
        .map(|part| {
            Ok(SpeechPart {
                start_ms: number(part, "start_ms", "a speech part")?,
                end_ms: number(part, "end_ms", "a speech part")?,
            })
        })
        .collect()
}

/// The `speakers` and `turns` of a `diarize` reply.
pub(crate) fn diarization(message: &Message) -> Result<Diarization, String> {
    let turns = list(message, "turns")?
        .iter()
        .map(|turn| {
            Ok(Turn {
                start_ms: number(turn, "start_ms", "a turn")?,
                end_ms: number(turn, "end_ms", "a turn")?,
                speaker: u32::try_from(number(turn, "speaker", "a turn")?)
                    .map_err(|_| "a turn has a speaker number that is too large".to_string())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let speakers = number(&message.body, "speakers", "the reply")?;
    Ok(Diarization {
        speakers: u32::try_from(speakers).map_err(|_| "the reply has too many speakers".to_string())?,
        turns,
    })
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
                    message: "boom".into(),
                    reason: None
                }
            ))
        );
        assert_eq!(
            parse(
                r#"{"v":1,"type":"model_state","model":"a","state":"failed","message":"boom","reason":"incomplete_download"}"#
            ),
            Some((
                ModelId::new("a"),
                ModelStatus::Failed {
                    message: "boom".into(),
                    reason: Some(EngineReason::IncompleteDownload)
                }
            ))
        );
        // A code from a newer engine is not an error: the app shows its general sentence.
        assert_eq!(
            parse(
                r#"{"v":1,"type":"model_state","model":"a","state":"failed","message":"boom","reason":"from_the_future"}"#
            ),
            Some((
                ModelId::new("a"),
                ModelStatus::Failed {
                    message: "boom".into(),
                    reason: None
                }
            ))
        );
        // `reason` means nothing in a state other than `failed`.
        assert_eq!(
            parse(r#"{"v":1,"type":"model_state","model":"a","state":"ready","reason":"load_failed"}"#),
            Some((ModelId::new("a"), ModelStatus::Ready))
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

    fn parse(line: &str) -> Message {
        Message::parse(line).unwrap()
    }

    #[test]
    fn features_come_from_hello_and_default_to_none() {
        let hello = parse(r#"{"v":1,"type":"hello","features":["vad","segments",7]}"#);
        assert_eq!(features(&hello), ["vad", "segments"]);
        assert!(features(&parse(r#"{"v":1,"type":"hello"}"#)).is_empty());
    }

    #[test]
    fn segments_keep_their_words_and_times() {
        let reply = parse(
            r#"{"v":1,"type":"final","text":"Hi there. Bye","elapsed_ms":9,"segments":[
                {"start_ms":10,"end_ms":900,"text":"Hi there.","words":[
                    {"start_ms":10,"end_ms":400,"text":"Hi"},{"start_ms":450,"end_ms":900,"text":"there."}]},
                {"start_ms":1500,"end_ms":2000,"text":"Bye"}]}"#,
        );
        let parsed = segments(&reply).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].words.len(), 2);
        assert_eq!(parsed[0].words[1], Word { start_ms: 450, end_ms: 900, text: "there.".into() });
        assert_eq!((parsed[1].start_ms, parsed[1].end_ms, parsed[1].text.as_str()), (1500, 2000, "Bye"));
        assert!(parsed[1].words.is_empty(), "a model with segment times has no words");
        assert_eq!(segments(&parse(r#"{"v":1,"type":"final","segments":[]}"#)).unwrap(), []);
    }

    #[test]
    fn a_reply_without_segments_or_with_a_broken_segment_is_an_error() {
        assert!(segments(&parse(r#"{"v":1,"type":"final","text":"x"}"#)).unwrap_err().contains("segments"));
        let broken = parse(r#"{"v":1,"type":"final","segments":[{"start_ms":1,"text":"x"}]}"#);
        assert!(segments(&broken).unwrap_err().contains("end_ms"));
    }

    #[test]
    fn speech_parts_and_turns_are_read() {
        let vad = parse(r#"{"v":1,"type":"ok","speech":[{"start_ms":100,"end_ms":900},{"start_ms":2000,"end_ms":2500}]}"#);
        assert_eq!(
            speech(&vad).unwrap(),
            [SpeechPart { start_ms: 100, end_ms: 900 }, SpeechPart { start_ms: 2000, end_ms: 2500 }]
        );
        assert_eq!(speech(&parse(r#"{"v":1,"type":"ok","speech":[]}"#)).unwrap(), []);
        assert!(speech(&parse(r#"{"v":1,"type":"ok"}"#)).is_err());

        let diarize = parse(
            r#"{"v":1,"type":"ok","speakers":2,"turns":[
                {"start_ms":0,"end_ms":900,"speaker":0},{"start_ms":800,"end_ms":1500,"speaker":1}]}"#,
        );
        let parsed = diarization(&diarize).unwrap();
        assert_eq!(parsed.speakers, 2);
        assert_eq!(parsed.turns[1], Turn { start_ms: 800, end_ms: 1500, speaker: 1 });
        assert_eq!(
            diarization(&parse(r#"{"v":1,"type":"ok","speakers":0,"turns":[]}"#)).unwrap(),
            Diarization { speakers: 0, turns: Vec::new() }
        );
        assert!(diarization(&parse(r#"{"v":1,"type":"ok","turns":[]}"#)).is_err());
    }

    #[test]
    fn pcm_round_trips_through_base64() {
        let encoded = pcm_base64(&[1, -2]);
        assert_eq!(STANDARD.decode(encoded).unwrap(), vec![1, 0, 0xFE, 0xFF]);
    }
}
