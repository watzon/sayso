//! `AppModel`: the single source of state for every window.
//!
//! Views read it and call its methods. Blocking work (engine, store, AI,
//! insertion) runs on the background executor; results come back through
//! `this.update(...)`.

use crate::hub::Route;
use crate::live::LiveAudio;
use crate::services::Services;
use gpui_kit::*;
use sayso_i18n::t;
use sayso_core::config::{Config, ConfigIssue, PreviewChoice, Provider, ProviderKind, RoleChoice};
use sayso_core::enhance::ModelChoice;
use sayso_core::dictation::{Event, Machine, State, Timing};
use sayso_core::dictionary::{Replacement, Replacer, Word};
use sayso_core::history::HistoryEntry;
use sayso_core::hotkey::Hotkey;
use sayso_core::models::{ModelId, ModelInfo, ModelTask, Vad};
use sayso_core::speech::{SpeechProvider, split_model_id};
use sayso_core::paths::Paths;
use sayso_core::stats::{self, Stats};
use sayso_core::stt::ModelStatus;
use sayso_core::style::{Style, StyleLibrary};
use sayso_platform::{HotkeyConflict, PartReadiness, Permission, PermissionState};
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
    /// The name of the model when Apple Intelligence can run on this Mac.
    pub apple_intelligence: Option<String>,
    /// Why Apple Intelligence cannot run, when the engine runs but the model does not.
    pub apple_unavailable: Option<sayso_core::enhance::Unavailable>,
    pub done: bool,
}

/// The login of a CLI provider, as last read from the tool.
#[derive(Clone, Default)]
pub struct ProviderLogin {
    /// None until the first check ends.
    pub status: Option<sayso_enhance::AuthStatus>,
    pub checking: bool,
    /// The sign-in that runs now.
    pub sign_in: Option<Arc<sayso_enhance::SignIn>>,
    /// The sign-in page that the tool printed.
    pub url: Option<String>,
    /// Why the last sign-in did not work.
    pub error: Option<LoginError>,
}

/// Why a sign-in did not work. It keeps the kind and the text of the tool,
/// so the banner changes with the interface language.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginError {
    /// The tool could not start the sign-in. The error of the tool.
    Start(String),
    /// The sign-in ended, but the tool is not signed in.
    NotFinished,
    /// The tool did not take the code. The error of the tool.
    Code(String),
}

impl LoginError {
    pub fn text(&self) -> String {
        match self {
            LoginError::Start(error) => t!("app.sign_in.start_failed", error = error),
            LoginError::NotFinished => t!("app.sign_in.not_finished"),
            LoginError::Code(error) => t!("app.sign_in.code_failed", error = error),
        }
    }
}

/// A provider's models, as last fetched from the provider.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelList {
    Loading,
    Ready(Vec<ModelChoice>),
    /// The English error, and the reason code of the engine when it sent one.
    Failed(String, Option<sayso_core::stt::EngineReason>),
}

pub struct AppModel {
    pub paths: Paths,
    pub config: Config,
    pub config_issues: Vec<ConfigIssue>,
    pub styles: StyleLibrary,
    pub route: Route,
    pub services: Services,
    /// The downloads of cleanup models. See `cleanup_models.rs`.
    pub cleanup: crate::cleanup_models::CleanupModels,

    pub machine: Machine,
    pub live: Arc<LiveAudio>,
    /// Live preview text: committed words and the tentative tail.
    pub preview: (String, String),
    /// The text of the last dictation, for "paste last transcript".
    pub last_text: Option<String>,
    pub last_app: Option<String>,

    pub model_status: HashMap<ModelId, ModelStatus>,
    /// The active model is not in memory: its idle time ended, or the load
    /// at the start is off and no dictation ran yet. The next dictation loads it.
    pub model_idle: bool,
    /// When a dictation last ran or the active model loaded. The idle time
    /// counts from here.
    pub(crate) last_dictation: Instant,
    pub permissions: HashMap<Permission, PermissionState>,
    /// A permission check runs on the background executor.
    permissions_checking: bool,
    pub detected: Detected,

    pub recent: Vec<HistoryEntry>,
    pub stats: Stats,
    pub history_total: usize,
    /// What the store holds on disk, for Settings › History and privacy.
    /// That page asks for it with [`AppModel::refresh_storage`].
    pub storage: sayso_store::StorageUse,
    pub words: Vec<Word>,
    pub replacements: Vec<Replacement>,
    pub(crate) replacer: Arc<Replacer>,

    /// The engine process reported a crash and has not come back yet.
    pub engine_down: Option<String>,
    /// The engine runs but has not answered yet. Until it does, the status
    /// of the local models is not known.
    pub engine_starting: bool,
    /// A short message in the idle overlay, for example when a hotkey cannot
    /// start a dictation.
    pub blocked_notice: Option<OverlayNotice>,
    /// Incognito: Sayso saves no history entry while it is on. It is never
    /// written to disk, so it is off after each start.
    pub incognito: bool,
    /// Bumps when history changes, so pages can refresh their own queries.
    pub history_revision: u64,
    /// The newest history reload. An older reload that ends later is dropped.
    history_reload: u64,

    pub(crate) sessions: crate::dictation::Sessions,
    /// Pauses the media of other apps while a dictation records.
    pub(crate) media: crate::media::MediaPause,
    pub(crate) started: Instant,
    pub(crate) config_mtime: Option<std::time::SystemTime>,
    /// The styles folder when `styles` was last checked. A change reloads the styles.
    pub(crate) styles_fingerprint: crate::style_files::Fingerprint,
    /// The macOS appearance last applied, for "Auto" theme mode.
    pub(crate) system_dark: bool,
    /// App icons for badges, cached on disk.
    pub icons: crate::icons::AppIcons,
    /// Site icons for badges, cached on disk.
    site_icons: crate::site_icons::SiteIcons,
    /// Model lists by provider id (or a draft key during onboarding).
    pub model_lists: HashMap<String, ModelList>,
    /// The login of each CLI provider, by provider id.
    pub logins: HashMap<String, ProviderLogin>,
    /// True when this Mac has Pindrop data to import.
    pub pindrop_found: bool,
    pub pindrop_import: crate::pindrop_import::PindropImport,
    /// What the UI shows about updates.
    pub update: sayso_update::Status,
    /// None when this build does not look for updates (a build from source).
    pub(crate) updater: Option<sayso_update::Updater>,
    /// The version of the last update, until the user saw "Sayso is now X".
    pub update_notice: Option<sayso_update::manifest::Version>,
    /// The input devices, scanned off the UI thread. See `devices.rs`.
    pub(crate) devices: crate::devices::InputDevices,
    /// An extension is installed, so the Models page shows the VAD and
    /// diarization models. The slow checks read the folder again.
    pub extensions_installed: bool,
}

