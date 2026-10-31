//! Startup and window management.

use crate::hub::{HubView, Route};
use crate::model::{AppEvent, AppModel};
use crate::services::Services;
use gpui_kit::*;
use sayso_core::config::{Config, ThemeMode};
use sayso_core::ink::Appearance;
use sayso_core::paths::{Paths, SystemEnv};
use sayso_ui::PaperTheme;
use std::sync::Arc;

/// Window handles. Main thread only.
#[derive(Default)]
pub struct Windows {
    pub hub: Option<AnyWindowHandle>,
    pub onboarding: Option<AnyWindowHandle>,
    pub whats_new: Option<AnyWindowHandle>,
}

impl Global for Windows {}

pub fn run() {
    crate::shell::hand_off();
    let paths = Paths::resolve(&SystemEnv);
    if let Err(e) = paths.create_all() {
        eprintln!("sayso: could not create {}: {e}", paths.data_dir.display());
    }
    let mut loaded = Config::load(&paths.config_file()).unwrap_or_else(|e| {
        eprintln!("sayso: {e}");
        // The file stays as it is, and Settings shows the problem.
        sayso_core::config::LoadedConfig::unreadable(&e)
    });
    init_logging(&paths, loaded.config.advanced.log_level.as_deref());
    crate::shell::prepare();
    for issue in &loaded.issues {
        log::warn!("config: {}: {}", issue.field, issue.problem);
    }
    if loaded.created {
        // The first start: dictation in the language of the system, with a model for it.
        let language = sayso_core::languages::from_system(&sayso_i18n::system_locales());
        loaded.config.dictation.model = sayso_core::models::suggested_for(&language, crate::shell::model_runs_here);
        loaded.config.dictation.language = language;
        let _ = loaded.config.save(&paths.config_file());
    }
    // A staged update takes the place of this version now, before the engine starts.
    let updates = sayso_update::Options::for_this_build(&paths.data_dir, &paths.cache_dir, loaded.config.updates.check, loaded.config.updates.automatic)
        .map(|options| {
            let found = options.at_start();
            if matches!(found, sayso_update::AtStart::Relaunching | sayso_update::AtStart::Stale) {
                log::info!("update: the new version starts");
                std::process::exit(0);
            }
            if let sayso_update::AtStart::Failed(failure) = &found {
                log::warn!("update: {failure}");
            }
            (options, found)
        });

    // Sayso lives in the tray. Without this, GPUI quits on Windows and Linux
    // when the last window closes.
    gpui_kit::application().with_quit_mode(QuitMode::Explicit).with_assets(sayso_ui::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        // sayso-ui has English texts only. These hooks give it the catalogs and
        // the locale of the app, so a change of language shows at the next render.
        sayso_ui::l10n::install(sayso_i18n::translate, sayso_i18n::locale);
        sayso_ui::init(cx);
        // The Insertion sends Shift+Insert on Linux, and the text fields bind
        // only Ctrl+V. Without this, a dictation into Sayso's own fields fails.
        #[cfg(target_os = "linux")]
        cx.bind_keys([KeyBinding::new("shift-insert", gpui_kit::base::input::Paste, Some("Input"))]);
        crate::shell::become_accessory();

        apply_language(&loaded.config);
        let services = start_services(&paths, &loaded.config, updates);
        // Apps connect from now on. When Sayso quits, each one gets `host_closing`.
        // The OS can end Sayso (log out, shut down) without the quit question:
        // `host_closing` still goes out.
        let started = crate::host::start(&paths, &services);
        if let Some(started) = &started {
            let host = started.host.clone();
            cx.on_app_quit(move |_| {
                host.close();
                async {}
            })
            .detach();
        }
        let (host, questions, approvals, host_jobs) = match started {
            Some(s) => (Some(s.host), Some(s.questions), Some(s.approvals), Some(s.jobs)),
            None => (None, None, None, None),
        };
        apply_theme(&loaded.config, &services, cx);
        cx.set_global(Windows::default());

        let config = loaded.config.clone();
        let issues = loaded.issues.clone();
        let p = paths.clone();
        let model = cx.new(|cx| {
            let mut model = AppModel::new(p, config, issues, services, cx);
            model.host_jobs = host_jobs;
            model.host = host.clone();
            if let Some(host) = &host {
                // One value for the host and for Settings.
                model.apps_file = host.apps();
            }
            model
        });
        // A live session or a job of an app starts or ends: the update that
        // waits for it, and the views of the model.
        if let Some(host) = &host {
            let (changed, changes) = async_channel::bounded::<()>(1);
            host.apps().set_observer(Arc::new(move || {
                let _ = changed.try_send(());
            }));
            let m = model.clone();
            cx.spawn(async move |cx| {
                while changes.recv().await.is_ok() {
                    cx.update(|cx| m.update(cx, |m, cx| m.apps_changed(cx)));
                }
            })
            .detach();

            let (changed, changes) = async_channel::bounded::<()>(1);
            host.set_holds_observer(Arc::new(move || {
                let _ = changed.try_send(());
            }));
            let m = model.clone();
            cx.spawn(async move |cx| {
                while changes.recv().await.is_ok() {
                    cx.update(|cx| m.update(cx, |m, cx| m.holds_changed(cx)));
                }
            })
            .detach();
        }

        cx.subscribe(&model, |model, event, cx| match event {
            AppEvent::OpenHub(route) => open_hub(&model, *route, cx),
            AppEvent::ThemeChanged => {}
            AppEvent::LanguageChanged => {
                crate::popover::refresh_tray_labels();
                cx.refresh_windows();
            }
        })
        .detach();

        // The questions of apps come from threads of the host, and show as windows.
        if let Some(questions) = questions {
            crate::download_ask::listen(&model, questions, cx);
        }
        if let Some(approvals) = approvals {
            crate::approval_ask::listen(&model, approvals, cx);
        }
        if crate::dev::arg("ask-approval").is_some() || crate::dev::flag("ask-approval") {
            crate::approval_ask::open_sample(&model, cx);
        }
        if crate::dev::flag("ask-download") {
            crate::download_ask::open_sample(&model, cx);
        }
        crate::overlay::open(&model, cx);
        crate::popover::install(&model, cx);
        crate::a11y::start(cx);
        // A second start of Sayso (Windows, Linux) asks this one to open the Hub.
        if let Some(requests) = crate::shell::open_requests() {
            let m = model.clone();
            cx.spawn(async move |cx| {
                while requests.recv().await.is_ok() {
                    cx.update(|cx| open_hub(&m, Route::Home, cx));
                }
            })
            .detach();
        }
        if crate::dev::flag("popover") {
            let m = model.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(1200)).await;
                cx.update(|cx| crate::popover::toggle(&m, cx));
            })
            .detach();
        }

        let onboarding_flag = crate::dev::flag("onboarding") || crate::dev::arg("onboarding").is_some();
        if let Some(step) = crate::dev::arg("onboarding").and_then(|s| s.parse::<u8>().ok()) {
            model.update(cx, |m, cx| m.set_onboarding_step(step, cx));
        }
        if onboarding_flag || !model.read(cx).config.onboarding.completed {
            open_onboarding(&model, cx);
        } else {
            if crate::dev::flag("hub") || crate::dev::route().is_some() {
                open_hub(&model, crate::dev::route().unwrap_or(Route::Home), cx);
            }
            if crate::dev::flag("whats-new") {
                crate::whats_new::open_newest(&model, cx);
            } else {
                crate::whats_new::show_after_update(&model, cx);
            }
        }
        // Keep the model alive for the whole run.
        std::mem::forget(model);
    });
}

