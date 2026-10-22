//! Read helpers on `AppModel` for the Hub pages. They only read state.

use crate::model::AppModel;
use sayso_core::cleanup::CleanupModelInfo;
use sayso_core::config::{Provider, ProviderKind};
use sayso_core::models::ModelId;
use sayso_core::style::{RunPlan, Style, StyleOrigin};
use sayso_i18n::t;
use sayso_platform::{Permission, PermissionState};

/// Where a style's text goes, for the provider line on cards.
#[derive(Debug, Clone, PartialEq)]
pub enum StyleRoute {
    /// Raw, or a style with no prompt.
    NoAi,
    /// The style has a prompt but AI is off or has no provider.
    NeedsProvider,
    /// The provider is set up, but no model is chosen for it yet.
    NeedsModel { provider: String },
    Local { provider: String, model: String },
    Cloud { provider: String, model: String },
    /// The provider is a cleanup model that cannot run the style. It does a
    /// cleanup only, on this computer.
    CleanupCannotRun { model: String },
}

impl StyleRoute {
    /// "Cloud · OpenRouter · Llama 3.3 8B", "Nothing leaves your Mac".
    pub fn line(&self) -> String {
        match self {
            StyleRoute::NoAi => t!("styles.route.no_ai", computer = crate::shell::computer()),
            StyleRoute::NeedsProvider => t!("styles.route.needs_provider"),
            StyleRoute::NeedsModel { provider } => t!("styles.route.needs_model", provider = provider),
            StyleRoute::Local { provider, model } if model.is_empty() => t!("styles.route.local_no_model", provider = provider),
            StyleRoute::Local { provider, model } => t!("styles.route.local", provider = provider, model = model),
            StyleRoute::Cloud { provider, model } if model.is_empty() => t!("styles.route.cloud_no_model", provider = provider),
            StyleRoute::Cloud { provider, model } => t!("styles.route.cloud", provider = provider, model = model),
            StyleRoute::CleanupCannotRun { model } => t!("styles.route.cleanup_cannot_run", model = model),
        }
    }
}

/// The catalog entry of a provider that is a cleanup model. None for any
/// other provider, and for a model that this version does not know.
pub fn cleanup_model_info(p: &Provider) -> Option<&'static CleanupModelInfo> {
    match &p.kind {
        ProviderKind::CleanupModel { model } => sayso_core::cleanup::find(model),
        _ => None,
    }
}

/// What a cleanup model does with a style. `name` is the name of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupRun {
    /// The model runs the style with its settings.
    Runs { name: String },
    /// The model cannot run the style. It cleans up with the default settings.
    CannotRun { name: String },
}

/// What a cleanup model does with a style. None when the provider is not a
/// cleanup model, and when the style runs no AI on it.
pub fn cleanup_run(style: &Style, p: &Provider) -> Option<CleanupRun> {
    let name = cleanup_model_info(p)?.name.to_string();
    match style.run_plan(p) {
        RunPlan::Cleanup(_) => Some(CleanupRun::Runs { name }),
        RunPlan::CleanupOnly => Some(CleanupRun::CannotRun { name }),
        RunPlan::Prompt | RunPlan::Nothing => None,
    }
}

/// "meta-llama/llama-3.3-8b-instruct" becomes "llama-3.3-8b-instruct".
pub fn short_model(model: &str) -> String {
    model.rsplit('/').next().unwrap_or(model).to_string()
}

