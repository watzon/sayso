//! Read helpers on `AppModel` for the Hub pages. They only read state.

use crate::model::AppModel;
use sayso_core::config::{Provider, ProviderKind};
use sayso_core::models::ModelId;
use sayso_core::style::{Style, StyleOrigin};
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
}

impl StyleRoute {
    /// "Cloud · OpenRouter · Llama 3.3 8B", "Nothing leaves your Mac".
    pub fn line(&self) -> String {
        match self {
            StyleRoute::NoAi => format!("Nothing leaves your {}", crate::shell::COMPUTER),
            StyleRoute::NeedsProvider => "AI is off. Inserts the transcript.".into(),
            StyleRoute::NeedsModel { provider } => format!("Choose a model for {provider}"),
            StyleRoute::Local { provider, model } => join(["Local", provider, model]),
            StyleRoute::Cloud { provider, model } => join(["Cloud", provider, model]),
        }
    }
}

fn join<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// "meta-llama/llama-3.3-8b-instruct" becomes "llama-3.3-8b-instruct".
pub fn short_model(model: &str) -> String {
    model.rsplit('/').next().unwrap_or(model).to_string()
}

/// A one-line description of a provider for the Providers strip.
pub fn provider_detail(p: &Provider) -> String {
    match &p.kind {
        ProviderKind::OpenAiCompatible { base_url, zero_data_retention, .. } => {
            let host = base_url.split("://").nth(1).unwrap_or(base_url).trim_end_matches('/');
            let host = host.strip_suffix("/v1").unwrap_or(host);
            if p.is_cloud() {
                if *zero_data_retention { "Cloud · zero data retention on".into() } else { format!("Cloud · {host}") }
            } else {
                format!("Local · {host}")
            }
        }
        // The time depends on the model: about 1.8 s with Haiku.
        ProviderKind::ClaudeCli { .. } => "Cloud · your Claude login".into(),
        ProviderKind::CodexCli { .. } => "Cloud · about 5 s per call".into(),
    }
}

impl AppModel {
    pub fn style_origin(&self, id: &str) -> Option<StyleOrigin> {
        self.styles.entries.iter().find(|e| e.style.id == id).map(|e| e.origin)
    }

    /// The style name for a history entry, or its id when the style is gone.
    pub fn style_name(&self, id: &str) -> String {
        self.styles.get(id).map(|s| s.name.clone()).unwrap_or_else(|| id.to_string())
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
        if !style.uses_ai() {
            return StyleRoute::NoAi;
        }
        let Some(p) = self.provider_for(style) else { return StyleRoute::NeedsProvider };
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