impl EventEmitter<AppEvent> for AppModel {}

/// A short message in the idle overlay. It goes away after a few seconds.
#[derive(Debug, Clone)]
pub struct OverlayNotice {
    pub icon: NoticeIcon,
    pub title: String,
    /// A second line under the title.
    pub detail: Option<String>,
    pub shown: Instant,
}

impl OverlayNotice {
    pub fn new(icon: NoticeIcon, title: impl Into<String>) -> Self {
        Self { icon, title: title.into(), detail: None, shown: Instant::now() }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The icon of an overlay notice. The producer of the notice sets it, so it
/// does not depend on the words of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeIcon {
    Info,
    Microphone,
    Style,
    Incognito,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Show or focus the Hub at a route.
    OpenHub(Route),
    /// The theme changed; windows re-read the palette.
    ThemeChanged,
    /// The interface language changed; windows draw their text again.
    LanguageChanged,
}

impl AppModel {
    pub fn new(paths: Paths, config: Config, issues: Vec<ConfigIssue>, services: Services, cx: &mut Context<Self>) -> Self {
        let styles_fingerprint = crate::style_files::Fingerprint::of(&paths.styles_dir());
        let styles = StyleLibrary::load(&paths.styles_dir());
        let timing = Timing {
            min_duration_ms: config.dictation.min_duration_ms,
            max_duration_ms: config.dictation.max_duration_s * 1000,
            ..Timing::default()
        };
        let config_mtime = std::fs::metadata(paths.config_file()).and_then(|m| m.modified()).ok();
        let icons = crate::icons::AppIcons::new(paths.cache_dir.join("icons"), services.platform.context.clone());
        let media = crate::media::MediaPause::new(services.platform.media.clone());
        let (site_icons, fetched_icons) = crate::site_icons::SiteIcons::new(paths.cache_dir.join("site-icons"));
        cx.spawn(async move |this, cx| {
            while let Ok((host, png)) = fetched_icons.recv().await {
                let shown = this.update(cx, |m, cx| {
                    m.site_icons.fetched(&host, png);
                    cx.notify();
                });
                if shown.is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut model = AppModel {
            paths,
            config,
            config_issues: issues,
            styles,
            route: Route::Home,
            services,
            machine: Machine::new(timing),
            cleanup: Default::default(),
            live: Arc::new(LiveAudio::default()),
            preview: (String::new(), String::new()),
            last_text: None,
            last_app: None,
            model_status: HashMap::new(),
            model_idle: false,
            last_dictation: Instant::now(),
            permissions: HashMap::new(),
            permissions_checking: false,
            detected: Detected::default(),
            recent: Vec::new(),
            stats: Stats::default(),
            history_total: 0,
            storage: sayso_store::StorageUse::default(),
            words: Vec::new(),
            replacements: Vec::new(),
            replacer: Arc::new(Replacer::new(&[])),
            engine_down: None,
            engine_starting: false,
            blocked_notice: None,
            incognito: false,
            history_revision: 0,
            history_reload: 0,
            sessions: Default::default(),
            media,
            started: Instant::now(),
            config_mtime,
            styles_fingerprint,
            system_dark: false,
            icons,
            site_icons,
            model_lists: HashMap::new(),
            logins: HashMap::new(),
            pindrop_found: sayso_store::pindrop::found(&crate::pindrop_import::pindrop_dir()),
            pindrop_import: Default::default(),
            update: sayso_update::Status::Idle { last_check: None },
            updater: None,
            update_notice: None,
            devices: Default::default(),
            extensions_installed: false,
        };
        model.system_dark = model.services.platform.prefs.dark_mode();
        model.extensions_installed = sayso_core::extensions::any_installed(&model.paths.extensions_dir());
        model.reload_dictionary();
        model.reload_history(cx);
        model.refresh_permissions(cx);
        model.refresh_input_devices(cx);
        let engine_events = model.services.engine_events.take();
        model.engine_starting = engine_events.is_some();
        // Local models only: speech, VAD, and diarization models. The status
        // of a cloud model comes from its provider.
        for m in sayso_core::models::local_models() {
            let status = model.services.engine.as_ref().map(|e| e.status(&m.id)).unwrap_or(ModelStatus::NotDownloaded);
            model.model_status.insert(m.id.clone(), status);
        }
        let hotkeys = model.services.platform.hotkeys.events();
        crate::dictation::start_background_loops(hotkeys, engine_events, cx);
        model.start_updater(cx);
        model.detect_providers(cx);
        // Without the load at the start, the active model is out of memory as
        // after its idle time: the first dictation loads it.
        model.model_idle = !model.config.dictation.preload_model;
        model.load_active_models(cx);
        // Model names for the provider cards and style lines. A CLI provider
        // starts its tool for the list, so its list loads on first need: when
        // the Hub opens (the Providers page checks the sign-in, then loads it)
        // or a model picker opens. Until then a label is the short id.
        let ids: Vec<String> = model
            .config
            .ai
            .providers
            .iter()
            .filter(|p| !matches!(p.kind, ProviderKind::ClaudeCli { .. } | ProviderKind::CodexCli { .. }))
            .map(|p| p.id.clone())
            .collect();
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
    // Updates
    // -----------------------------------------------------------------------

    /// Start the updater thread, when this build looks for updates.
    fn start_updater(&mut self, cx: &mut Context<Self>) {
        use sayso_update::{AtStart, Status};
        let Some((options, found)) = self.services.updates.take() else { return };
        let state = sayso_update::state::UpdateState::load(&options.context.state_path);
        self.update_notice = state.notice;
        self.update = match &found {
            AtStart::Ready(version) => Status::Ready { version: version.clone() },
            AtStart::Failed(failure) => Status::Failed { failure: failure.clone() },
            AtStart::Nothing | AtStart::Updated(_) | AtStart::Relaunching | AtStart::Stale => Status::Idle { last_check: state.last_check },
        };
        let (tx, statuses) = async_channel::unbounded();
        self.updater = Some(sayso_update::Updater::spawn(options, move |status| {
            let _ = tx.send_blocking(status);
        }));
        cx.spawn(async move |this, cx| {
            while let Ok(status) = statuses.recv().await {
                let updated = this.update(cx, |m, cx| {
                    m.update = status;
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        if matches!(found, AtStart::Updated(_)) {
            self.confirm_update_when_running(cx);
        }
    }

    /// This is the first start after an update. Keep the old version until
    /// this one ran for 30 seconds with a working engine, then delete it.
    fn confirm_update_when_running(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(30)).await;
                let done = this.update(cx, |m, cx| {
                    if m.services.engine.is_none() || m.engine_down.is_some() || m.engine_starting {
                        return false;
                    }
                    // Not yet, when another Sayso uses this install.
                    let Some(updater) = m.updater.as_ref().filter(|u| u.confirm()) else { return false };
                    m.update_notice = updater.notice();
                    cx.notify();
                    true
                });
                if !matches!(done, Ok(false)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// True when this build looks for updates.
    pub fn updates_enabled(&self) -> bool {
        self.updater.is_some()
    }

    /// "Check for updates" and "Try again" in Settings.
    pub fn check_for_updates(&self) {
        if let Some(updater) = &self.updater {
            updater.check_now();
        }
    }

    /// "Update now": download the new version and prepare it.
    pub fn install_update(&self) {
        if let Some(updater) = &self.updater {
            updater.install();
        }
    }

    pub fn cancel_update(&self) {
        if let Some(updater) = &self.updater {
            updater.cancel();
        }
    }

    /// "Restart to update": put the new version in place and start it. Does
    /// nothing during a dictation. The button is off then.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        if !self.can_restart_to_update() {
            return;
        }
        let Some(updater) = &self.updater else { return };
        match updater.swap() {
            // The installed app is the new version now. Quit, and it starts.
            Ok(()) => cx.quit(),
            Err(sayso_update::SwapError::Busy) => {
                self.update = sayso_update::Status::Failed { failure: sayso_update::Failure::Busy };
            }
            Err(sayso_update::SwapError::Failed(failure)) => self.update = sayso_update::Status::Failed { failure },
        }
        cx.notify();
    }

    /// False during a dictation: a restart would lose it.
    pub fn can_restart_to_update(&self) -> bool {
        matches!(self.machine.state, State::Idle)
    }

    /// The user saw "Sayso is now X".
    pub fn dismiss_update_notice(&mut self, cx: &mut Context<Self>) {
        if let Some(updater) = &self.updater {
            updater.dismiss_notice();
        }
        self.update_notice = None;
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
        // No cleanup model can run any more: free the memory of the runtime
        // now, not after its idle time. Off the UI thread, because a cleanup
        // that runs now holds the runtime until it ends.
        let cleanup_in_use = |c: &Config| c.ai.enabled && c.ai.providers.iter().any(|p| matches!(p.kind, ProviderKind::CleanupModel { .. }));
        if cleanup_in_use(before) != cleanup_in_use(&self.config) {
            let on = cleanup_in_use(&self.config);
            self.services.cleanup_runtime.set_enabled(on);
            if !on {
                let runtime = self.services.cleanup_runtime.clone();
                cx.background_spawn(async move { runtime.stop() }).detach();
            }
        }
        if before.appearance != self.config.appearance {
            crate::app::apply_theme(&self.config, &self.services, cx);
            cx.emit(AppEvent::ThemeChanged);
        }
        if before.general.language != self.config.general.language {
            crate::app::apply_language(&self.config);
            cx.emit(AppEvent::LanguageChanged);
        }
        if before.hotkeys != self.config.hotkeys {
            self.register_hotkeys();
        }
        if !self.config.history.enabled {
            // Nothing is saved while history is off, so incognito has no meaning.
            self.incognito = false;
        }
        if before.updates.check != self.config.updates.check
            && let Some(updater) = &self.updater
        {
            updater.set_auto_check(self.config.updates.check);
        }
        if before.updates.automatic != self.config.updates.automatic
            && let Some(updater) = &self.updater
        {
            updater.set_automatic(self.config.updates.automatic);
        }
        if before.general.launch_at_login != self.config.general.launch_at_login
            && let Err(e) = self.services.platform.login_item.set_enabled(self.config.general.launch_at_login) {
                log::warn!("launch at login: {e}");
            }
        let keep_changed = before.dictation.keep_model_minutes != self.config.dictation.keep_model_minutes
            // The user turned on the load at the start: the model loads now.
            || (self.config.dictation.preload_model && !before.dictation.preload_model);
        if before.dictation.model != self.config.dictation.model || keep_changed {
            // A new model or a new idle time starts with the model in memory.
            self.model_idle = false;
            self.last_dictation = Instant::now();
        }
        if before.dictation.model != self.config.dictation.model
            || before.dictation.language != self.config.dictation.language
            || before.dictation.live_preview != self.config.dictation.live_preview
            || before.dictation.preview_model != self.config.dictation.preview_model
            || keep_changed
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
        crate::shell::reveal(&path);
    }

    pub fn open_path(path: &std::path::Path) {
        crate::shell::open(path);
    }

    /// Open a web page in the default browser.
    pub fn open_url(url: &str) {
        crate::shell::open(url);
    }

    pub fn register_hotkeys(&mut self) {
        let h = &self.config.hotkeys;
        // Config files can bypass the recorder's conflict checks.
        let usable = |hotkey: Option<Hotkey>| hotkey.filter(|key| h.toggle.is_none_or(|toggle| !key.conflicts_with_toggle(toggle)));
        let bindings = sayso_platform::HotkeyBindings {
            toggle: h.toggle,
            push_to_talk: usable(h.push_to_talk),
            paste_last: usable(h.paste_last),
            cycle_style: usable(h.cycle_style),
            incognito: usable(h.incognito),
            enter_sends: h.enter_sends_usable(),
            take_escape: h.take_escape_usable(),
            single_escape: h.cancel == sayso_core::config::CancelMode::SingleEscape,
            double_escape_window: Duration::from_millis(h.double_escape_ms),
        };
        let reg = self.services.platform.hotkeys.register(&bindings);
        for (hk, why) in reg.failed {
            log::warn!("hotkey {hk} not registered: {why}");
        }
    }

    pub fn set_toggle_hotkey(&mut self, hotkey: Option<Hotkey>, cx: &mut Context<Self>) {
        if hotkey.and_then(Hotkey::modifier_keys).is_some() {
            self.request_hotkey_access();
        }
        self.edit_config(cx, |c| c.hotkeys.toggle = hotkey);
    }

    pub fn set_push_to_talk(&mut self, hotkey: Option<Hotkey>, cx: &mut Context<Self>) {
        if hotkey.is_some() {
            self.request_hotkey_access();
        }
        self.edit_config(cx, |c| c.hotkeys.push_to_talk = hotkey);
    }

    pub(crate) fn request_hotkey_access(&self) {
        if self.permission(Permission::InputMonitoring) != PermissionState::Granted {
            self.services.platform.permissions.request(Permission::InputMonitoring);
        }
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

    /// Read the permissions again on the background executor. On macOS each
    /// read is a call to the privacy service of the system.
    pub fn refresh_permissions(&mut self, cx: &mut Context<Self>) {
        if self.permissions_checking {
            return;
        }
        self.permissions_checking = true;
        let guide = self.services.platform.permissions.clone();
        cx.spawn(async move |this, cx| {
            let states = cx
                .background_spawn(async move {
                    [Permission::Microphone, Permission::Accessibility, Permission::InputMonitoring].map(|p| (p, guide.status(p)))
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.permissions_checking = false;
                let mut changed = false;
                for (p, s) in states {
                    changed |= m.permissions.insert(p, s) != Some(s);
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The microphone permission as the system says it now. A dictation
    /// that the cached state would block asks here first: the user may have
    /// just allowed the microphone.
    pub(crate) fn microphone_permission_now(&mut self) -> PermissionState {
        let state = self.permission(Permission::Microphone);
        if state == PermissionState::Granted {
            return state;
        }
        let state = self.services.platform.permissions.status(Permission::Microphone);
        self.permissions.insert(Permission::Microphone, state);
        state
    }

    /// True when a window shows the permissions: onboarding, or the Hub on
    /// Home or on a Settings page with a permission on it.
    pub(crate) fn permissions_shown(&self, cx: &App) -> bool {
        use crate::hub::SettingsPage;
        let Some(windows) = cx.try_global::<crate::app::Windows>() else { return false };
        let page = matches!(
            self.route,
            Route::Home | Route::Settings(SettingsPage::Permissions | SettingsPage::Audio | SettingsPage::Dictation)
        );
        windows.onboarding.is_some() || (windows.hub.is_some() && page)
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

    /// Every model this computer can use: the local models its system runs,
    /// and the models of the configured speech providers.
    pub fn catalog(&self) -> Vec<ModelInfo> {
        sayso_core::models::catalog_with(&self.config.speech).into_iter().filter(crate::shell::model_runs_here).collect()
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

    /// The model for the live preview, when it is on and usable
    /// (`models::preview_for`).
    pub fn preview_model(&self) -> Option<ModelId> {
        if self.config.dictation.live_preview { self.preview_candidate() } else { None }
    }

    /// The model the live preview uses when the setting is on.
    pub fn preview_candidate(&self) -> Option<ModelId> {
        let d = &self.config.dictation;
        sayso_core::models::preview_for(&self.active_model(), &d.preview_model, &d.language, |id| self.status_of(id).is_on_disk())
    }

    /// The chosen preview model does not understand the dictation language.
    pub fn preview_language_mismatch(&self) -> bool {
        let d = &self.config.dictation;
        d.live_preview && sayso_core::models::preview_language_mismatch(&self.active_model(), &d.preview_model, &d.language)
    }

    /// Choose the preview model, or turn the preview off with `None`. A chosen
    /// model that is not on disk downloads.
    pub fn set_preview(&mut self, choice: Option<PreviewChoice>, cx: &mut Context<Self>) {
        if let Some(PreviewChoice::Model(id)) = &choice
            && matches!(self.status_of(id), ModelStatus::NotDownloaded | ModelStatus::Failed { .. })
        {
            self.download_model(id.clone(), cx);
        }
        self.edit_config(cx, |c| match choice {
            Some(choice) => {
                c.dictation.live_preview = true;
                c.dictation.preview_model = choice;
            }
            None => c.dictation.live_preview = false,
        });
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
                    m.model_status.insert(id.clone(), ModelStatus::Failed { message: e.to_string(), reason: None });
                }
                if id == m.config.dictation.model || Some(&id) == m.preview_model().as_ref() {
                    m.load_active_models(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The engine cannot read the files of a model that an older engine left: the
    /// Parakeet files of the Windows engine before 0.12 (`EngineReason::FormatChanged`).
    /// A model in use downloads again. The files of any other model go, so the model
    /// shows as not downloaded.
    pub fn replace_old_model_files(&mut self, id: ModelId, cx: &mut Context<Self>) {
        if id == self.config.dictation.model || Some(&id) == self.preview_model().as_ref() {
            log::info!("{id}: the files are in an old format; downloading the model again");
            self.download_model(id, cx);
        } else {
            log::info!("{id}: the files are in an old format; deleting them");
            self.delete_model(id, cx);
        }
    }

    pub fn delete_model(&mut self, id: ModelId, cx: &mut Context<Self>) {
        let Some(engine) = self.services.engine.clone() else { return };
        cx.spawn(async move |this, cx| {
            let id2 = id.clone();
            let result = cx.background_spawn(async move { engine.delete(&id2) }).await;
            let _ = this.update(cx, |m, cx| {
                match result {
                    Ok(()) => m.model_status.insert(id, ModelStatus::NotDownloaded),
                    Err(e) => m.model_status.insert(id, ModelStatus::Failed { message: e.to_string(), reason: None }),
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

    /// The active model, when it is a local model.
    fn active_local_model(&self) -> Option<ModelId> {
        (!self.active_model().is_remote()).then(|| self.config.dictation.model.clone())
    }

    /// The models that must be in engine memory now.
    fn wanted_models(&self) -> Vec<ModelId> {
        crate::memory::wanted(self.active_local_model().as_ref(), self.preview_model().as_ref(), self.model_idle)
    }

    /// Prepare the optional parts of the ready models in use.
    pub(crate) fn prepare_active_models(&self) {
        let Some(engine) = &self.services.engine else { return };
        let preview = self.preview_model();
        for id in self.wanted_models() {
            if !self.status_of(&id).is_usable() { continue; }
            let words = self.vocabulary();
            let (preview, vocabulary) = crate::memory::preparation(&id, self.active_local_model().as_ref(), preview.as_ref(), &words);
            if let Err(e) = engine.prepare(&id, preview, vocabulary) {
                log::warn!("prepare model {id}: {e}");
            }
        }
    }

    /// The live preview waits for its model, or for the part of the model
    /// that streams. The overlay then shows "Loading the live preview".
    pub(crate) fn preview_loading(&self) -> bool {
        let Some(preview) = self.preview_model() else { return false };
        matches!(self.status_of(&preview), ModelStatus::Optimizing)
            || self.services.engine.as_ref().is_some_and(|e| e.part_readiness(&preview, true) == PartReadiness::Preparing)
    }

    /// The final pass waits for the helper model of the dictionary words.
    pub(crate) fn dictionary_loading(&self) -> bool {
        let Some(model) = self.active_local_model() else { return false };
        self.services.engine.as_ref().is_some_and(|e| e.part_readiness(&model, false) == PartReadiness::Preparing)
    }

    /// Load the final-pass and preview models when they are on disk. A cloud
    /// model needs no load. A model that the idle time unloaded stays out.
    pub fn load_active_models(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.services.engine.clone() else { return };
        for id in self.wanted_models() {
            let status = self.status_of(&id);
            if !matches!(status, ModelStatus::Downloaded) {
                continue;
            }
            self.model_status.insert(id.clone(), ModelStatus::Optimizing);
            let engine = engine.clone();
            cx.spawn(async move |this, cx| {
                let id2 = id.clone();
                // After a failed load, the engine status holds the reason code from `model_state`.
                let (result, after) = cx.background_spawn(async move { (engine.load(&id2), engine.status(&id2)) }).await;
                let _ = this.update(cx, |m, cx| {
                    let status = match result {
                        Ok(()) => ModelStatus::Ready,
                        Err(e) => ModelStatus::Failed {
                            message: e.to_string(),
                            reason: match after {
                                ModelStatus::Failed { reason, .. } => reason,
                                _ => None,
                            },
                        },
                    };
                    if id == m.config.dictation.model {
                        // A final pass that waited for this load must find the model.
                        m.last_dictation = Instant::now();
                    }
                    m.model_status.insert(id.clone(), status);
                    m.prepare_active_models();
                    m.start_late_preview(&id, cx);
                    cx.notify();
                });
            })
            .detach();
        }
        self.prepare_active_models();
        cx.notify();
    }

    /// Unload the local models that are not in use: a model that is not the
    /// active model or the live-preview model, and the active model when its
    /// idle time ended. Runs with the slow checks, and never during a dictation.
    pub(crate) fn unload_unused_models(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.services.engine.clone() else { return };
        if !matches!(self.machine.state, State::Idle) {
            self.last_dictation = Instant::now();
            return;
        }
        let keep = self.config.dictation.keep_model_minutes;
        if self.active_local_model().is_some_and(|id| self.status_of(&id).is_usable())
            && crate::memory::idle_time_ended(keep, self.last_dictation.elapsed())
        {
            self.model_idle = true;
        }
        let preview = self.preview_model();
        for id in crate::memory::unwanted(&self.model_status, &self.wanted_models()) {
            // The idle active model that also gives the live preview: an engine
            // that holds two parts keeps the preview part in memory.
            match engine.unload(&id, crate::memory::keeps_preview(&id, preview.as_ref())) {
                Ok(()) => {
                    log::info!("unloaded {id}: it is not in use");
                    self.model_status.insert(id, ModelStatus::Downloaded);
                    cx.notify();
                }
                Err(e) => log::warn!("could not unload {id}: {e}"),
            }
        }
    }

    /// A final pass comes: the idle time starts again, and the active model
    /// loads in the background when the idle time unloaded it.
    pub(crate) fn wake_model(&mut self, cx: &mut Context<Self>) {
        self.last_dictation = Instant::now();
        if self.model_idle {
            self.model_idle = false;
            self.load_active_models(cx);
        }
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
        self.downloadable_models().filter(|m| self.status_of(&m.id).is_on_disk()).map(|m| m.size_bytes).sum()
    }

    /// The number of downloaded models, for Settings › History and privacy.
    pub fn models_on_disk(&self) -> usize {
        self.downloadable_models().filter(|m| m.size_bytes > 0 && self.status_of(&m.id).is_on_disk()).count()
    }

    /// The models with files on this computer: the models of [`Self::catalog`],
    /// and the VAD and diarization models.
    fn downloadable_models(&self) -> impl Iterator<Item = ModelInfo> {
        let roles = [ModelTask::Vad, ModelTask::Diarization].into_iter().flat_map(sayso_core::models::models_for).cloned();
        self.catalog().into_iter().chain(roles)
    }

    // -----------------------------------------------------------------------
    // VAD and diarization models (jobs of extensions; dictation does not use them)
    // -----------------------------------------------------------------------

    /// The VAD models of this system, in catalog order. Basic VAD is not a model.
    pub fn vad_models(&self) -> Vec<ModelInfo> {
        sayso_core::models::models_for(ModelTask::Vad).filter(|m| crate::shell::model_runs_here(m)).cloned().collect()
    }

    /// The diarization models of this system, in catalog order.
    pub fn diarization_models(&self) -> Vec<ModelInfo> {
        sayso_core::models::models_for(ModelTask::Diarization).filter(|m| crate::shell::model_runs_here(m)).cloned().collect()
    }

    /// The VAD that a job gets when it names none (`models.vad`).
    pub fn vad_in_use(&self) -> Vad {
        sayso_core::models::vad_for(None, &self.config.models.vad, |id| self.status_of(id).is_on_disk())
    }

    /// The diarization model that a job gets when it names none (`models.diarization`).
    pub fn diarization_in_use(&self) -> Option<ModelId> {
        sayso_core::models::diarization_for(None, &self.config.models.diarization, |id| self.status_of(id).is_on_disk())
    }

    /// Choose the VAD of jobs: Automatic, Basic VAD, or a VAD model.
    pub fn set_vad(&mut self, choice: RoleChoice, cx: &mut Context<Self>) {
        self.edit_config(cx, |c| c.models.vad = choice);
    }

    /// Choose the diarization model of jobs: Automatic or a diarization model.
    pub fn set_diarization(&mut self, choice: RoleChoice, cx: &mut Context<Self>) {
        self.edit_config(cx, |c| c.models.diarization = choice);
    }

    // -----------------------------------------------------------------------
    // Styles and AI
    // -----------------------------------------------------------------------

    /// The icon of the site that the entry went into, when it has one and
    /// site icons are on.
    pub fn site_icon(&self, entry: &HistoryEntry) -> Option<Arc<gpui_kit::Image>> {
        let site = entry.app.as_ref()?.site.as_deref()?;
        self.config.history.site_icons.then(|| self.site_icons.get(site)).flatten()
    }

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

    /// The style of a dictation: the style from the hotkey, then the style
    /// of a style rule, then the default style.
    pub(crate) fn session_style(&self, session: sayso_core::dictation::SessionId) -> Style {
        self.sessions
            .get(&session)
            .and_then(|s| s.picked_style.as_ref().or(s.rule_style.as_ref()))
            .and_then(|id| self.styles.get(id))
            .cloned()
            .unwrap_or_else(|| self.active_style())
    }

    /// The style of the dictation that records now, when it is not the
    /// default style. The overlay shows it.
    pub fn recording_style(&self) -> Option<Style> {
        let State::Recording { session, .. } = self.machine.state else { return None };
        let style = self.session_style(session);
        (style.id != self.active_style().id).then_some(style)
    }

    /// Move to the next style (the cycle-style hotkey). During a recording
    /// this changes the style of that dictation only.
    pub fn cycle_style(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.styles.styles().map(|s| s.id.clone()).collect();
        if let State::Recording { session, .. } = self.machine.state {
            let current = self.session_style(session).id;
            let i = ids.iter().position(|id| *id == current).unwrap_or(0);
            if let Some(s) = self.sessions.get_mut(&session) {
                s.picked_style = Some(ids[(i + 1) % ids.len()].clone());
            }
            cx.notify();
            return;
        }
        let i = ids.iter().position(|id| *id == self.config.ai.active_style).unwrap_or(0);
        let next = ids[(i + 1) % ids.len()].clone();
        self.set_active_style(&next, cx);
        let name = self.active_style().display_name();
        self.blocked_notice = Some(OverlayNotice::new(NoticeIcon::Style, t!("overlay.notice.style", name = name)));
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

    pub fn save_base_prompt(&mut self, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        sayso_core::style::save_base_prompt(&self.paths.styles_dir(), text).map_err(|e| e.to_string())?;
        self.styles = StyleLibrary::load(&self.paths.styles_dir());
        cx.notify();
        Ok(())
    }

    pub fn reset_base_prompt(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = sayso_core::style::reset_base_prompt(&self.paths.styles_dir()) {
            log::warn!("reset base prompt: {e}");
        }
        self.styles = StyleLibrary::load(&self.paths.styles_dir());
        cx.notify();
    }

    /// Turn the style rules on or off. The first time, on adds the starter set.
    pub fn set_style_rules(&mut self, on: bool, cx: &mut Context<Self>) {
        let sites = self.services.platform.context.reads_page_url();
        self.edit_config(cx, |c| {
            if on {
                c.ai.style_rules.turn_on(sayso_core::style_rules::starter_rules(std::env::consts::OS, sites));
            } else {
                c.ai.style_rules.enabled = false;
            }
        });
    }

    /// Delete a user style file. Built-in ids are reset instead.
    pub fn delete_style(&mut self, id: &str, cx: &mut Context<Self>) {
        self.reset_style(id, cx);
        if self.styles.get(id).is_none() {
            let id = id.to_string();
            self.edit_config(cx, |c| c.ai.style_rules.remove_style(&id));
        }
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
        if let Some(sign_in) = self.logins.remove(&id).and_then(|l| l.sign_in) {
            sign_in.cancel();
        }
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

    /// The folder of the login that Sayso keeps for a CLI provider, apart
    /// from the login of the user's terminal.
    pub fn sayso_login_dir(&self, id: &str) -> PathBuf {
        self.paths.data_dir.join("logins").join(id)
    }

    /// Set the folder that holds the login of a CLI provider. None is the
    /// folder that the tool uses by default.
    pub fn set_provider_config_dir(&mut self, id: &str, dir: Option<String>, cx: &mut Context<Self>) {
        let dir = dir.filter(|d| !d.trim().is_empty());
        if self.config.ai.providers.iter().find(|p| p.id == id).is_none_or(|p| p.config_dir() == dir.as_deref()) {
            return;
        }
        self.cancel_sign_in(id, cx);
        // The models and the login belong to the account in the old folder.
        self.model_lists.remove(id);
        self.logins.remove(id);
        let key = id.to_string();
        self.edit_config(cx, move |c| {
            if let Some(p) = c.ai.providers.iter_mut().find(|p| p.id == key)
                && let ProviderKind::ClaudeCli { config_dir, .. } | ProviderKind::CodexCli { config_dir, .. } = &mut p.kind
            {
                *config_dir = dir;
            }
        });
        self.check_provider_login(id, cx);
    }

    pub fn set_provider_thinking(&mut self, id: &str, level: String, cx: &mut Context<Self>) {
        let key = id.to_string();
        self.edit_config(cx, move |c| {
            if let Some(p) = c.ai.providers.iter_mut().find(|p| p.id == key) {
                p.set_thinking(Some(level));
            }
        });
    }

    /// The thinking levels to offer for the model of a provider, from the
    /// least thinking to the most. They come from the model list of the
    /// provider. Until the list is here, a CLI provider offers the levels
    /// that all its models with levels take. Empty for a model without levels.
    pub fn thinking_choices(&self, provider: &Provider) -> Vec<String> {
        let listed = match self.model_lists.get(&provider.id) {
            Some(ModelList::Ready(list)) => list.iter().find(|m| Some(m.id.as_str()) == provider.model()).map(|m| m.thinking.clone()),
            _ => None,
        };
        let cli = matches!(provider.kind, ProviderKind::ClaudeCli { .. } | ProviderKind::CodexCli { .. });
        let mut levels = listed.unwrap_or_else(|| {
            if cli { [sayso_core::config::DEFAULT_CLI_THINKING, "medium", "high"].map(String::from).to_vec() } else { Vec::new() }
        });
        // A level from the config file that the list does not have still shows.
        if let Some(chosen) = provider.thinking().filter(|chosen| !levels.iter().any(|level| level == chosen)) {
            levels.push(chosen.to_string());
        }
        levels
    }

    /// Ask the tool of a CLI provider if it is signed in. Does nothing for
    /// other providers, and while a check or a sign-in runs.
    pub fn check_provider_login(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(provider) = self.config.ai.providers.iter().find(|p| p.id == id).cloned() else { return };
        if !matches!(provider.kind, ProviderKind::ClaudeCli { .. } | ProviderKind::CodexCli { .. }) {
            return;
        }
        let login = self.logins.entry(id.to_string()).or_default();
        if login.checking || login.sign_in.is_some() {
            return;
        }
        login.checking = true;
        cx.notify();
        let key = id.to_string();
        let checked = provider.config_dir().map(str::to_string);
        cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async move { sayso_enhance::auth_status(&provider, Duration::from_secs(10)) }).await;
            let _ = this.update(cx, |m, cx| {
                // The folder changed while the tool ran: the answer is about the old folder.
                let stale = m.config.ai.providers.iter().find(|p| p.id == key).is_none_or(|p| p.config_dir() != checked.as_deref());
                let Some(login) = m.logins.get_mut(&key) else { return };
                login.checking = false;
                if stale {
                    m.check_provider_login(&key, cx);
                    return;
                }
                // The names of the models come with the list.
                let signed_in = matches!(status, Some(sayso_enhance::AuthStatus::SignedIn { .. }));
                login.status = status;
                if signed_in {
                    m.load_provider_models(&key, false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Start the sign-in of the tool of a CLI provider. The tool opens the
    /// sign-in page in the browser.
    pub fn sign_in_provider(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(provider) = self.config.ai.providers.iter().find(|p| p.id == id).cloned() else { return };
        let login = self.logins.entry(id.to_string()).or_default();
        if login.sign_in.is_some() {
            return;
        }
        login.url = None;
        login.error = None;
        cx.notify();
        let sign_in = match sayso_enhance::SignIn::start(&provider) {
            Ok(sign_in) => Arc::new(sign_in),
            Err(e) => {
                login.error = Some(LoginError::Start(e.to_string()));
                return;
            }
        };
        login.sign_in = Some(sign_in.clone());
        let key = id.to_string();
        cx.spawn(async move |this, cx| {
            loop {
                let waiting = sign_in.clone();
                let event = cx.background_spawn(async move { waiting.next_event() }).await;
                let more = matches!(event, Some(sayso_enhance::SignInEvent::Url(_)));
                let _ = this.update(cx, |m, cx| m.sign_in_event(&key, &sign_in, event, cx));
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    fn sign_in_event(&mut self, id: &str, sign_in: &Arc<sayso_enhance::SignIn>, event: Option<sayso_enhance::SignInEvent>, cx: &mut Context<Self>) {
        use sayso_enhance::{AuthStatus, SignInEvent};
        let Some(login) = self.logins.get_mut(id) else { return };
        // False after Cancel, and when a newer sign-in runs.
        let current = login.sign_in.as_ref().is_some_and(|s| Arc::ptr_eq(s, sign_in));
        match event {
            Some(SignInEvent::Url(url)) => {
                if current {
                    login.url = Some(url);
                }
            }
            Some(SignInEvent::Finished(status)) => {
                let signed_in = matches!(status, AuthStatus::SignedIn { .. });
                if current {
                    login.sign_in = None;
                    login.url = None;
                    if !signed_in {
                        login.error = Some(LoginError::NotFinished);
                    }
                }
                if login.sign_in.is_none() {
                    login.status = Some(status);
                }
                if signed_in {
                    self.load_provider_models(id, true, cx);
                }
            }
            None => {
                if current {
                    login.sign_in = None;
                    login.url = None;
                }
            }
        }
        cx.notify();
    }

    /// Stop the sign-in of a provider, if one runs.
    pub fn cancel_sign_in(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(login) = self.logins.get_mut(id) else { return };
        if let Some(sign_in) = login.sign_in.take() {
            sign_in.cancel();
            login.url = None;
            cx.notify();
        }
    }

    /// Give the code from the sign-in page to the tool.
    pub fn send_sign_in_code(&mut self, id: &str, code: &str, cx: &mut Context<Self>) {
        let Some(login) = self.logins.get_mut(id) else { return };
        let Some(sign_in) = login.sign_in.clone() else { return };
        login.error = sign_in.send_code(code.trim()).err().map(|e| LoginError::Code(e.to_string()));
        cx.notify();
    }

    /// Fetch the model list of a saved provider. Without `force`, a list that
    /// is loading or loaded stays as it is.
    pub fn load_provider_models(&mut self, id: &str, force: bool, cx: &mut Context<Self>) {
        let Some(provider) = self.config.ai.providers.iter().find(|p| p.id == id).cloned() else { return };
        // A cleanup model has one fixed model and no list.
        if matches!(provider.kind, ProviderKind::CleanupModel { .. }) {
            return;
        }
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
        let engine = self.services.language_model.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    sayso_enhance::list_provider_models(&provider, secrets.as_ref(), engine.as_deref(), Duration::from_secs(10)).map_err(|e| {
                        // Apple Intelligence: the engine says why it cannot run, as a code the app translates.
                        let reason = match (&provider.kind, engine.as_deref()) {
                            (ProviderKind::AppleIntelligence, Some(engine)) => engine.availability().err().and_then(|u| u.reason),
                            _ => None,
                        };
                        (e, reason)
                    })
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let list = match result {
                    Ok(models) => ModelList::Ready(models),
                    Err((e, reason)) => ModelList::Failed(e.to_string(), reason),
                };
                m.model_lists.insert(key, list);
                cx.notify();
            });
        })
        .detach();
    }

    /// Choose the model a provider uses when a style does not name one.
    pub fn set_provider_model(&mut self, id: &str, model: String, cx: &mut Context<Self>) {
        // The new model can have other thinking levels than the old one.
        let levels = match self.model_lists.get(id) {
            Some(ModelList::Ready(list)) => list.iter().find(|m| m.id == model).map(|m| m.thinking.clone()),
            _ => None,
        };
        let id = id.to_string();
        self.edit_config(cx, move |c| {
            if let Some(p) = c.ai.providers.iter_mut().find(|p| p.id == id) {
                p.set_model(model);
                if let Some(levels) = levels {
                    p.fit_thinking(&levels);
                }
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
        let engine = self.services.language_model.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let enhancer = sayso_enhance::build_enhancer(&provider, secrets.as_ref(), &ai, engine);
                    sayso_enhance::test_provider(enhancer.as_ref()).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |m, cx| done(m, result, cx));
        })
        .detach();
    }

    pub fn detect_providers(&mut self, cx: &mut Context<Self>) {
        let engine = self.services.language_model.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let apple = engine.map(|e| e.availability());
                    Detected {
                        apple_intelligence: apple.as_ref().and_then(|a| a.as_ref().ok().cloned()),
                        apple_unavailable: apple.and_then(|a| a.err()),
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
        self.prepare_active_models();
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

    /// Read the recent entries, the count, and the stats again, on the
    /// background executor.
    pub fn reload_history(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.services.store.clone() else { return };
        self.history_reload += 1;
        let reload = self.history_reload;
        cx.spawn(async move |this, cx| {
            let (recent, total, stats) = cx
                .background_spawn(async move {
                    let q = sayso_store::HistoryQuery { limit: Some(50), ..Default::default() };
                    let recent = store.list(&q).unwrap_or_default();
                    let total = store.count(&sayso_store::HistoryQuery::default()).unwrap_or(0);
                    let since = chrono::Utc::now() - chrono::Duration::days(8);
                    let stats = stats::compute(&store.stats_since(since).unwrap_or_default(), chrono::Local::now());
                    (recent, total, stats)
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                if reload != m.history_reload {
                    return;
                }
                m.recent = recent;
                m.history_total = total;
                m.stats = stats;
                m.history_revision += 1;
                cx.notify();
            });
        })
        .detach();
    }

    /// Measure what the store holds on disk, on the background executor. It
    /// reads the size of every audio file, so only the page that shows it asks.
    pub fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.services.store.clone() else { return };
        cx.spawn(async move |this, cx| {
            let storage = cx.background_spawn(async move { store.storage_use().unwrap_or_default() }).await;
            let _ = this.update(cx, |m, cx| {
                m.storage = storage;
                cx.notify();
            });
        })
        .detach();
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
        self.reload_history(cx);
        cx.notify();
    }

    pub fn clear_history(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.services.store {
            let _ = store.clear_all();
        }
        self.reload_history(cx);
        cx.notify();
    }

    /// Delete the audio of every entry. The text stays.
    pub fn clear_audio(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.services.store
            && let Err(e) = store.clear_audio()
        {
            log::error!("could not delete audio: {e}");
        }
        self.reload_history(cx);
        cx.notify();
    }

    /// Turn incognito on or off. It does nothing while history is off.
    pub fn set_incognito(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.config.history.enabled && self.incognito != on {
            self.incognito = on;
            cx.notify();
        }
    }

    /// The incognito hotkey: switch the mode and say so in the overlay.
    pub fn toggle_incognito(&mut self, cx: &mut Context<Self>) {
        if !self.config.history.enabled {
            return;
        }
        self.set_incognito(!self.incognito, cx);
        let notice = if self.incognito {
            OverlayNotice::new(NoticeIcon::Incognito, t!("overlay.notice.incognito_on")).with_detail(t!("overlay.notice.incognito_on_detail"))
        } else {
            OverlayNotice::new(NoticeIcon::Incognito, t!("overlay.notice.incognito_off")).with_detail(t!("overlay.notice.incognito_off_detail"))
        };
        self.blocked_notice = Some(notice);
    }

    /// History has a place in the Hub while it is on, or while entries from before remain.
    pub fn history_visible(&self) -> bool {
        self.config.history.enabled || self.history_total > 0
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

    /// One-line engine status for the sidebar: (title, detail, color).
    pub fn engine_summary(&self) -> (String, String, fn(&sayso_ui::Colors) -> Hsla) {
        let model = self.active_model();
        if model.is_remote() {
            // The engine process plays no part in a cloud final pass.
            return match self.status_of(&model.id) {
                ModelStatus::Ready => (t!("app.engine.ready"), t!("app.engine.detail_cloud", model = model.name), |c| c.success),
                _ => (t!("app.engine.no_model"), t!("app.engine.choose_model"), |c| c.danger),
            };
        }
        if self.services.engine.is_none() || self.engine_down.is_some() {
            return (t!("app.engine.stopped"), t!("app.engine.restarting"), |c| c.danger);
        }
        if self.engine_starting {
            return (t!("app.engine.loading"), model.name, |c| c.accent);
        }
        match self.status_of(&model.id) {
            ModelStatus::Ready => (t!("app.engine.ready"), model.name, |c| c.success),
            ModelStatus::Optimizing => (t!("app.engine.optimizing"), model.name, |c| c.accent),
            ModelStatus::Downloading { fraction, .. } => {
                (t!("app.engine.downloading"), t!("app.engine.detail_downloading", model = model.name, percent = format!("{:.0}", fraction * 100.0)), |c| c.accent)
            }
            // The idle time unloaded the model. The next dictation loads it.
            ModelStatus::Downloaded if self.model_idle => (t!("app.engine.ready"), t!("app.engine.detail_not_in_memory", model = model.name), |c| c.success),
            ModelStatus::Downloaded => (t!("app.engine.loading"), model.name, |c| c.accent),
            ModelStatus::NotDownloaded => (t!("app.engine.no_model"), t!("app.engine.download_model"), |c| c.danger),
            ModelStatus::Failed { .. } => (t!("app.engine.model_error"), model.name, |c| c.danger),
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
