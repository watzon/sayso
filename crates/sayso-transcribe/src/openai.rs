//! OpenAI-compatible adapter: OpenAI, Groq, Mistral, and any server with
//! `POST {base}/audio/transcriptions`.
//!
//! The language and the dictionary words go in as hints. `transcribe` in
//! `lib.rs` sends the request again without them when a strict server rejects
//! a field it does not know.

use crate::http::{Deadline, Reply, Target, language, text_at, words};
use crate::multipart::Multipart;
use sayso_core::speech::{SpeechError, SpeechProviderKind, SpeechRequest};
use serde_json::Value;

/// The longest `prompt` field, in characters.
const PROMPT_LIMIT: usize = 800;
/// The most `context_bias` words Mistral takes.
const CONTEXT_BIAS_LIMIT: usize = 100;
/// This OpenAI model names its hint fields in another way.
const GPT_TRANSCRIBE: &str = "gpt-transcribe";

fn auth(target: &Target) -> Vec<(&'static str, String)> {
    target.key.map(|key| ("Authorization", format!("Bearer {key}"))).into_iter().collect()
}

/// The provider's error text: `error.message`, `message`, `detail`, or a plain `error` string.
fn error_message(json: &Value) -> Option<String> {
    json.pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| json["message"].as_str())
        .or_else(|| json["detail"].as_str())
        .or_else(|| json["error"].as_str())
        .map(str::to_string)
}

pub(crate) fn transcribe(target: &Target, request: &SpeechRequest, deadline: &Deadline) -> Result<String, SpeechError> {
    let form = form(target.kind, request);
    let content_type = form.content_type();
    let body = form.finish();
    let reply = target.post(&target.url("/audio/transcriptions"), &auth(target), &content_type, &body, deadline)?;
    text_at(&reply.into_json(error_message)?, "/text")
}

fn form(kind: SpeechProviderKind, request: &SpeechRequest) -> Multipart {
    let mut form = Multipart::new();
    form.file("file", "audio.wav", "audio/wav", request.wav);
    form.text("model", request.model);
    form.text("response_format", "json");
    let gpt = kind == SpeechProviderKind::OpenAi && request.model == GPT_TRANSCRIBE;
    if let Some(code) = language(request) {
        form.text(if gpt { "languages[]" } else { "language" }, code);
    }
    let words = words(request);
    if words.is_empty() {
        return form;
    }
    if gpt {
        // One word per line. The characters below could end the field early.
        let lines: Vec<String> = words
            .iter()
            .map(|w| w.chars().filter(|c| !matches!(c, '<' | '>' | '\r' | '\n')).collect::<String>())
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .collect();
        if !lines.is_empty() {
            form.text("keywords", &lines.join("\n"));
        }
    } else if kind == SpeechProviderKind::Mistral {
        for word in words.iter().take(CONTEXT_BIAS_LIMIT) {
            form.text("context_bias", word);
        }
    } else {
        let prompt = truncate_prompt(&words, PROMPT_LIMIT);
        if !prompt.is_empty() {
            form.text("prompt", &prompt);
        }
    }
    form
}

/// Join the words with ", ". Stop before the first word that would push the
/// text over `limit` characters, so no word is cut in two.
fn truncate_prompt(words: &[&str], limit: usize) -> String {
    let mut prompt = String::new();
    let mut length = 0;
    for word in words {
        let added = word.chars().count() + if prompt.is_empty() { 0 } else { 2 };
        if length + added > limit {
            break;
        }
        if !prompt.is_empty() {
            prompt.push_str(", ");
        }
        prompt.push_str(word);
        length += added;
    }
    prompt
}

/// `GET {base}/models`.
fn models(target: &Target, deadline: &Deadline) -> Result<Reply, SpeechError> {
    let reply = target.get(&target.url("/models"), &auth(target), deadline)?;
    if reply.is_success() { Ok(reply) } else { Err(reply.error(error_message)) }
}

pub(crate) fn check(target: &Target, deadline: &Deadline) -> Result<(), SpeechError> {
    models(target, deadline).map(|_| ())
}

/// The ids in `data[].id`, sorted.
pub(crate) fn list_models(target: &Target, deadline: &Deadline) -> Result<Vec<String>, SpeechError> {
    let json = models(target, deadline)?.into_json(error_message)?;
    let data = json["data"].as_array().ok_or_else(|| SpeechError::InvalidResponse("the body has no `data` list".into()))?;
    let mut ids: Vec<String> = data.iter().filter_map(|m| m["id"].as_str()).map(str::to_string).collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_cut_at_a_word_boundary() {
        assert_eq!(truncate_prompt(&["Sayso", "Kubernetes"], 800), "Sayso, Kubernetes");
        // "aaaa, bbbb" is 10 characters. A third word does not fit in 12.
        assert_eq!(truncate_prompt(&["aaaa", "bbbb", "cccc"], 12), "aaaa, bbbb");
        assert_eq!(truncate_prompt(&["aaaa", "bbbb"], 10), "aaaa, bbbb");
        assert_eq!(truncate_prompt(&["aaaa", "bbbb"], 9), "aaaa");
        // A first word over the limit gives no prompt.
        assert_eq!(truncate_prompt(&["abcdef"], 5), "");
        assert_eq!(truncate_prompt(&[], 5), "");
    }

    #[test]
    fn prompt_counts_characters_not_bytes() {
        assert_eq!(truncate_prompt(&["äöü", "ñññ"], 8), "äöü, ñññ");
    }

    #[test]
    fn long_dictionaries_stay_under_the_limit() {
        let words: Vec<String> = (0..500).map(|i| format!("word{i}")).collect();
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let prompt = truncate_prompt(&refs, PROMPT_LIMIT);
        assert!(prompt.chars().count() <= PROMPT_LIMIT);
        assert!(prompt.ends_with(|c: char| c.is_ascii_digit()), "no cut word: {prompt}");
    }
}
