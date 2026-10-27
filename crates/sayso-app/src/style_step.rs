//! The style step: run a style on text with an AI provider or a cleanup
//! model. A dictation, the "Enhance" of a history entry, and `enhance` of an
//! extension (`host.rs`) use this code. The step blocks, so it runs on a
//! background thread.

use sayso_cleanup_client::cleaner::RuntimeCleaner;
use sayso_core::config::{Ai, Provider, ProviderKind};
use sayso_core::dictionary::AppliedReplacement;
use sayso_core::enhance::LanguageModel;
use sayso_core::pipeline::{CleanerRun, PipelineOutput, TextPipeline};
use sayso_core::speech::Speech;
use sayso_core::style::Style;
use sayso_enhance::SecretStore;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The data of a dictation that a cleanup model reads. Built on the UI thread,
/// used on a background thread.
pub(crate) struct CleanupContext {
    cleaner: Arc<RuntimeCleaner>,
    model: &'static sayso_core::cleanup::CleanupModelInfo,
    language_setting: String,
    final_model_languages: Vec<&'static str>,
    app: Option<String>,
    site: Option<String>,
}

impl CleanupContext {
    /// What a cleanup model needs, when `provider` is one. `language` is the
    /// language setting of the dictation and `speech_model` the model of its
    /// final pass.
    pub(crate) fn for_provider(
        cleaner: &Arc<RuntimeCleaner>,
        speech: &Speech,
        provider: &Provider,
        app: Option<String>,
        site: Option<String>,
        language: String,
        speech_model: &sayso_core::models::ModelId,
    ) -> Option<CleanupContext> {
        let ProviderKind::CleanupModel { model } = &provider.kind else { return None };
        let model = sayso_core::cleanup::find(model)?;
        let final_model_languages = sayso_core::models::find_with(speech, speech_model).map(|info| info.languages.to_vec()).unwrap_or_default();
        Some(CleanupContext { cleaner: cleaner.clone(), model, language_setting: language, final_model_languages, app, site })
    }

    pub(crate) fn run<'a>(&'a self, style: &Style, provider: &'a Provider) -> CleanerRun<'a> {
        CleanerRun {
            cleaner: self.cleaner.as_ref(),
            provider_id: &provider.id,
            model: self.model,
            plan: style.run_plan(provider),
            language_setting: &self.language_setting,
            final_model_languages: &self.final_model_languages,
            app: self.app.as_deref(),
            site: self.site.as_deref(),
        }
    }
}

/// The time that a provider gets for the style step of a dictation. The
/// language model of the engine and a cleanup model answer as fast as a server.
pub(crate) fn style_timeout(ai: &Ai, provider: &Provider) -> Duration {
    Duration::from_millis(match provider.kind {
        ProviderKind::OpenAiCompatible { .. } | ProviderKind::AppleIntelligence | ProviderKind::CleanupModel { .. } => ai.http_timeout_ms,
        _ => ai.cli_timeout_ms,
    })
}

/// All that the style step needs, collected on the UI thread (or by the host)
/// and sent to a background thread.
pub(crate) struct StyleStep {
    pub ai: Ai,
    pub secrets: Arc<dyn SecretStore>,
    pub engine: Option<Arc<dyn LanguageModel>>,
    pub vocabulary: Vec<String>,
    pub base_prompt: String,
    /// Some when the provider is a cleanup model.
    pub cleanup: Option<CleanupContext>,
    pub timeout: Duration,
}

impl StyleStep {
    /// Run `style` with `provider` on `text`. The timings tell the setup time,
    /// the time of the whole step, and the provider.
    pub(crate) fn run(&self, style: &Style, provider: &Provider, text: String, replacements: Vec<AppliedReplacement>) -> PipelineOutput {
        let started = Instant::now();
        let replacer = sayso_core::dictionary::Replacer::new(&[]);
        let (base_prompt, vocabulary, timeout) = (&self.base_prompt, &self.vocabulary, self.timeout);
        let mut out = match &self.cleanup {
            Some(c) => {
                let p = TextPipeline { replacer: &replacer, style, base_prompt, vocabulary, enhancer: None, cleaner: Some(c.run(style, provider)), timeout };
                p.run_style(text, replacements)
            }
            None => {
                let enhancer = sayso_enhance::build_enhancer(provider, self.secrets.as_ref(), &self.ai, self.engine.clone());
                let setup_ms = started.elapsed().as_millis() as u64;
                let p = TextPipeline { replacer: &replacer, style, base_prompt, vocabulary, enhancer: Some(enhancer.as_ref()), cleaner: None, timeout };
                let mut out = p.run_style(text, replacements);
                if let Some(timings) = &mut out.timings {
                    timings.setup_ms = setup_ms;
                }
                out
            }
        };
        if let Some(timings) = &mut out.timings {
            timings.step_ms = started.elapsed().as_millis() as u64;
            timings.provider_kind = provider.kind.name().into();
            timings.thinking = provider.thinking_sent().map(str::to_string);
        }
        out
    }
}
