//! `AppModel`: the single source of state for every window.
//!
//! Views read it and call its methods. Blocking work (engine, store, AI,
//! insertion) runs on the background executor; results come back through
//! `this.update(...)`.

use crate::hub::Route;
use crate::live::LiveAudio;
use crate::services::Services;
use gpui_kit::*;
use sayso_core::config::{Config, ConfigIssue, Provider, ProviderKind};
use sayso_core::enhance::ModelChoice;
use sayso_core::dictation::{Event, Machine, State, Timing};
use sayso_core::dictionary::{Replacement, Replacer, Word};
use sayso_core::history::HistoryEntry;
use sayso_core::hotkey::Hotkey;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::speech::{SpeechProvider, split_model_id};
use sayso_core::paths::Paths;
use sayso_core::stats::{self, Stats};
use sayso_core::stt::ModelStatus;
use sayso_core::style::{Style, StyleLibrary};
use sayso_platform::{AudioDevice, HotkeyConflict, Permission, PermissionState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// AI providers found on this Mac, for onboarding and Styles.
#[derive(Debug, Clone, Default)]
pub struct Detected {
    pub claude_cli: Option<PathBuf>,
    pub codex_cli: Option<PathBuf>,
    /// Model names when Ollama is running.
    pub ollama: Option<Vec<String>>,
    pub done: bool,
}

/// A provider's models, as last fetched from the provider.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelList {
    Loading,
    Ready(Vec<ModelChoice>),
    Failed(String),
}

pub struct AppModel {
    pub paths: Paths,
    pub config: Config,
    pub config_issues: Vec<ConfigIssue>,
    pub styles: StyleLibrary,
    pub route: Route,
    pub services: Services,

    pub machine: Machine,
    pub live: Arc<LiveAudio>,
    /// Live preview text: committed words and the tentative tail.
    pub preview: (String, String),
    /// The text of the last dictation, for "paste last transcript".
    pub last_text: Option<String>,
    pub last_app: Option<String>,

    pub model_status: HashMap<ModelId, ModelStatus>,
    pub permissions: HashMap<Permission, PermissionState>,
    pub detected: Detected,

    pub recent: Vec<HistoryEntry>,
    pub stats: Stats,
    pub history_total: usize,
    pub words: Vec<Word>,
    pub replacements: Vec<Replacement>,
    pub(crate) replacer: Arc<Replacer>,

    /// The engine process reported a crash and has not come back yet.
    pub engine_down: Option<String>,
    /// A short message for the overlay when a hotkey is blocked (Secure Input).
    pub blocked_notice: Option<(String, Instant)>,
    /// Bumps when history changes, so pages can refresh their own queries.
    pub history_revision: u64,

    pub(crate) sessions: crate::dictation::Sessions,
    pub(crate) started: Instant,
    pub(crate) config_mtime: Option<std::time::SystemTime>,
    /// The macOS appearance last applied, for "Auto" theme mode.
    pub(crate) system_dark: bool,
    /// App icons for badges, cached on disk.
    pub icons: crate::icons::AppIcons,
    /// Model lists by provider id (or a draft key during onboarding).
    pub model_lists: HashMap<String, ModelList>,
    /// True when this Mac has Pindrop data to import.
    pub pindrop_found: bool,
    pub pindrop_import: crate::pindrop_import::PindropImport,
}

impl EventEmitter<AppEvent> for AppModel {}

#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Show or focus the Hub at a route.
    OpenHub(Route),
    /// The theme changed; windows re-read the palette.
    ThemeChanged,
}