/// A one-line description of a provider for the Providers page.
pub fn provider_detail(p: &Provider) -> String {
    match &p.kind {
        ProviderKind::OpenAiCompatible { base_url, zero_data_retention, .. } => {
            let host = base_url.split("://").nth(1).unwrap_or(base_url).trim_end_matches('/');
            let host = host.strip_suffix("/v1").unwrap_or(host);
            if p.is_cloud() {
                if *zero_data_retention { t!("hub.provider.cloud_zero_retention") } else { t!("hub.provider.cloud_host", host = host) }
            } else {
                t!("hub.provider.local_host", host = host)
            }
        }
        // The time depends on the model: about 1.8 s with Haiku.
        ProviderKind::ClaudeCli { .. } => t!("hub.provider.claude_cli"),
        ProviderKind::CodexCli { .. } => t!("hub.provider.codex_cli"),
        ProviderKind::AppleIntelligence => t!("hub.provider.apple_intelligence"),
        // The model id, until the cleanup models have a row of their own.
        ProviderKind::CleanupModel { model } => model.clone(),
    }
}

impl AppModel {
    pub fn style_origin(&self, id: &str) -> Option<StyleOrigin> {
        self.styles.entries.iter().find(|e| e.style.id == id).map(|e| e.origin)
    }

    /// The style name for a history entry, or its id when the style is gone.
    pub fn style_name(&self, id: &str) -> String {
        self.styles.get(id).map(|s| s.display_name()).unwrap_or_else(|| id.to_string())
    }

