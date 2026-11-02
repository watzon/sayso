//! AI enhancement providers for Sayso: the `Enhancer` implementations behind
//! the contract in `sayso_core::enhance`.
//!
//! - [`OpenAiCompatible`]: HTTP to OpenRouter, OpenAI, Ollama, LM Studio, llama.cpp.
//! - [`ClaudeCli`]: the user's own `claude` command.
//! - [`ChatGpt`]: the user's ChatGPT plan, with a sign-in that Sayso keeps.
//! - [`AcpAgent`]: any agent that speaks the Agent Client Protocol, for
//!   example `opencode acp`. The [`registry`] lists the known agents.
//! - [`EngineModel`]: the language model that the engine runs on this computer
//!   (Apple Intelligence on macOS).
//!
//! Use [`build_enhancer`] to turn a provider from `config.toml` into an
//! `Enhancer`. API keys and the ChatGPT sign-in come from a [`SecretStore`],
//! never from the config.
//! All calls block: run them on a background thread.

mod acp;
mod auth;
mod chatgpt;
mod claude;
mod cli;
mod detect;
mod engine;
mod models;
mod openai;
mod secrets;
#[cfg(windows)]
mod win_shim;

pub use acp::registry;
pub use acp::{AcpAgent, AgentCheck, IDLE_LIMIT as ACP_IDLE_LIMIT, check_agent, secret_account as acp_secret_account, stop_agent as stop_acp_agent};
pub use auth::{AuthStatus, SIGN_IN_LIMIT, SignIn, SignInEvent, auth_status};
pub use chatgpt::ChatGpt;
pub use claude::ClaudeCli;
pub use engine::EngineModel;
pub use detect::{detect_claude_cli, detect_ollama, detect_ollama_at, list_models, test_provider};
pub use models::list_provider_models;
pub use openai::{OpenAiCompatible, clear_mode_cache};
/// Remove the saved ChatGPT sign-in of a provider.
pub use chatgpt::sign_out as chatgpt_sign_out;
pub use secrets::{KEYCHAIN_SERVICE, KeychainStore, MemoryStore, SecretError, SecretStore};

