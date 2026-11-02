//! Lists models with the real `claude` command and the user's own login, and
//! with the ChatGPT sign-in that Sayso saved in the Keychain. Run with `cargo test -p sayso-enhance --test real_models -- --ignored --nocapture`.

use sayso_core::config::{Provider, ProviderKind};
use sayso_enhance::{KeychainStore, MemoryStore, SecretStore, list_provider_models};
use std::time::Duration;

fn show(label: &str, kind: ProviderKind, secrets: &dyn SecretStore) -> Vec<String> {
    let provider = Provider {
        id: label.into(),
        name: label.into(),
        kind,
    };
    let choices = list_provider_models(&provider, secrets, None, Duration::from_secs(20))
        .expect("model list");
    let ids: Vec<String> = choices.iter().map(|c| c.id.clone()).collect();
    eprintln!(
        "{label}: {} models, first: {:?}",
        ids.len(),
        ids.iter().take(5).collect::<Vec<_>>()
    );
    for choice in &choices {
        eprintln!("  {}: thinking {:?}", choice.id, choice.thinking);
    }
    assert!(choices.iter().any(|c| c.thinking.iter().any(|level| level == "low")), "no model reports a low thinking level");
    ids
}

#[test]
#[ignore = "needs the claude command and a login"]
fn claude_lists_models() {
    let ids = show(
        "claude",
        ProviderKind::ClaudeCli {
            path: None,
            model: "haiku".into(),
            config_dir: None,
            thinking: Default::default(),
        },
        &MemoryStore::new(),
    );
    assert!(!ids.is_empty());
    assert!(ids.iter().any(|id| id == "haiku"), "no haiku in {ids:?}");
    assert!(!ids.iter().any(|id| id == "default"));
}

/// The id of the ChatGPT provider in Sayso: `SAYSO_CHATGPT_PROVIDER`, else "chatgpt".
#[test]
#[ignore = "needs a ChatGPT sign-in that Sayso saved in the Keychain"]
fn chatgpt_lists_models() {
    let id = std::env::var("SAYSO_CHATGPT_PROVIDER").unwrap_or_else(|_| "chatgpt".into());
    let ids = show(&id, ProviderKind::ChatGpt { model: None, thinking: None }, &KeychainStore);
    assert!(!ids.is_empty());
}
