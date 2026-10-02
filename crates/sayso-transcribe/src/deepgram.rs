//! Deepgram adapter: `POST {base}/listen` with the raw WAV as the body.
//! The options go in the query string.

use crate::http::{Deadline, Target, language, text_at, words};
use sayso_core::speech::{SpeechError, SpeechRequest};
use serde_json::Value;

/// The key terms share a budget, because they go in the URL.
const KEYTERM_BUDGET_CHARS: usize = 400;

fn auth(target: &Target) -> Vec<(&'static str, String)> {
    target.key.map(|key| ("Authorization", format!("Token {key}"))).into_iter().collect()
}

/// `err_msg`, else `message`.
fn error_message(json: &Value) -> Option<String> {
    json["err_msg"].as_str().or_else(|| json["message"].as_str()).map(str::to_string)
}

/// The URL for one request.
fn listen_url(target: &Target, request: &SpeechRequest) -> String {
    let mut url = format!("{}?model={}&smart_format=true&punctuate=true", target.url("/listen"), percent_encode(request.model));
    match language(request) {
        Some(code) => url.push_str(&format!("&language={}", percent_encode(code))),
        None => url.push_str("&detect_language=true"),
    }
    // Only Nova-3 takes key terms.
    if request.model.starts_with("nova-3") {
        let mut used = 0;
        for word in words(request) {
            used += word.chars().count();
            if used > KEYTERM_BUDGET_CHARS {
                break;
            }
            url.push_str(&format!("&keyterm={}", percent_encode(word)));
        }
    }
    url
}

pub(crate) fn transcribe(target: &Target, request: &SpeechRequest, deadline: &Deadline) -> Result<String, SpeechError> {
    let reply = target.post(&listen_url(target, request), &auth(target), "audio/wav", request.wav, deadline)?;
    text_at(&reply.into_json(error_message)?, "/results/channels/0/alternatives/0/transcript")
}

/// `GET {base}/projects`.
pub(crate) fn check(target: &Target, deadline: &Deadline) -> Result<(), SpeechError> {
    let reply = target.get(&target.url("/projects"), &auth(target), deadline)?;
    if reply.is_success() { Ok(()) } else { Err(reply.error(error_message)) }
}

/// Percent-encode a query value. Letters, digits, `-`, `.`, `_`, and `~` stay as they are.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_reserved_and_non_ascii_bytes() {
        assert_eq!(percent_encode("nova-3.general_~"), "nova-3.general_~");
        assert_eq!(percent_encode("New York"), "New%20York");
        assert_eq!(percent_encode("a&b=c#d+e/f?g"), "a%26b%3Dc%23d%2Be%2Ff%3Fg");
        assert_eq!(percent_encode("Zoë"), "Zo%C3%AB");
        assert_eq!(percent_encode(""), "");
    }
}
