//! ElevenLabs adapter: `POST {base}/speech-to-text` with a `xi-api-key` header.

use crate::http::{Deadline, Target, language, text_at, words};
use crate::multipart::Multipart;
use sayso_core::speech::{SpeechError, SpeechRequest};
use serde_json::Value;

/// ElevenLabs refuses a key term this long.
const KEYTERM_MAX_CHARS: usize = 50;
const KEYTERM_LIMIT: usize = 100;

fn auth(target: &Target) -> Vec<(&'static str, String)> {
    target.key.map(|key| ("xi-api-key", key.to_string())).into_iter().collect()
}

/// `detail.message`, `detail` as a string, or `detail[0].msg` for a validation error list.
fn error_message(json: &Value) -> Option<String> {
    let detail = &json["detail"];
    let text = match detail {
        Value::Object(_) => detail["message"].as_str(),
        Value::String(s) => Some(s.as_str()),
        Value::Array(_) => detail[0]["msg"].as_str(),
        _ => None,
    };
    text.map(str::to_string)
}

pub(crate) fn transcribe(target: &Target, request: &SpeechRequest, deadline: &Deadline) -> Result<String, SpeechError> {
    let mut form = Multipart::new();
    form.file("file", "audio.wav", "audio/wav", request.wav);
    form.text("model_id", request.model);
    form.text("tag_audio_events", "false");
    form.text("diarize", "false");
    if let Some(code) = language(request) {
        form.text("language_code", code);
    }
    for word in words(request).into_iter().filter(|w| w.chars().count() < KEYTERM_MAX_CHARS).take(KEYTERM_LIMIT) {
        form.text("keyterms", word);
    }
    let content_type = form.content_type();
    let body = form.finish();
    let reply = target.post(&target.url("/speech-to-text"), &auth(target), &content_type, &body, deadline)?;
    text_at(&reply.into_json(error_message)?, "/text")
}

/// `GET {base}/models`. Any 2xx answer is fine.
pub(crate) fn check(target: &Target, deadline: &Deadline) -> Result<(), SpeechError> {
    let reply = target.get(&target.url("/models"), &auth(target), deadline)?;
    if reply.is_success() { Ok(()) } else { Err(reply.error(error_message)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_three_shapes_of_detail() {
        assert_eq!(error_message(&json!({"detail": {"status": "invalid_api_key", "message": "bad key"}})).as_deref(), Some("bad key"));
        assert_eq!(error_message(&json!({"detail": "plain"})).as_deref(), Some("plain"));
        assert_eq!(error_message(&json!({"detail": [{"loc": ["body"], "msg": "field required"}]})).as_deref(), Some("field required"));
        assert_eq!(error_message(&json!({"other": 1})), None);
    }
}
