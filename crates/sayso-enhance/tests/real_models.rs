//! Lists models with the real `claude` and `codex` commands and the user's own
//! login. Run with `cargo test -p sayso-enhance --test real_models -- --ignored --nocapture`.

use sayso_core::config::{Provider, ProviderKind};
use sayso_enhance::{MemoryStore, detect_codex_cli, list_provider_models};
use std::time::Duration;

fn show(label: &str, kind: ProviderKind) -> Vec<String> {
    let provider = Provider {
        id: label.into(),
        name: label.into(),
        kind,
    };
    let choices = list_provider_models(&provider, &MemoryStore::new(), None, Duration::from_secs(20))
        .expect("model list");
    let ids: Vec<String> = choices.iter().map(|c| c.id.clone()).collect();
    eprintln!(
        "{label}: {} models, first: {:?}",
        ids.len(),
        ids.iter().take(5).collect::<Vec<_>>()
    );
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
        },
    );
    assert!(!ids.is_empty());
    assert!(ids.iter().any(|id| id == "haiku"), "no haiku in {ids:?}");
    assert!(!ids.iter().any(|id| id == "default"));
}

#[test]
#[ignore = "needs the codex command and a login"]
fn codex_lists_models() {
    if detect_codex_cli().is_none() {
        eprintln!("codex is not installed: skipped");
        return;
    }
    let ids = show(
        "codex",
        ProviderKind::CodexCli {
            path: None,
            model: None,
        },
    );
    assert!(!ids.is_empty());
}