impl AppModel {
    pub fn new(paths: Paths, config: Config, issues: Vec<ConfigIssue>, services: Services, cx: &mut Context<Self>) -> Self {
        let styles = StyleLibrary::load(&paths.styles_dir());
        let timing = Timing {
            min_duration_ms: config.dictation.min_duration_ms,
            max_duration_ms: config.dictation.max_duration_s * 1000,
            ..Timing::default()
        };
        let config_mtime = std::fs::metadata(paths.config_file()).and_then(|m| m.modified()).ok();
        let icons = crate::icons::AppIcons::new(paths.cache_dir.join("icons"), services.platform.context.clone());
        let mut model = AppModel {
            paths,
            config,
            config_issues: issues,
            styles,
            route: Route::Home,
            services,
            machine: Machine::new(timing),
            live: Arc::new(LiveAudio::default()),
            preview: (String::new(), String::new()),
            last_text: None,
            last_app: None,
            model_status: HashMap::new(),
            permissions: HashMap::new(),
            detected: Detected::default(),
            recent: Vec::new(),
            stats: Stats::default(),
            history_total: 0,
            words: Vec::new(),
            replacements: Vec::new(),
            replacer: Arc::new(Replacer::new(&[])),
            engine_down: None,
            blocked_notice: None,
            history_revision: 0,
            sessions: Default::default(),
            started: Instant::now(),
            config_mtime,
            system_dark: false,
            icons,
            model_lists: HashMap::new(),
            pindrop_found: sayso_store::pindrop::found(&crate::pindrop_import::pindrop_dir()),
            pindrop_import: Default::default(),
        };
        model.system_dark = model.services.platform.prefs.dark_mode();
        model.reload_dictionary();
        model.reload_history();
        model.refresh_permissions();
        // Local models only. The status of a cloud model comes from its provider.
        for m in sayso_core::models::catalog() {
            let status = model.services.engine.as_ref().map(|e| e.status(&m.id)).unwrap_or(ModelStatus::NotDownloaded);
            model.model_status.insert(m.id, status);
        }
        let hotkeys = model.services.platform.hotkeys.events();
        let engine_events = model.services.engine.as_ref().map(|e| e.subscribe());
        crate::dictation::start_background_loops(hotkeys, engine_events, cx);
        model.detect_providers(cx);
        model.load_active_models(cx);
        // Model names for the provider cards and style lines. The CLIs answer
        // without a prompt, so this costs no tokens.
        let ids: Vec<String> = model.config.ai.providers.iter().map(|p| p.id.clone()).collect();
        for id in ids {
            model.load_provider_models(&id, false, cx);
        }
        model
    }

