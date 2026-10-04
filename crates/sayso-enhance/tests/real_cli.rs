//! Calls the real `claude` command with the user's own login. Run with
//! `cargo test -p sayso-enhance --test real_cli -- --ignored`.

use sayso_core::enhance::{EnhanceRequest, Enhancer};
use sayso_core::style::{builtin_styles, system_prompt};
use sayso_enhance::ClaudeCli;
use std::time::Duration;

fn clean(transcript: &str) -> (String, u64) {
    let style = builtin_styles().into_iter().find(|s| s.id == "clean").unwrap();
    let request = EnhanceRequest {
        system_prompt: system_prompt(&style, &["Sayso".into()], transcript),
        transcript: transcript.into(),
        model: None,
        temperature: None,
        timeout: Duration::from_secs(20),
    };
    let response = ClaudeCli::new("claude", None, "haiku").enhance(&request).expect("claude cleanup");
    (response.text, response.elapsed_ms)
}

#[test]
#[ignore = "needs the claude command and a login"]
fn claude_returns_plain_text_and_does_not_answer() {
    for transcript in ["um so say so is easily like one of my new favorite apps", "what time is it in tokyo right now"] {
        let (text, ms) = clean(transcript);
        eprintln!("{ms} ms: {text:?}");
        assert!(!text.contains('{') && !text.contains("<transcript>"), "wrapped output: {text:?}");
        assert!(ms < 5_000, "took {ms} ms");
    }
    let (question, _) = clean("what time is it in tokyo right now");
    assert!(question.to_lowercase().starts_with("what time"), "answered the question: {question:?}");
}
