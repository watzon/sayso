//! AssemblyAI Sync adapter: `POST {base}/transcribe`, a multipart body with a
//! JSON `config` part and the `audio` part.

use crate::http::{Deadline, Reply, Target, language, text_at, words};
use crate::multipart::Multipart;
use sayso_core::speech::{SpeechError, SpeechRequest};
use serde_json::{Value, json};
use std::time::Duration;

const KEYTERM_LIMIT: usize = 100;

/// The key goes in as it is, without "Bearer".
fn headers(target: &Target) -> Vec<(&'static str, String)> {
    let mut headers = vec![("X-AAI-Model", target.model.to_string())];
    if let Some(key) = target.key {
        headers.push(("Authorization", key.to_string()));
    }
    headers
}

/// `error`, else `detail` (a string), else `error_code`.
fn error_message(json: &Value) -> Option<String> {
    json["error"]
        .as_str()
        .or_else(|| json["detail"].as_str())
        .or_else(|| json["error_code"].as_str())
        .map(str::to_string)
}

/// The `config` part.
fn config(request: &SpeechRequest) -> Value {
    let mut config = json!({});
    if let Some(code) = language(request) {
        config["language_codes"] = json!([code]);
    }
    let words: Vec<&str> = words(request).into_iter().take(KEYTERM_LIMIT).collect();
    if !words.is_empty() {
        config["keyterms_prompt"] = json!(words);
    }
    config
}

/// Send the request and return the answer, whatever its status.
fn send(target: &Target, request: &SpeechRequest, deadline: &Deadline) -> Result<Reply, SpeechError> {
    let mut form = Multipart::new();
    form.part("config", "application/json", config(request).to_string().as_bytes());
    form.file("audio", "audio.wav", "audio/wav", request.wav);
    let content_type = form.content_type();
    let body = form.finish();
    target.post(&target.url("/transcribe"), &headers(target), &content_type, &body, deadline)
}

pub(crate) fn transcribe(target: &Target, request: &SpeechRequest, deadline: &Deadline) -> Result<String, SpeechError> {
    text_at(&send(target, request, deadline)?.into_json(error_message)?, "/text")
}

/// AssemblyAI Sync has no list endpoint, so the check sends half a second of
/// silence. The key works when the answer is 2xx, or when the service says the
/// audio is bad (the key was accepted before it read the audio).
pub(crate) fn check(target: &Target, deadline: &Deadline) -> Result<(), SpeechError> {
    let wav = silent_wav(500);
    let request = SpeechRequest { model: target.model, wav: &wav, language: None, vocabulary: &[], timeout: Duration::ZERO };
    let reply = send(target, &request, deadline)?;
    if reply.is_success() {
        return Ok(());
    }
    let bad_audio = matches!(reply.status, 400 | 422)
        && reply.json().is_some_and(|json| matches!(json["error_code"].as_str(), Some("audio_too_short" | "bad_audio")));
    if bad_audio { Ok(()) } else { Err(reply.error(error_message)) }
}

/// A WAV file of silence: 16 kHz, mono, 16-bit.
fn silent_wav(millis: u32) -> Vec<u8> {
    const RATE: u32 = 16_000;
    let data_len = RATE / 1000 * millis * 2;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    wav.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(44 + data_len as usize, 0);
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_wav_has_a_valid_header_and_half_a_second_of_samples() {
        let wav = silent_wav(500);
        assert_eq!(wav.len(), 44 + 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 16_000);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1, "mono");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 16_000);
        assert!(wav[44..].iter().all(|&b| b == 0));
    }

    #[test]
    fn config_has_only_the_hints_that_are_set() {
        let words = vec!["Sayso".to_string(), " ".to_string()];
        let mut request = SpeechRequest { model: "m", wav: &[], language: None, vocabulary: &[], timeout: Duration::ZERO };
        assert_eq!(config(&request), json!({}));
        request.language = Some("de");
        request.vocabulary = &words;
        assert_eq!(config(&request), json!({"language_codes": ["de"], "keyterms_prompt": ["Sayso"]}));
    }
}