fn init_logging(paths: &Paths, level: Option<&str>) {
    let level = level.unwrap_or("info");
    // The D-Bus library (Linux) logs each connection step at info.
    let filter = format!("{level},zbus=warn,tracing=warn");
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(filter));
    // GPUI logs its errors without a module. The file and the line say where they come from.
    builder.format(|out, record| {
        use std::io::Write;
        let source = match (record.target(), record.file(), record.line()) {
            ("", Some(file), Some(line)) => format!("{file}:{line}"),
            (target, ..) => target.to_owned(),
        };
        writeln!(out, "[{} {:<5} {source}] {}", out.timestamp(), record.level(), record.args())
    });
    if let Ok(file) = std::fs::File::create(paths.log_dir().join("sayso.log")) {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }
    let _ = builder.try_init();
}

fn start_services(paths: &Paths, config: &Config, updates: Option<(sayso_update::Options, sayso_update::AtStart)>) -> Services {
    // Sounds are embedded; write them where the player can read them.
    let sounds_dir = paths.cache_dir.join("sounds");
    let _ = std::fs::create_dir_all(&sounds_dir);
    for name in ["start", "stop", "cancel", "insert"] {
        if let Some(bytes) = sayso_ui::assets::bytes(&format!("sounds/{name}.wav")) {
            let _ = std::fs::write(sounds_dir.join(format!("{name}.wav")), bytes);
        }
    }
    let platform = crate::shell::platform(sounds_dir);
    let mic = crate::mic::SharedMic::new(match crate::dev::fake_mic() {
        Some(path) => Arc::new(crate::dev::FakeMic(path)),
        None => platform.audio.clone(),
    });

    let engine_path = config
        .advanced
        .engine_path
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(sayso_engine_client::EngineClient::default_engine_path);
    // The engine answers on a thread of its own, so the tray and the overlay
    // do not wait for it. `AppModel` hears `Started` or `StartFailed`.
    // The migration can hash gigabytes, so it runs on that thread too. An engine
    // that the user chose can be older than the store, so it keeps its folders.
    let chosen_engine = config.advanced.engine_path.is_some() || std::env::var_os("SAYSO_ENGINE_PATH").is_some_and(|path| !path.is_empty());
    let before_launch = (!chosen_engine).then(|| {
        let models_dir = paths.models_dir();
        sayso_engine_client::BeforeLaunch(Arc::new(move || model_store_env(&models_dir)))
    });
    let options = sayso_engine_client::Options { env: crate::shell::engine_env(), before_launch, ..Default::default() };
    let (client, engine_events) = match sayso_engine_client::EngineClient::start_with(engine_path.clone(), paths.models_dir(), paths.cache_dir.clone(), options) {
        Ok((e, events)) => (Some(e), Some(events)),
        Err(e) => {
            log::error!("could not start the engine at {}: {e:#}", engine_path.display());
            (None, None)
        }
    };
    // Both handles share the one sidecar.
    let language_model = client.clone().map(|e| Arc::new(e) as Arc<dyn sayso_core::enhance::LanguageModel>);
    let engine = client.map(|e| Arc::new(e) as Arc<dyn sayso_platform::SttBackend>);

    let store = match sayso_store::Store::open(paths.database_file(), paths.audio_dir()) {
        Ok(s) => {
            if crate::dev::flag("seed-demo") {
                crate::dev::seed_demo(&s);
            }
            Some(Arc::new(s))
        }
        Err(e) => {
            log::error!("could not open the database: {e}");
            None
        }
    };

    // The cleanup runtime starts at its first use, not here.
    let cleanup_runtime = sayso_cleanup_client::runtime::Runtime::new(sayso_cleanup_client::runtime::Runtime::default_path());
    // Nothing loads while no cleanup model is in use. `apply_config_change` keeps this up to date.
    cleanup_runtime.set_enabled(
        config.ai.enabled && config.ai.providers.iter().any(|p| matches!(p.kind, sayso_core::config::ProviderKind::CleanupModel { .. })),
    );
    let cleaner = Arc::new(sayso_cleanup_client::cleaner::RuntimeCleaner::new(cleanup_runtime.clone(), paths.models_dir()));

    Services { platform, mic, engine, engine_events, language_model, cleaner, cleanup_runtime, store, secrets: Arc::new(sayso_enhance::KeychainStore), updates }
}

