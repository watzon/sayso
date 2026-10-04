//! AI enhancement providers for Sayso: the `Enhancer` implementations behind
//! the contract in `sayso_core::enhance`.
//!
//! - [`OpenAiCompatible`]: HTTP to OpenRouter, OpenAI, Ollama, LM Studio, llama.cpp.
//! - [`ClaudeCli`]: the user's own `claude` command.
//! - [`CodexCli`]: the user's own `codex` command (experimental).
//! - [`EngineModel`]: the language model that the engine runs on this computer
//!   (Apple Intelligence on macOS).
//!
//! Use [`build_enhancer`] to turn a provider from `config.toml` into an
//! `Enhancer`. API keys come from a [`SecretStore`], never from the config.
//! All calls block: run them on a background thread.

mod claude;
mod cli;
mod codex;
mod detect;
mod engine;
mod models;
mod openai;
mod secrets;
#[cfg(windows)]
mod win_shim;

pub use claude::ClaudeCli;
pub use codex::CodexCli;
pub use engine::EngineModel;
pub use detect::{detect_claude_cli, detect_codex_cli, detect_ollama, detect_ollama_at, list_models, test_provider};
pub use models::list_provider_models;
pub use openai::{OpenAiCompatible, clear_mode_cache};
pub use secrets::{KEYCHAIN_SERVICE, KeychainStore, MemoryStore, SecretError, SecretStore};

use sayso_core::config::{Ai, Provider, ProviderKind};
use sayso_core::enhance::{Enhancer, LanguageModel};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Build the enhancer for a configured provider.
///
/// This never fails. A provider that cannot work yet, for example one whose
/// API key is missing, still gets an enhancer: its `enhance` returns
/// `EnhanceError::NotConfigured`, and the pipeline then inserts the raw
/// transcript. `cfg` sets the default timeout, used by requests with a zero timeout.
/// `engine` is the language model of the engine, None when the engine does not run.
pub fn build_enhancer(
    provider: &Provider,
    secrets: &dyn SecretStore,
    cfg: &Ai,
    engine: Option<Arc<dyn LanguageModel>>,
) -> Box<dyn Enhancer> {
    let http_timeout = Duration::from_millis(cfg.http_timeout_ms);
    let cli_timeout = Duration::from_millis(cfg.cli_timeout_ms);
    match &provider.kind {
        ProviderKind::OpenAiCompatible { base_url, model, api_key_account, zero_data_retention } => {
            let mut enhancer = OpenAiCompatible::new(&provider.id, base_url, model)
                .with_cloud(provider.is_cloud())
                .with_zero_data_retention(*zero_data_retention)
                .with_default_timeout(http_timeout);
            if let Some(account) = api_key_account {
                match secrets.get(account) {
                    Some(key) => enhancer = enhancer.with_api_key(Some(key)),
                    None => {
                        enhancer = enhancer.with_setup_error(format!("no API key is saved for \"{account}\""));
                    }
                }
            }
            Box::new(enhancer)
        }
        ProviderKind::ClaudeCli { path, model } => {
            Box::new(ClaudeCli::new(&provider.id, path.as_ref().map(PathBuf::from), model).with_default_timeout(cli_timeout))
        }
        ProviderKind::CodexCli { path, model } => Box::new(
            CodexCli::new(&provider.id, path.as_ref().map(PathBuf::from), model.clone()).with_default_timeout(cli_timeout),
        ),
        ProviderKind::AppleIntelligence => Box::new(EngineModel::new(&provider.id, engine).with_default_timeout(http_timeout)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_core::enhance::{EnhanceError, EnhanceRequest, Generated};

    fn provider(kind: ProviderKind) -> Provider {
        Provider { id: "p".into(), name: "P".into(), kind }
    }

    fn request() -> EnhanceRequest {
        EnhanceRequest {
            system_prompt: "s".into(),
            transcript: "t".into(),
            model: None,
            temperature: None,
            timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn factory_sets_id_and_cloud_flag() {
        let cfg = Ai::default();
        let secrets = MemoryStore::new();
        let local = provider(ProviderKind::OpenAiCompatible {
            base_url: "http://localhost:11434/v1".into(),
            model: "llama".into(),
            api_key_account: None,
            zero_data_retention: false,
        });
        let e = build_enhancer(&local, &secrets, &cfg, None);
        assert_eq!(e.id(), "p");
        assert!(!e.is_cloud());
        let cloud = provider(ProviderKind::OpenAiCompatible {
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "m".into(),
            api_key_account: None,
            zero_data_retention: false,
        });
        assert!(build_enhancer(&cloud, &secrets, &cfg, None).is_cloud());
        let claude = provider(ProviderKind::ClaudeCli { path: Some("/nowhere/claude".into()), model: "haiku".into() });
        assert!(build_enhancer(&claude, &secrets, &cfg, None).is_cloud());
        let codex = provider(ProviderKind::CodexCli { path: Some("/nowhere/codex".into()), model: None });
        assert!(build_enhancer(&codex, &secrets, &cfg, None).is_cloud());
    }

    #[test]
    fn missing_api_key_gives_not_configured() {
        let p = provider(ProviderKind::OpenAiCompatible {
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "m".into(),
            api_key_account: Some("openrouter".into()),
            zero_data_retention: false,
        });
        let e = build_enhancer(&p, &MemoryStore::new(), &Ai::default(), None);
        assert!(matches!(e.enhance(&request()), Err(EnhanceError::NotConfigured(m)) if m.contains("openrouter")));
    }

    /// A language model that answers with a fixed text, or with an error.
    struct Scripted(Result<&'static str, EnhanceError>);

    impl LanguageModel for Scripted {
        fn model_name(&self) -> Result<String, EnhanceError> {
            Ok("Scripted".into())
        }

        fn generate(&self, instructions: &str, prompt: &str, _: Option<f32>, timeout: Duration) -> Result<Generated, EnhanceError> {
            assert_eq!((instructions, prompt), ("s", "<transcript>\nt\n</transcript>"));
            assert_eq!(timeout, Duration::from_secs(1));
            self.0.clone().map(|text| Generated { text: text.into(), model: "Scripted".into() })
        }
    }

    #[test]
    fn apple_intelligence_runs_in_the_engine() {
        let p = provider(ProviderKind::AppleIntelligence);
        let run = |reply| {
            let engine: Arc<dyn LanguageModel> = Arc::new(Scripted(reply));
            build_enhancer(&p, &MemoryStore::new(), &Ai::default(), Some(engine)).enhance(&request())
        };
        let done = run(Ok(" Hello. ")).unwrap();
        assert_eq!((done.text.as_str(), done.provider_id.as_str(), done.model.as_str()), ("Hello.", "p", "Scripted"));
        assert_eq!(done.mode, "guided_generation");
        assert!(matches!(run(Ok("  ")), Err(EnhanceError::InvalidOutput(_))));
        assert_eq!(run(Err(EnhanceError::Refused("no".into()))), Err(EnhanceError::Refused("no".into())));

        let stopped = build_enhancer(&p, &MemoryStore::new(), &Ai::default(), None);
        assert!(!stopped.is_cloud());
        assert!(matches!(stopped.enhance(&request()), Err(EnhanceError::NotConfigured(_))));
    }

    #[test]
    fn missing_cli_binary_gives_not_configured() {
        let p = provider(ProviderKind::ClaudeCli { path: Some("/nowhere/claude".into()), model: "haiku".into() });
        let e = build_enhancer(&p, &MemoryStore::new(), &Ai::default(), None);
        assert!(matches!(e.enhance(&request()), Err(EnhanceError::NotConfigured(_))));
    }
}