    pub fn provider_name(&self, id: &str) -> String {
        self.config.ai.providers.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string())
    }

    /// The name of a model in a history entry. The entry can be older than the
    /// catalog or name a provider that is gone, so an unknown id shows as it is
    /// (a cloud id without its provider part).
    pub fn model_name(&self, id: &ModelId) -> String {
        sayso_core::models::find_with(&self.config.speech, id)
            .map(|m| m.name)
            .unwrap_or_else(|| sayso_core::speech::split_model_id(id).map_or(id.as_str(), |(_, model)| model).to_string())
    }

    /// Where the text of a style goes now.
    pub fn style_route(&self, style: &Style) -> StyleRoute {
        if style.is_raw() {
            return StyleRoute::NoAi;
        }
        let Some(p) = self.provider_for(style) else { return StyleRoute::NeedsProvider };
        // A cleanup model has no prompt and no model to choose. It cleans up
        // on this computer, also a style that it cannot run (with the default settings).
        match cleanup_run(style, p) {
            Some(CleanupRun::Runs { name }) => return StyleRoute::Local { provider: name, model: String::new() },
            Some(CleanupRun::CannotRun { name }) => return StyleRoute::CleanupCannotRun { model: name },
            None if cleanup_model_info(p).is_some() => return StyleRoute::NoAi,
            None => {}
        }
        if !style.uses_ai() {
            return StyleRoute::NoAi;
        }
        let model = match style.model.as_deref().or(p.model()) {
            Some(id) => self.model_label(&p.id, id),
            None if p.needs_model() => return StyleRoute::NeedsModel { provider: p.name.clone() },
            None => String::new(),
        };
        let provider = p.name.clone();
        if p.is_cloud() { StyleRoute::Cloud { provider, model } } else { StyleRoute::Local { provider, model } }
    }

    /// The name of the default microphone, when macOS reports one.
    pub fn microphone_name(&self) -> Option<String> {
        let devices = self.input_devices();
        if let Some(id) = &self.config.audio.input_device
            && let Some(d) = devices.iter().find(|d| &d.id == id || &d.name == id) {
                return Some(d.name.clone());
            }
        devices.iter().find(|d| d.is_default).or(devices.first()).map(|d| d.name.clone())
    }

    pub fn granted(&self, p: Permission) -> bool {
        self.permission(p) == PermissionState::Granted
    }

    /// Ask for a permission, or open System Settings when macOS already said no.
    pub fn fix_permission(&mut self, p: Permission) {
        if self.permission(p) == PermissionState::NotDetermined {
            self.request_permission(p);
        } else {
            self.open_permission_settings(p);
        }
    }

    /// The models folder as the user sees it ("~/Library/...").
    pub fn models_dir_display(&self) -> String {
        let home = sayso_core::paths::PathEnv::home(&sayso_core::paths::SystemEnv);
        sayso_core::paths::Paths::display(&self.paths.models_dir(), &home)
    }

    pub fn styles_dir_display(&self) -> String {
        let home = sayso_core::paths::PathEnv::home(&sayso_core::paths::SystemEnv);
        sayso_core::paths::Paths::display(&self.paths.styles_dir(), &home)
    }

    /// Median final-pass time and dictation length for a model, from recent history.
    pub fn measured_speed(&self, id: &ModelId) -> Option<(u64, u64)> {
        let mut rows: Vec<(u64, u64)> =
            self.recent.iter().filter(|e| &e.model == id && e.transcribe_ms > 0).map(|e| (e.transcribe_ms, e.duration_ms)).collect();
        if rows.is_empty() {
            return None;
        }
        rows.sort();
        let mid = rows[rows.len() / 2];
        let mut durations: Vec<u64> = rows.iter().map(|r| r.1).collect();
        durations.sort();
        Some((mid.0, durations[durations.len() / 2]))
    }

    /// A unique provider id from a name.
    pub fn new_provider_id(&self, name: &str) -> String {
        let base = super::kit::slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.config.ai.providers.iter().any(|p| p.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// A unique style id from a name.
    pub fn new_style_id(&self, name: &str) -> String {
        let base = super::kit::slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.styles.get(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_core::cleanup::{CleanupSettings, Format, Layout, Tone};
    use sayso_core::style::{CleanupField, builtin_styles};

    fn cleanup_model(model: &str) -> Provider {
        Provider { id: "c".into(), name: "Custom name".into(), kind: ProviderKind::CleanupModel { model: model.into() } }
    }

    fn built_in(id: &str) -> Style {
        builtin_styles().into_iter().find(|s| s.id == id).unwrap()
    }

    fn runs(name: &str) -> Option<CleanupRun> {
        Some(CleanupRun::Runs { name: name.into() })
    }

    fn cannot_run(name: &str) -> Option<CleanupRun> {
        Some(CleanupRun::CannotRun { name: name.into() })
    }

    #[test]
    fn a_style_that_the_cleanup_model_runs_uses_the_model_name() {
        // The name is the name in the catalog, not the name of the provider.
        assert_eq!(cleanup_run(&built_in("clean"), &cleanup_model("s1-mini")), runs("S1-mini"));
    }

    #[test]
    fn a_style_that_the_cleanup_model_cannot_run_says_so() {
        // Polished has no cleanup settings. Email needs a format that Cleanup 1 lacks.
        assert_eq!(cleanup_run(&built_in("polished"), &cleanup_model("s1-mini")), cannot_run("S1-mini"));
        assert_eq!(cleanup_run(&built_in("email"), &cleanup_model("spokenly-cleanup-1")), cannot_run("Cleanup 1"));
    }

    #[test]
    fn a_style_with_a_value_that_is_not_known_cannot_run() {
        let mut style = built_in("clean");
        style.cleanup = CleanupField::Settings(CleanupSettings { layout: Some("table".into()), ..Default::default() });
        assert_eq!(cleanup_run(&style, &cleanup_model("s1-mini")), cannot_run("S1-mini"));
        style.cleanup = CleanupField::Settings(CleanupSettings::new(Tone::Casual, Layout::Paragraphs, Format::Plain));
        assert_eq!(cleanup_run(&style, &cleanup_model("spokenly-cleanup-1")), runs("Cleanup 1"));
    }

    #[test]
    fn raw_and_other_providers_have_no_cleanup_run() {
        let other = Provider { id: "a".into(), name: "A".into(), kind: ProviderKind::AppleIntelligence };
        assert_eq!(cleanup_run(&built_in("clean"), &other), None);
        assert_eq!(cleanup_run(&built_in("raw"), &cleanup_model("s1-mini")), None);
        assert_eq!(cleanup_run(&built_in("clean"), &cleanup_model("no-such-model")), None);
    }
}
