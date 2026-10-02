//! Calls to the real providers. Each test is ignored. It reads a key from an
//! environment variable and skips itself when the variable is not set.
//!
//! ```text
//! SAYSO_TEST_GROQ_KEY=... cargo test -p sayso-transcribe --test real -- --ignored --nocapture
//! ```

use sayso_core::speech::{SpeechProvider, SpeechProviderKind, SpeechRequest};
use sayso_transcribe::{check, list_models, transcribe};
use std::time::Duration;

const SHORT_WAV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spikes/engine/audio/short.wav");

/// Check the key, then transcribe the short recording with the provider's first known model.
fn run(kind: SpeechProviderKind, env_var: &str) {
    let key = match std::env::var(env_var) {
        Ok(key) if !key.trim().is_empty() => key.trim().to_string(),
        _ => {
            println!("skipped: {env_var} is not set");
            return;
        }
    };
    let provider = SpeechProvider {
        id: "real".into(),
        name: kind.label().into(),
        kind,
        base_url: None,
        api_key_account: Some("speech.real".into()),
        models: vec![],
    };
    let wav = std::fs::read(SHORT_WAV).expect("read short.wav");
    let timeout = Duration::from_secs(30);

    check(&provider, Some(&key), timeout).expect("check");
    let models = list_models(&provider, Some(&key), timeout).expect("list_models");
    println!("{}: {} models", kind.label(), models.len());

    let model = kind.known_models()[0].id;
    let vocabulary = vec!["Sayso".to_string()];
    let request = SpeechRequest { model, wav: &wav, language: Some("en"), vocabulary: &vocabulary, timeout };
    let text = transcribe(&provider, Some(&key), &request).expect("transcribe");
    println!("{} {model}: {text:?}", kind.label());
    assert!(!text.is_empty(), "the recording has speech");

    // Without hints the provider must also answer.
    let plain = SpeechRequest { language: None, vocabulary: &[], ..request };
    let text = transcribe(&provider, Some(&key), &plain).expect("transcribe without hints");
    assert!(!text.is_empty());
}

#[test]
#[ignore = "needs SAYSO_TEST_OPENAI_KEY and network"]
fn openai() {
    run(SpeechProviderKind::OpenAi, "SAYSO_TEST_OPENAI_KEY");
}

#[test]
#[ignore = "needs SAYSO_TEST_GROQ_KEY and network"]
fn groq() {
    run(SpeechProviderKind::Groq, "SAYSO_TEST_GROQ_KEY");
}

#[test]
#[ignore = "needs SAYSO_TEST_ELEVENLABS_KEY and network"]
fn elevenlabs() {
    run(SpeechProviderKind::ElevenLabs, "SAYSO_TEST_ELEVENLABS_KEY");
}

#[test]
#[ignore = "needs SAYSO_TEST_DEEPGRAM_KEY and network"]
fn deepgram() {
    run(SpeechProviderKind::Deepgram, "SAYSO_TEST_DEEPGRAM_KEY");
}

#[test]
#[ignore = "needs SAYSO_TEST_ASSEMBLYAI_KEY and network"]
fn assemblyai() {
    run(SpeechProviderKind::AssemblyAi, "SAYSO_TEST_ASSEMBLYAI_KEY");
}

#[test]
#[ignore = "needs SAYSO_TEST_MISTRAL_KEY and network"]
fn mistral() {
    run(SpeechProviderKind::Mistral, "SAYSO_TEST_MISTRAL_KEY");
}