use sayso_core::config::{Ai, Provider, ProviderKind};
use sayso_core::enhance::{EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, LanguageModel};
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
    secrets: &Arc<dyn SecretStore>,
    cfg: &Ai,
    engine: Option<Arc<dyn LanguageModel>>,
) -> Box<dyn Enhancer> {
    let http_timeout = Duration::from_millis(cfg.http_timeout_ms);
    let cli_timeout = Duration::from_millis(cfg.cli_timeout_ms);
    match &provider.kind {
        ProviderKind::OpenAiCompatible { base_url, model, api_key_account, zero_data_retention, .. } => {
            let mut enhancer = OpenAiCompatible::new(&provider.id, base_url, model)
                .with_cloud(provider.is_cloud())
                .with_zero_data_retention(*zero_data_retention)
                .with_thinking(provider.thinking_sent())
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
        ProviderKind::ClaudeCli { path, model, .. } => Box::new(
            ClaudeCli::new(&provider.id, path.as_ref().map(PathBuf::from), model)
                .with_thinking(provider.thinking_sent())
                .with_config_dir(provider.config_dir().map(PathBuf::from))
                .with_default_timeout(cli_timeout),
        ),
        // A network call like the HTTP providers, but the backend thinks first, so it gets the longer CLI timeout.
        ProviderKind::ChatGpt { model, .. } => Box::new(
            ChatGpt::new(&provider.id, model.clone(), secrets.clone())
                .with_thinking(provider.thinking_sent())
                .with_default_timeout(cli_timeout),
        ),
        // The agent thinks and may run a turn of its own, so it gets the CLI timeout.
        ProviderKind::Acp { .. } => Box::new(AcpAgent::new(provider, secrets.as_ref()).with_default_timeout(cli_timeout)),
        ProviderKind::AppleIntelligence => Box::new(EngineModel::new(&provider.id, engine).with_default_timeout(http_timeout)),
        // A cleanup model reads no prompt: the app runs it as a `Cleaner`
        // (`sayso-cleanup-client`), never through this function.
        ProviderKind::CleanupModel { .. } => Box::new(CleanupModelStub { id: provider.id.clone() }),
    }
}

/// The enhancer of a cleanup model. A cleanup model reads no prompt, so it
/// cannot enhance.
struct CleanupModelStub {
    id: String,
}

impl Enhancer for CleanupModelStub {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_cloud(&self) -> bool {
        false
    }

    fn enhance(&self, _request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
        Err(EnhanceError::NotConfigured("cleanup models run in the cleanup runtime".into()))
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

    fn store() -> Arc<dyn SecretStore> {
        Arc::new(MemoryStore::new())
    }

    #[test]
    fn factory_sets_id_and_cloud_flag() {
        let cfg = Ai::default();
        let secrets = store();
        let local = provider(ProviderKind::OpenAiCompatible {
            base_url: "http://localhost:11434/v1".into(),
            model: "llama".into(),
            api_key_account: None,
            zero_data_retention: false,
            thinking: None,
        });
        let e = build_enhancer(&local, &secrets, &cfg, None);
        assert_eq!(e.id(), "p");
        assert!(!e.is_cloud());
        let cloud = provider(ProviderKind::OpenAiCompatible {
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "m".into(),
            api_key_account: None,
            zero_data_retention: false,
            thinking: None,
        });
        assert!(build_enhancer(&cloud, &secrets, &cfg, None).is_cloud());
        let claude = provider(ProviderKind::ClaudeCli { path: Some("/nowhere/claude".into()), model: "haiku".into(), config_dir: None, thinking: Default::default() });
        assert!(build_enhancer(&claude, &secrets, &cfg, None).is_cloud());
        let chatgpt = provider(ProviderKind::ChatGpt { model: None, thinking: None });
        assert!(build_enhancer(&chatgpt, &secrets, &cfg, None).is_cloud());
        assert!(build_enhancer(&acp(vec!["KEY".into()]), &secrets, &cfg, None).is_cloud());
    }

    fn acp(secret_env: Vec<String>) -> Provider {
        provider(ProviderKind::Acp {
            command: "/nowhere/agent".into(),
            args: vec!["acp".into()],
            env: Default::default(),
            secret_env,
            registry: None,
            model: None,
            keep_running: false,
        })
    }

    #[test]
    fn acp_without_a_secret_value_or_a_command_gives_not_configured() {
        let e = build_enhancer(&acp(vec!["AGENT_KEY".into()]), &store(), &Ai::default(), None);
        assert!(matches!(e.enhance(&request()), Err(EnhanceError::NotConfigured(m)) if m.contains("AGENT_KEY")));
        let e = build_enhancer(&acp(Vec::new()), &store(), &Ai::default(), None);
        assert!(matches!(e.enhance(&request()), Err(EnhanceError::NotConfigured(m)) if m.contains("could not start")));
    }

    #[test]
    fn missing_api_key_gives_not_configured() {
        let p = provider(ProviderKind::OpenAiCompatible {
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "m".into(),
            api_key_account: Some("openrouter".into()),
            zero_data_retention: false,
            thinking: None,
        });
        let e = build_enhancer(&p, &store(), &Ai::default(), None);
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
            build_enhancer(&p, &store(), &Ai::default(), Some(engine)).enhance(&request())
        };
        let done = run(Ok(" Hello. ")).unwrap();
        assert_eq!((done.text.as_str(), done.provider_id.as_str(), done.model.as_str()), ("Hello.", "p", "Scripted"));
        assert_eq!(done.mode, "guided_generation");
        assert!(matches!(run(Ok("  ")), Err(EnhanceError::InvalidOutput(_))));
        assert_eq!(run(Err(EnhanceError::Refused("no".into()))), Err(EnhanceError::Refused("no".into())));

        let stopped = build_enhancer(&p, &store(), &Ai::default(), None);
        assert!(!stopped.is_cloud());
        assert!(matches!(stopped.enhance(&request()), Err(EnhanceError::NotConfigured(_))));
    }

    #[test]
    fn missing_cli_binary_gives_not_configured() {
        let p = provider(ProviderKind::ClaudeCli { path: Some("/nowhere/claude".into()), model: "haiku".into(), config_dir: None, thinking: Default::default() });
        let e = build_enhancer(&p, &store(), &Ai::default(), None);
        assert!(matches!(e.enhance(&request()), Err(EnhanceError::NotConfigured(_))));
    }

    #[test]
    fn chatgpt_without_a_sign_in_gives_not_configured() {
        let p = provider(ProviderKind::ChatGpt { model: Some("gpt-mini".into()), thinking: None });
        let e = build_enhancer(&p, &store(), &Ai::default(), None);
        assert!(matches!(e.enhance(&request()), Err(EnhanceError::NotConfigured(m)) if m.contains("ChatGPT")));
    }
}