/// Set the interface language from config and the languages of the system.
/// `--pseudo` turns on the pseudo-locale in a debug build (see `docs/i18n.md`).
pub fn apply_language(config: &Config) {
    let setting = if sayso_i18n::HAS_PSEUDO && crate::dev::flag("pseudo") { sayso_i18n::PSEUDO } else { config.general.language.as_str() };
    let locale = sayso_i18n::resolve(setting, &sayso_i18n::system_locales());
    sayso_i18n::set_locale(locale);
    log::info!("interface language: {locale}");
}

/// Build the paper theme from config and the system appearance.
pub fn apply_theme(config: &Config, services: &Services, cx: &mut App) {
    let forced = if crate::dev::flag("dark") { Some(true) } else if crate::dev::flag("light") { Some(false) } else { None };
    let dark = forced.unwrap_or(match config.appearance.theme {
        ThemeMode::System => services.platform.prefs.dark_mode(),
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
    });
    let reduce = config.appearance.reduce_motion.unwrap_or_else(|| services.platform.prefs.reduce_motion());
    let theme = PaperTheme::new(
        config.appearance.ink,
        if dark { Appearance::Dark } else { Appearance::Light },
        reduce,
        config.appearance.paper_texture,
    );
    sayso_ui::theme::set(cx, theme);
}

