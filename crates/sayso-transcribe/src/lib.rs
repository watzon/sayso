//! Cloud speech-to-text clients for the final pass. All calls block.
//!
//! One function per job, each for any [`SpeechProvider`]:
//!
//! - [`transcribe`] sends one WAV recording and returns the transcript.
//! - [`check`] proves that the address and the key work.
//! - [`list_models`] asks the server for its model ids.
//!
//! The caller reads the API key from the Keychain and passes it in. This crate
//! never stores, logs, or returns a key. The audio is never logged either.

mod assemblyai;
mod deepgram;
mod elevenlabs;
mod http;
mod multipart;
mod openai;

use http::{Deadline, Target};
use sayso_core::speech::{SpeechError, SpeechProvider, SpeechProviderKind, SpeechRequest};
use std::time::Duration;

/// Run the final pass on one recording. Blocks. Returns the transcript, trimmed.
///
/// A recording without speech gives `Ok("")`. `request.timeout` is a deadline
/// for the whole call, retries included. A zero timeout means 30 s.
///
/// The language and the dictionary words are hints. If the provider answers
/// HTTP 400 or 422 to a request that has a hint, the request goes out once
/// more without the hints, and that second result comes back. A provider that
/// rejects a hint field (an unknown field, an unsupported language, a word it
/// does not accept) then still gives a transcript.
pub fn transcribe(provider: &SpeechProvider, api_key: Option<&str>, request: &SpeechRequest) -> Result<String, SpeechError> {
    let target = Target::new(provider, api_key, request.model)?;
    let deadline = Deadline::new(request.timeout);
    let has_hints = http::language(request).is_some() || !http::words(request).is_empty();
    let text = match send(&target, request, &deadline) {
        Err(SpeechError::Http { status: status @ (400 | 422), .. }) if has_hints => {
            log::debug!("{}: HTTP {status} with hints, sending the request again without them", provider.id);
            send(&target, &SpeechRequest { language: None, vocabulary: &[], ..request.clone() }, &deadline)
        }
        other => other,
    }?;
    log::debug!("{}: transcribed with {:?} in {} ms", provider.id, request.model, deadline.elapsed_ms());
    Ok(text.trim().to_string())
}

/// One request to the provider's adapter.
fn send(target: &Target, request: &SpeechRequest, deadline: &Deadline) -> Result<String, SpeechError> {
    match target.kind {
        SpeechProviderKind::OpenAi | SpeechProviderKind::Groq | SpeechProviderKind::Mistral | SpeechProviderKind::OpenAiCompatible => {
            openai::transcribe(target, request, deadline)
        }
        SpeechProviderKind::ElevenLabs => elevenlabs::transcribe(target, request, deadline),
        SpeechProviderKind::Deepgram => deepgram::transcribe(target, request, deadline),
        SpeechProviderKind::AssemblyAi => assemblyai::transcribe(target, request, deadline),
    }
}

/// A cheap request that proves the address and the key work. Sends no audio
/// where the provider has a list endpoint.
pub fn check(provider: &SpeechProvider, api_key: Option<&str>, timeout: Duration) -> Result<(), SpeechError> {
    let target = Target::new(provider, api_key, default_model(provider))?;
    let deadline = Deadline::new(timeout);
    match provider.kind {
        SpeechProviderKind::OpenAi | SpeechProviderKind::Groq | SpeechProviderKind::Mistral | SpeechProviderKind::OpenAiCompatible => {
            openai::check(&target, &deadline)
        }
        SpeechProviderKind::ElevenLabs => elevenlabs::check(&target, &deadline),
        SpeechProviderKind::Deepgram => deepgram::check(&target, &deadline),
        SpeechProviderKind::AssemblyAi => assemblyai::check(&target, &deadline),
    }
}

/// Model ids the server reports (`GET {base}/models`, `data[].id`), sorted. Only
/// the OpenAI-compatible kinds have such a list. The other kinds return their
/// known ids and send no request.
pub fn list_models(provider: &SpeechProvider, api_key: Option<&str>, timeout: Duration) -> Result<Vec<String>, SpeechError> {
    match provider.kind {
        SpeechProviderKind::OpenAi | SpeechProviderKind::Groq | SpeechProviderKind::Mistral | SpeechProviderKind::OpenAiCompatible => {
            let target = Target::new(provider, api_key, "")?;
            openai::list_models(&target, &Deadline::new(timeout))
        }
        other => Ok(other.known_models().iter().map(|m| m.id.to_string()).collect()),
    }
}

/// The model that `check` names in its log line, and that AssemblyAI needs in a header.
fn default_model(provider: &SpeechProvider) -> &str {
    provider.kind.known_models().first().map(|m| m.id).or_else(|| provider.models.first().map(String::as_str)).unwrap_or("")
}