    /// Milliseconds since the app started. The dictation machine's clock.
    pub fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        self.route = route;
        cx.notify();
    }

    pub fn open_hub(&mut self, route: Route, cx: &mut Context<Self>) {
        self.route = route;
        cx.emit(AppEvent::OpenHub(route));
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Config
    // -----------------------------------------------------------------------

    /// Change config, save it, and apply the change everywhere.
    pub fn edit_config(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Config)) {
        let before = self.config.clone();
        f(&mut self.config);
        if self.config == before {
            return;
        }
        if let Err(e) = self.config.save(&self.paths.config_file()) {
            log::error!("could not save config: {e}");
        }
        self.config_mtime = std::fs::metadata(self.paths.config_file()).and_then(|m| m.modified()).ok();
        self.config_issues = self.config.validate();
        self.apply_config_change(&before, cx);
        cx.notify();
    }

    /// Apply a config that changed (from the UI or from a file edit).
    pub(crate) fn apply_config_change(&mut self, before: &Config, cx: &mut Context<Self>) {
        if before.appearance != self.config.appearance {
            crate::app::apply_theme(&self.config, &self.services, cx);
            cx.emit(AppEvent::ThemeChanged);
        }
        if before.hotkeys != self.config.hotkeys {
            self.register_hotkeys();
        }
        if before.general.launch_at_login != self.config.general.launch_at_login
            && let Err(e) = self.services.platform.login_item.set_enabled(self.config.general.launch_at_login) {
                log::warn!("launch at login: {e}");
            }
        if before.dictation.model != self.config.dictation.model
            || before.dictation.language != self.config.dictation.language
            || before.dictation.live_preview != self.config.dictation.live_preview
        {
            // The language can change which model gives the live preview.
            self.load_active_models(cx);
        }
        self.machine.timing.min_duration_ms = self.config.dictation.min_duration_ms;
        self.machine.timing.max_duration_ms = self.config.dictation.max_duration_s * 1000;
    }

    pub fn config_path_display(&self) -> String {
        Paths::display(&self.paths.config_file(), &sayso_core::paths::PathEnv::home(&sayso_core::paths::SystemEnv))
    }

    pub fn reveal_config_file(&self) {
        let path = self.paths.config_file();
        if !path.exists() {
            let _ = self.config.save(&path);
        }
        crate::os::reveal(&path);
    }

    pub fn open_path(path: &std::path::Path) {
        crate::os::open_path(path);
    }

    /// Open a web page in the default browser.
    pub fn open_url(url: &str) {
        crate::os::open_url(url);
    }

    pub fn register_hotkeys(&mut self) {
        let h = &self.config.hotkeys;
        let bindings = sayso_platform::HotkeyBindings {
            toggle: h.toggle,
            push_to_talk: h.push_to_talk,
            paste_last: h.paste_last,
            cycle_style: h.cycle_style,
            single_escape: h.cancel == sayso_core::config::CancelMode::SingleEscape,
            double_escape_window: Duration::from_millis(h.double_escape_ms),
        };
        let reg = self.services.platform.hotkeys.register(&bindings);
        for (hk, why) in reg.failed {
            log::warn!("hotkey {hk} not registered: {why}");
        }
    }

    pub fn set_toggle_hotkey(&mut self, hotkey: Option<Hotkey>, cx: &mut Context<Self>) {
        self.edit_config(cx, |c| c.hotkeys.toggle = hotkey);
    }

    pub fn set_push_to_talk(&mut self, hotkey: Option<Hotkey>, cx: &mut Context<Self>) {
        if hotkey.is_some() && self.permission(Permission::InputMonitoring) != PermissionState::Granted {
            self.services.platform.permissions.request(Permission::InputMonitoring);
        }
        self.edit_config(cx, |c| c.hotkeys.push_to_talk = hotkey);
    }

    pub fn hotkey_conflicts(&self, hotkey: &Hotkey) -> Vec<HotkeyConflict> {
        self.services.platform.hotkeys.conflicts(hotkey)
    }

    // -----------------------------------------------------------------------
    // Permissions
    // -----------------------------------------------------------------------

    pub fn permission(&self, p: Permission) -> PermissionState {
        self.permissions.get(&p).copied().unwrap_or(PermissionState::NotDetermined)
    }

    pub fn refresh_permissions(&mut self) -> bool {
        let mut changed = false;
        for p in [Permission::Microphone, Permission::Accessibility, Permission::InputMonitoring] {
            let s = self.services.platform.permissions.status(p);
            if self.permissions.insert(p, s) != Some(s) {
                changed = true;
            }
        }
        changed
    }

    pub fn request_permission(&mut self, p: Permission) {
        self.services.platform.permissions.request(p);
    }

    pub fn open_permission_settings(&mut self, p: Permission) {
        self.services.platform.permissions.open_settings(p);
    }

    // -----------------------------------------------------------------------
    // Models
    // -----------------------------------------------------------------------

    /// Every model this Mac can use: the local models its macOS version runs,
    /// and the models of the configured speech providers.
    pub fn catalog(&self) -> Vec<ModelInfo> {
        let os = macos_major();
        sayso_core::models::catalog_with(&self.config.speech).into_iter().filter(|m| m.min_macos <= os).collect()
    }

    /// A cloud model is ready when its provider is set up. It has no download.
    pub fn status_of(&self, id: &ModelId) -> ModelStatus {
        if let Some((provider, _)) = split_model_id(id) {
            return if self.config.speech.provider(provider).is_some() { ModelStatus::Ready } else { ModelStatus::NotDownloaded };
        }
        self.model_status.get(id).cloned().unwrap_or(ModelStatus::NotDownloaded)
    }

    pub fn active_model(&self) -> ModelInfo {
        sayso_core::models::find_with(&self.config.speech, &self.config.dictation.model)
            .unwrap_or_else(|| sayso_core::models::find(&sayso_core::models::default_model()).expect("default model"))
    }

    /// The provider of the active model, when the active model is a cloud model.
    pub fn active_speech_provider(&self) -> Option<&SpeechProvider> {
        let (provider, _) = split_model_id(&self.config.dictation.model)?;
        self.config.speech.provider(provider)
    }

    /// The model for the live preview, when it is on and usable.
    ///
    /// The active model streams its own preview if it can. If not, a streaming
    /// model that is on disk and understands the dictation language does it:
    /// the small Parakeet EOU first, then any other.
    pub fn preview_model(&self) -> Option<ModelId> {
        if self.config.dictation.live_preview { self.preview_candidate() } else { None }
    }

    /// The model the live preview uses when the setting is on.
    pub fn preview_candidate(&self) -> Option<ModelId> {
        sayso_core::models::preview_for(&self.active_model(), &self.config.dictation.language, |id| self.status_of(id).is_on_disk())
    }

    pub fn download_model(&mut self, id: ModelId, cx: &mut Context<Self>) {
        let Some(engine) = self.services.engine.clone() else { return };
        if split_model_id(&id).is_some() {
            return; // A cloud model has no download.
        }
        self.model_status.insert(id.clone(), ModelStatus::Downloading { fraction: 0.0, bytes_done: 0, bytes_total: 0 });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let result = cx.background_spawn(async move { engine.download(&id2) }).await;
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = result {
                    m.model_status.insert(id.clone(), ModelStatus::Failed { message: e.to_string() });
                }
                if id == m.config.dictation.model || Some(&id) == m.preview_model().as_ref() {
                    m.load_active_models(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn delete_model(&mut self, id: ModelId, cx: &mut Context<Self>) {
        let Some(engine) = self.services.engine.clone() else { return };
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let result = cx.background_spawn(async move { engine.delete(&id2) }).await;
            let _ = this.update(cx, |m, cx| {
                match result {
                    Ok(()) => m.model_status.insert(id, ModelStatus::NotDownloaded),
                    Err(e) => m.model_status.insert(id, ModelStatus::Failed { message: e.to_string() }),
                };
                // The deleted model may have given the live preview.
                m.load_active_models(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_active_model(&mut self, id: ModelId, cx: &mut Context<Self>) {
        self.edit_config(cx, |c| c.dictation.model = id);
    }

    /// Load the final-pass and preview models when they are on disk. A cloud
    /// model needs no load.
    pub fn load_active_models(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.services.engine.clone() else { return };
        let mut ids = Vec::new();
        if !self.active_model().is_remote() {
            ids.push(self.config.dictation.model.clone());
        }
        if let Some(p) = self.preview_model()
            && !ids.contains(&p) {
                ids.push(p);
            }
        for id in ids {
            let status = self.status_of(&id);
            if !matches!(status, ModelStatus::Downloaded) {
                continue;
            }
            self.model_status.insert(id.clone(), ModelStatus::Optimizing);
            let engine = engine.clone();
            cx.spawn(async move |this, cx| {
                let id2 = id.clone();
                let result = cx.background_spawn(async move { engine.load(&id2) }).await;
                let _ = this.update(cx, |m, cx| {
                    let status = match result {
                        Ok(()) => ModelStatus::Ready,
                        Err(e) => ModelStatus::Failed { message: e.to_string() },
                    };
                    m.model_status.insert(id, status);
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Speech providers (cloud models)
    // -----------------------------------------------------------------------

    /// Add or replace a speech provider. The API key goes to the Keychain.
    pub fn save_speech_provider(&mut self, provider: SpeechProvider, api_key: Option<String>, cx: &mut Context<Self>) {
        if let (Some(key), Some(account)) = (&api_key, &provider.api_key_account)
            && let Err(e) = self.services.secrets.set(account, key) {
                log::error!("could not store API key: {e:?}");
            }
        self.edit_config(cx, move |c| {
            c.speech.providers.retain(|p| p.id != provider.id);
            c.speech.providers.push(provider);
        });
    }

    /// Remove a speech provider and its key. If its model was active, the
    /// default local model becomes active.
    pub fn remove_speech_provider(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        if let Some(account) = self.config.speech.provider(&id).and_then(|p| p.api_key_account.clone()) {
            let _ = self.services.secrets.delete(&account);
        }
        self.edit_config(cx, move |c| {
            c.speech.providers.retain(|p| p.id != id);
            if split_model_id(&c.dictation.model).is_some_and(|(p, _)| p == id) {
                c.dictation.model = sayso_core::models::default_model();
            }
        });
    }

    /// Check the address and the key of a speech provider. `done` gets the time in ms or a sentence for the user.
    pub fn test_speech_provider(&mut self, id: &str, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<u64, String>, &mut Context<Self>) + 'static) {
        let Some(provider) = self.config.speech.provider(id).cloned() else { return };
        let secrets = self.services.secrets.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let key = provider.api_key_account.as_deref().and_then(|a| secrets.get(a));
                    let started = Instant::now();
                    sayso_transcribe::check(&provider, key.as_deref(), Duration::from_secs(15))
                        .map(|()| started.elapsed().as_millis() as u64)
                        .map_err(|e| e.user_message(&provider.name))
                })
                .await;
            let _ = this.update(cx, |m, cx| done(m, result, cx));
        })
        .detach();
    }

    /// A unique speech provider id from a name.
    pub fn new_speech_provider_id(&self, name: &str) -> String {
        let base = crate::hub::pages::kit::slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.config.speech.provider(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Bytes used by downloaded models, for the Models header.
    pub fn models_disk_bytes(&self) -> u64 {
        self.catalog().iter().filter(|m| self.status_of(&m.id).is_on_disk()).map(|m| m.size_bytes).sum()
    }

    // -----------------------------------------------------------------------
    // Styles and AI
    // -----------------------------------------------------------------------

    pub fn active_style(&self) -> Style {
        self.styles
            .get(&self.config.ai.active_style)
            .or_else(|| self.styles.get(sayso_core::style::DEFAULT_STYLE_ID))
            .cloned()
            .expect("built-in styles exist")
    }

    pub fn set_active_style(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.edit_config(cx, |c| c.ai.active_style = id);
    }

    /// Move to the next style (the cycle-style hotkey).
    pub fn cycle_style(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.styles.styles().map(|s| s.id.clone()).collect();
        let i = ids.iter().position(|id| *id == self.config.ai.active_style).unwrap_or(0);
        let next = ids[(i + 1) % ids.len()].clone();
        self.set_active_style(&next, cx);
        let name = self.active_style().name;
        self.blocked_notice = Some((format!("Style: {name}"), Instant::now()));
    }

    pub fn save_style(&mut self, style: &Style, cx: &mut Context<Self>) -> Result<(), String> {
        sayso_core::style::save_style(&self.paths.styles_dir(), style).map_err(|e| e.to_string())?;
        self.styles = StyleLibrary::load(&self.paths.styles_dir());
        cx.notify();
        Ok(())
    }

    pub fn reset_style(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(e) = sayso_core::style::reset_style(&self.paths.styles_dir(), id) {
            log::warn!("reset style: {e}");
        }
        self.styles = StyleLibrary::load(&self.paths.styles_dir());
        cx.notify();
    }

    /// Delete a user style file. Built-in ids are reset instead.
    pub fn delete_style(&mut self, id: &str, cx: &mut Context<Self>) {
        self.reset_style(id, cx);
        if self.config.ai.active_style == id && self.styles.get(id).is_none() {
            self.set_active_style(sayso_core::style::DEFAULT_STYLE_ID, cx);
        }
    }

    /// The provider a style uses, if AI is on and the provider exists.
    pub fn provider_for(&self, style: &Style) -> Option<&Provider> {
        if !self.config.ai.enabled {
            return None;
        }
        self.config.ai.provider(style.provider.as_deref())
    }

    /// Add or replace a provider. The API key goes to the Keychain.
    pub fn save_provider(&mut self, provider: Provider, api_key: Option<String>, make_default: bool, cx: &mut Context<Self>) {
        if let (Some(key), ProviderKind::OpenAiCompatible { api_key_account: Some(account), .. }) = (&api_key, &provider.kind)
            && let Err(e) = self.services.secrets.set(account, key) {
                log::error!("could not store API key: {e:?}");
            }
        // The address or key may have changed, so the old list may be wrong.
        self.model_lists.remove(&provider.id);
        self.edit_config(cx, move |c| {
            c.ai.providers.retain(|p| p.id != provider.id);
            if make_default || c.ai.default_provider.is_none() {
                c.ai.default_provider = Some(provider.id.clone());
            }
            c.ai.providers.push(provider);
            c.ai.enabled = true;
        });
    }

    pub fn remove_provider(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.model_lists.remove(&id);
        if let Some(Provider { kind: ProviderKind::OpenAiCompatible { api_key_account: Some(acc), .. }, .. }) =
            self.config.ai.providers.iter().find(|p| p.id == id)
        {
            let _ = self.services.secrets.delete(acc);
        }
        self.edit_config(cx, move |c| {
            c.ai.providers.retain(|p| p.id != id);
            if c.ai.default_provider.as_deref() == Some(id.as_str()) {
                c.ai.default_provider = c.ai.providers.first().map(|p| p.id.clone());
            }
            if c.ai.providers.is_empty() {
                c.ai.enabled = false;
            }
        });
    }

    /// Fetch the model list of a saved provider. Without `force`, a list that
    /// is loading or loaded stays as it is.
    pub fn load_provider_models(&mut self, id: &str, force: bool, cx: &mut Context<Self>) {
        let Some(provider) = self.config.ai.providers.iter().find(|p| p.id == id).cloned() else { return };
        let secrets = self.services.secrets.clone();
        self.load_models(id.to_string(), provider, secrets, force, cx);
    }

    /// Fetch the model list of a provider that is not saved yet. The API key
    /// stays in memory, under the account the provider names.
    pub fn load_draft_models(&mut self, key: &str, provider: Provider, api_key: Option<String>, force: bool, cx: &mut Context<Self>) {
        let secrets = sayso_enhance::MemoryStore::new();
        if let (Some(k), ProviderKind::OpenAiCompatible { api_key_account: Some(account), .. }) = (&api_key, &provider.kind) {
            let _ = sayso_enhance::SecretStore::set(&secrets, account, k);
        }
        self.load_models(key.to_string(), provider, Arc::new(secrets), force, cx);
    }

    fn load_models(
        &mut self,
        key: String,
        provider: Provider,
        secrets: Arc<dyn sayso_enhance::SecretStore>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if !force && matches!(self.model_lists.get(&key), Some(ModelList::Loading | ModelList::Ready(_))) {
            return;
        }
        self.model_lists.insert(key.clone(), ModelList::Loading);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    sayso_enhance::list_provider_models(&provider, secrets.as_ref(), Duration::from_secs(10))
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let list = match result {
                    Ok(models) => ModelList::Ready(models),
                    Err(e) => ModelList::Failed(e.to_string()),
                };
                m.model_lists.insert(key, list);
                cx.notify();
            });
        })
        .detach();
    }

    /// Choose the model a provider uses when a style does not name one.
    pub fn set_provider_model(&mut self, id: &str, model: String, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.edit_config(cx, move |c| {
            if let Some(p) = c.ai.providers.iter_mut().find(|p| p.id == id) {
                p.set_model(model);
            }
        });
    }

    /// The provider's name for a model ("Haiku 4.5"), when its list is loaded.
    /// Otherwise the last part of the id.
    pub fn model_label(&self, provider_id: &str, model: &str) -> String {
        if let Some(ModelList::Ready(list)) = self.model_lists.get(provider_id)
            && let Some(m) = list.iter().find(|m| m.id == model)
        {
            return m.name.clone();
        }
        crate::hub::pages::ext::short_model(model)
    }

    pub fn set_ai_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.edit_config(cx, |c| c.ai.enabled = on);
    }

    /// Send one short request to a provider. `done` gets the time in ms or an error.
    pub fn test_provider(&mut self, id: &str, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<u64, String>, &mut Context<Self>) + 'static) {
        let Some(provider) = self.config.ai.providers.iter().find(|p| p.id == id).cloned() else { return };
        let ai = self.config.ai.clone();
        let secrets = self.services.secrets.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let enhancer = sayso_enhance::build_enhancer(&provider, secrets.as_ref(), &ai);
                    sayso_enhance::test_provider(enhancer.as_ref()).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |m, cx| done(m, result, cx));
        })
        .detach();
    }

    pub fn detect_providers(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    Detected {
                        claude_cli: sayso_enhance::detect_claude_cli(),
                        codex_cli: sayso_enhance::detect_codex_cli(),
                        ollama: sayso_enhance::detect_ollama(Duration::from_millis(600)),
                        done: true,
                    }
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.detected = found;
                cx.notify();
            });
        })
        .detach();
    }

    // -----------------------------------------------------------------------
    // Dictionary
    // -----------------------------------------------------------------------

    pub fn reload_dictionary(&mut self) {
        let Some(store) = &self.services.store else { return };
        self.words = store.list_words().unwrap_or_default();
        self.replacements = store.list_replacements().unwrap_or_default();
        self.replacer = Arc::new(Replacer::new(&self.replacements));
    }

    pub fn add_word(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(store) = &self.services.store
            && let Err(e) = store.add_word(text) {
                log::warn!("add word: {e}");
            }
        self.reload_dictionary();
        cx.notify();
    }

    pub fn remove_word(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(store) = &self.services.store {
            let _ = store.remove_word(id);
        }
        self.reload_dictionary();
        cx.notify();
    }

    pub fn add_replacement(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        if from.trim().is_empty() {
            return;
        }
        if let Some(store) = &self.services.store {
            let rule = Replacement { id: 0, from: from.trim().into(), to: to.into(), case_sensitive: false, uses: 0 };
            if let Err(e) = store.add_replacement(&rule) {
                log::warn!("add replacement: {e}");
            }
        }
        self.reload_dictionary();
        cx.notify();
    }

    pub fn update_replacement(&mut self, rule: &Replacement, cx: &mut Context<Self>) {
        if let Some(store) = &self.services.store {
            let _ = store.update_replacement(rule);
        }
        self.reload_dictionary();
        cx.notify();
    }

    pub fn remove_replacement(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(store) = &self.services.store {
            let _ = store.remove_replacement(id);
        }
        self.reload_dictionary();
        cx.notify();
    }

    /// What the dictionary does to a phrase ("Try a phrase").
    pub fn try_phrase(&self, text: &str) -> String {
        self.replacer.apply(text).0
    }

    pub fn vocabulary(&self) -> Vec<String> {
        self.words.iter().map(|w| w.text.clone()).collect()
    }

    // -----------------------------------------------------------------------
    // History
    // -----------------------------------------------------------------------

    pub fn reload_history(&mut self) {
        let Some(store) = &self.services.store else { return };
        let q = sayso_store::HistoryQuery { limit: Some(50), ..Default::default() };
        self.recent = store.list(&q).unwrap_or_default();
        self.history_total = store.count(&sayso_store::HistoryQuery::default()).unwrap_or(0);
        let since = chrono::Utc::now() - chrono::Duration::days(8);
        let entries = store.entries_since(since).unwrap_or_default();
        self.stats = stats::compute(&entries, chrono::Local::now());
        self.history_revision += 1;
    }

    pub fn query_history(&self, q: &sayso_store::HistoryQuery) -> Vec<HistoryEntry> {
        self.services.store.as_ref().and_then(|s| s.list(q).ok()).unwrap_or_default()
    }

    pub fn count_history(&self, q: &sayso_store::HistoryQuery) -> usize {
        self.services.store.as_ref().and_then(|s| s.count(q).ok()).unwrap_or(0)
    }

    pub fn history_apps(&self) -> Vec<sayso_store::AppCount> {
        self.services.store.as_ref().and_then(|s| s.app_list().ok()).unwrap_or_default()
    }

    pub fn delete_entry(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(store) = &self.services.store {
            let _ = store.delete(id);
        }
        self.reload_history();
        cx.notify();
    }

    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.services.store {
            let _ = store.clear_all();
        }
        self.reload_history();
        cx.notify();
    }

    pub fn load_audio(&self, file: &str) -> Option<Vec<f32>> {
        self.services.store.as_ref().and_then(|s| s.load_audio(file).ok())
    }

    pub fn copy_text(&self, text: &str) {
        self.services.platform.inserter.copy_to_clipboard(text);
    }

    /// When the audio of an entry expires, for the History note.
    pub fn audio_expiry(&self, entry: &HistoryEntry) -> Option<chrono::DateTime<chrono::Local>> {
        let days = self.config.history.keep_audio_days?;
        entry.audio_file.as_ref()?;
        Some((entry.created_at + chrono::Duration::days(days as i64)).with_timezone(&chrono::Local))
    }

    // -----------------------------------------------------------------------
    // Dictation entry points (implementation in `dictation.rs`)
    // -----------------------------------------------------------------------

    pub fn state(&self) -> &State {
        &self.machine.state
    }

    pub fn toggle_dictation(&mut self, cx: &mut Context<Self>) {
        self.dispatch(Event::Toggle, cx);
    }

    pub fn undo_cancel(&mut self, cx: &mut Context<Self>) {
        self.dispatch(Event::Undo, cx);
    }

    pub fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        self.dispatch(Event::Dismiss, cx);
    }

    pub fn input_devices(&self) -> Vec<AudioDevice> {
        self.services.platform.audio.devices()
    }

    /// One-line engine status for the sidebar: (title, detail, color).
    pub fn engine_summary(&self) -> (String, String, fn(&sayso_ui::Colors) -> Hsla) {
        let model = self.active_model();
        if model.is_remote() {
            // The engine process plays no part in a cloud final pass.
            return match self.status_of(&model.id) {
                ModelStatus::Ready => ("Ready".into(), format!("{} · Cloud", model.name), |c| c.success),
                _ => ("No model".into(), "Open Models to choose one".into(), |c| c.danger),
            };
        }
        if self.services.engine.is_none() || self.engine_down.is_some() {
            return ("Engine stopped".into(), "Restarting…".into(), |c| c.danger);
        }
        match self.status_of(&model.id) {
            ModelStatus::Ready => ("Ready".into(), model.name, |c| c.success),
            ModelStatus::Optimizing => ("Optimizing".into(), model.name, |c| c.accent),
            ModelStatus::Downloading { fraction, .. } => {
                ("Downloading".into(), format!("{} · {:.0}%", model.name, fraction * 100.0), |c| c.accent)
            }
            ModelStatus::Downloaded => ("Loading".into(), model.name, |c| c.accent),
            ModelStatus::NotDownloaded => ("No model".into(), "Open Models to download".into(), |c| c.danger),
            ModelStatus::Failed { .. } => ("Model error".into(), model.name, |c| c.danger),
        }
    }

    /// True when the active model can run a final pass: a cloud model with its
    /// provider set up, or a local model on disk with the engine running.
    pub fn model_ready(&self) -> bool {
        let model = self.active_model();
        let status = self.status_of(&model.id);
        if model.is_remote() { status.is_usable() } else { self.services.engine.is_some() && status.is_on_disk() }
    }

    /// True when the app can dictate now.
    pub fn ready_to_dictate(&self) -> bool {
        self.model_ready() && self.permission(Permission::Microphone) == PermissionState::Granted
    }

    // -----------------------------------------------------------------------
    // Onboarding
    // -----------------------------------------------------------------------

    pub fn onboarding_step(&self) -> u8 {
        self.config.onboarding.step
    }

    pub fn set_onboarding_step(&mut self, step: u8, cx: &mut Context<Self>) {
        self.edit_config(cx, |c| c.onboarding.step = step.min(6));
    }

    pub fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.edit_config(cx, |c| {
            c.onboarding.completed = true;
            c.onboarding.step = 6;
        });
    }
}

/// The major version of macOS ("15" from "15.6.1"). 14, the oldest macOS Sayso
/// runs on, when the version cannot be read.
pub fn macos_major() -> u32 {
    if !cfg!(target_os = "macos") {
        // `min_macos` means nothing elsewhere, and no catalog model there needs more than 14.
        return 14;
    }
    static VERSION: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VERSION.get_or_init(|| {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|v| v.trim().split('.').next().and_then(|major| major.parse().ok()))
            .unwrap_or(14)
    })
}