fn titlebar() -> Option<TitlebarOptions> {
    Some(TitlebarOptions { title: Some("Sayso".into()), appears_transparent: true, traffic_light_position: Some(point(px(18.), px(18.))) })
}

/// Run an AppKit window call (move, show, hide, focus, activate) after the
/// current GPUI update. AppKit answers these calls with window callbacks into
/// GPUI at once. Inside an update the callbacks find the app borrowed, and
/// GPUI drops them with "RefCell already borrowed": a missed move, or a missed
/// focus change.
pub fn appkit_later(cx: &App, f: impl FnOnce() + 'static) {
    cx.foreground_executor().spawn(async move { f() }).detach();
}

pub fn open_hub(model: &Entity<AppModel>, route: Route, cx: &mut App) {
    model.update(cx, |m, cx| m.navigate(route, cx));
    if let Some(handle) = cx.global::<Windows>().hub
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
            appkit_later(cx, crate::shell::show_app_and_activate);
            return;
        }
    let bounds = Bounds::centered(None, crate::dev::size().unwrap_or(size(px(1280.), px(820.))), cx);
    let model2 = model.clone();
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: titlebar(),
            window_min_size: crate::dev::size().is_none().then_some(crate::layout::min_size(crate::layout::HUB_MIN)),
            // The window class on Linux, which matches the desktop entry.
            app_id: Some(crate::shell::APP_ID.into()),
            window_decorations: crate::chrome::decorations(&model.read(cx).config),
            window_background: crate::chrome::background(),
            focus: true,
            show: true,
            ..Default::default()
        },
        cx,
        |window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.global_mut::<Windows>().hub = None;
                crate::shell::set_dock_icon_visible(false);
                true
            });
            cx.new(|cx| HubView::new(model2, window, cx))
        },
    );
    match opened {
        Ok((handle, _)) => {
            cx.global_mut::<Windows>().hub = Some(handle);
            if model.read(cx).config.general.dock_icon_with_hub {
                crate::shell::set_dock_icon_visible(true);
            }
            appkit_later(cx, crate::shell::show_app_and_activate);
        }
        Err(e) => log::error!("could not open the Hub: {e:#}"),
    }
}

