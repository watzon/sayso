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
}

impl Global for Windows {}

pub fn run() {
    crate::shell::hand_off();
    let paths = Paths::resolve(&SystemEnv);
    if let Err(e) = paths.create_all() {
        eprintln!("sayso: could not create {}: {e}", paths.data_dir.display());
    }
    let loaded = Config::load(&paths.config_file()).unwrap_or_else(|e| {
        eprintln!("sayso: {e}");
        sayso_core::config::LoadedConfig { config: Config::default(), issues: vec![], created: true }
    });
    init_logging(&paths, loaded.config.advanced.log_level.as_deref());
    crate::shell::prepare();
    for issue in &loaded.issues {
        log::warn!("config: {}: {}", issue.field, issue.message);
    }
    if loaded.created {
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
            if let sayso_update::AtStart::Failed(message) = &found {
                log::warn!("update: {message}");
            }
            (options, found)
        });

    gpui_kit::application().with_assets(sayso_ui::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        sayso_ui::init(cx);
        // The Insertion sends Shift+Insert on Linux, and the text fields bind
        // only Ctrl+V. Without this, a dictation into Sayso's own fields fails.
        #[cfg(target_os = "linux")]
        cx.bind_keys([KeyBinding::new("shift-insert", gpui_kit::base::input::Paste, Some("Input"))]);
        crate::shell::become_accessory();

        let services = start_services(&paths, &loaded.config, updates);
        apply_theme(&loaded.config, &services, cx);
        cx.set_global(Windows::default());

        let config = loaded.config.clone();
        let issues = loaded.issues.clone();
        let p = paths.clone();
        let model = cx.new(|cx| AppModel::new(p, config, issues, services, cx));

        cx.subscribe(&model, |model, event, cx| match event {
            AppEvent::OpenHub(route) => open_hub(&model, *route, cx),
            AppEvent::ThemeChanged => {}
        })
        .detach();

        crate::overlay::open(&model, cx);
        crate::popover::install(&model, cx);
        // A second start of Sayso (Linux) asks this one to open the Hub.
        let m = model.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                if crate::shell::open_requested() {
                    cx.update(|cx| open_hub(&m, Route::Home, cx));
                }
            }
        })
        .detach();
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
        } else if crate::dev::flag("hub") || crate::dev::route().is_some() {
            open_hub(&model, crate::dev::route().unwrap_or(Route::Home), cx);
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

    let engine_path = config
        .advanced
        .engine_path
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(sayso_engine_client::EngineClient::default_engine_path);
    let client = match sayso_engine_client::EngineClient::spawn(engine_path.clone(), paths.models_dir(), paths.cache_dir.clone()) {
        Ok(e) => Some(e),
        Err(e) => {
            log::error!("could not start the engine at {}: {e:#}", engine_path.display());
            None
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

    Services { platform, engine, language_model, store, secrets: Arc::new(sayso_enhance::KeychainStore), updates }
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
    let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
    let model2 = model.clone();
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: titlebar(),
            window_min_size: Some(size(px(1040.), px(680.))),
            inactive_frame_interval: None,
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
    let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
    let model2 = model.clone();
    let opened = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: titlebar(),
            is_resizable: false,
            focus: true,
            // The window class on Linux, which matches the desktop entry.
            app_id: Some(crate::shell::APP_ID.into()),
            window_decorations: crate::chrome::decorations(&model.read(cx).config),
            window_background: crate::chrome::background(),
            show: true,
            inactive_frame_interval: None,
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
    cx.global_mut::<Windows>().onboarding = None;
    window.remove_window();
    open_hub(model, Route::Home, cx);
}