pub fn open_onboarding(model: &Entity<AppModel>, cx: &mut App) {
    if let Some(handle) = cx.global::<Windows>().onboarding
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    let bounds = Bounds::centered(None, crate::dev::size().unwrap_or(crate::layout::min_size(crate::layout::ONBOARDING_SIZE)), cx);
    let model2 = model.clone();
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: titlebar(),
            // Linux gives no fixed size, so the window also has a minimum size.
            is_resizable: crate::dev::size().is_some(),
            window_min_size: crate::dev::size().is_none().then_some(crate::layout::min_size(crate::layout::ONBOARDING_SIZE)),
            focus: true,
            // The window class on Linux, which matches the desktop entry.
            app_id: Some(crate::shell::APP_ID.into()),
            window_decorations: crate::chrome::decorations(&model.read(cx).config),
            window_background: crate::chrome::background(),
            show: true,
            ..Default::default()
        },
        cx,
        |window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.global_mut::<Windows>().onboarding = None;
                crate::shell::set_dock_icon_visible(false);
                true
            });
            cx.new(|cx| crate::onboarding::OnboardingView::new(model2, window, cx))
        },
    );
    match opened {
        Ok((handle, _)) => {
            cx.global_mut::<Windows>().onboarding = Some(handle);
            crate::shell::set_dock_icon_visible(true);
            appkit_later(cx, crate::shell::show_app_and_activate);
        }
        Err(e) => log::error!("could not open onboarding: {e:#}"),
    }
}

/// Close onboarding and open the Hub (the "Open Sayso" button).
pub fn finish_onboarding(model: &Entity<AppModel>, window: &mut Window, cx: &mut App) {
    model.update(cx, |m, cx| m.finish_onboarding(cx));
    // A first install is not an update: it gets no "What is new" window.
    crate::whats_new::mark_seen(&model.read(cx).paths);
    cx.global_mut::<Windows>().onboarding = None;
    window.remove_window();
    open_hub(model, Route::Home, cx);
}

/// Move the models from the old folders into the model store, and give the
/// environment that tells the engine to use the store. After an error, the engine
/// runs on the old folders for this start when they are all still there, and the
/// next start goes on with the migration.
fn model_store_env(models_dir: &std::path::Path) -> Vec<(String, String)> {
    let Some(store) = models_dir.join(sayso_core::model_store::FOLDER).to_str().map(str::to_string) else {
        log::warn!("the models folder {} is not UTF-8, so Sayso does not use the model store", models_dir.display());
        return Vec::new();
    };
    let store_env = vec![("SAYSO_MODEL_STORE".to_string(), store)];
    // A panic here must not leave the app waiting for an engine that never starts.
    let result = std::panic::catch_unwind(|| sayso_core::model_migration::run(models_dir));
    match result {
        Ok(Ok(report)) => {
            if !report.moved.is_empty() {
                log::info!("moved to the model store: {}", report.moved.join(", "));
            }
            for (model, reason) in &report.kept {
                log::info!("{model} is not in the model store, so its old files stay: {reason}");
            }
            store_env
        }
        failed => {
            match failed {
                Ok(Err(e)) => log::error!("the migration of the models failed: {e}"),
                _ => log::error!("the migration of the models panicked"),
            }
            if sayso_core::model_migration::store_in_use(models_dir) { store_env } else { Vec::new() }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_engine_gets_the_store_only_after_the_migration_ran() {
        let models = tempfile::tempdir().unwrap();
        let store = models.path().join("v1");
        assert_eq!(super::model_store_env(models.path()), [("SAYSO_MODEL_STORE".to_string(), store.to_str().unwrap().to_string())]);
        assert!(store.join("migration.json").is_file());

        // A store that cannot open: the engine uses its folder for this start.
        let models = tempfile::tempdir().unwrap();
        std::fs::write(models.path().join("v1"), "").unwrap();
        assert_eq!(super::model_store_env(models.path()), []);
    }

    /// `sayso-ui` asks for its texts by key at run time, so the catalog test of
    /// `sayso-i18n` does not see them. Each key must be in `en.yml`, with the
    /// text that `sayso-ui` has built in.
    #[test]
    fn the_catalogs_have_the_keys_of_sayso_ui() {
        let en = sayso_i18n::__messages("en").expect("the English catalog");
        for (key, text) in sayso_ui::l10n::TEXTS {
            let found = en.iter().find(|(k, _)| k == key).map(|(_, t)| t.as_str());
            assert_eq!(found, Some(*text), "{key} in en.yml");
            assert!(sayso_i18n::translate(key).is_some(), "{key} has no text in the current locale");
        }
    }
}
