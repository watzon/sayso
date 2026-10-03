//! Hardware smoke test for the macOS platform layer.
//!
//! Run: `cargo run -p sayso-platform-macos --example smoke`
//!
//! It reads state, records 2 s from the default microphone, and plays each
//! sound once. It never changes system settings and never requests a
//! permission (a request from an unbundled binary could crash it).

// The crate is empty on other systems, so the example is too.
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(target_os = "macos")]
mod mac {
    use sayso_core::hotkey::Hotkey;
    use sayso_platform::{Permission, SoundKind};
    use sayso_platform_macos::MacPlatform;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    pub fn main() {
        env_logger::init();
        let sounds_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/sounds");
        let p = MacPlatform::new(sounds_dir);

        println!("== permissions");
        for permission in [Permission::Microphone, Permission::Accessibility, Permission::InputMonitoring] {
            println!("{permission:?}: {:?}", p.permissions.status(permission));
        }

        println!("== input devices");
        for d in p.audio.devices() {
            println!("{}{} (id {})", if d.is_default { "* " } else { "  " }, d.name, d.id);
        }

        println!("== context");
        println!("frontmost: {:?}", p.context.frontmost_app());
        println!("running apps: {}", p.context.running_apps().len());
        let toggle = Hotkey::toggle_default();
        println!("conflicts for {toggle}:");
        for c in p.hotkeys.conflicts(&toggle) {
            println!("  {:?}", c.source);
        }
        for key in ["cmd+space", "ctrl+space", "ctrl+cmd+space"] {
            let hk: Hotkey = key.parse().unwrap();
            println!("conflicts for {key}: {:?}", p.hotkeys.conflicts(&hk).into_iter().map(|c| c.source).collect::<Vec<_>>());
        }
        println!("secure input holder: {:?}", p.hotkeys.secure_input_holder());
        if let Some(app) = p.context.frontmost_app() {
            let png = p.context.app_icon_png(&app.bundle_id, 64);
            println!("icon of {}: {:?} bytes, PNG header ok: {}", app.name, png.as_ref().map(Vec::len), png.as_deref().is_some_and(|b| b.starts_with(&[0x89, b'P', b'N', b'G'])));
        }

        println!("== hotkey registration (main thread)");
        let bindings = sayso_platform::HotkeyBindings {
            toggle: Some(Hotkey::toggle_default()),
            push_to_talk: Some(Hotkey::Solo(sayso_core::hotkey::SoloModifier::RightOption)),
            paste_last: Some(Hotkey::paste_last_default()),
            cycle_style: Some("ctrl+cmd+s".parse().unwrap()),
            single_escape: false,
            double_escape_window: Duration::from_millis(400),
        };
        let registration = p.hotkeys.register(&bindings);
        println!("failed: {:?}, needs input monitoring: {}", registration.failed, registration.needs_input_monitoring);
        p.hotkeys.set_recording(true);
        p.hotkeys.set_recording(false);
        p.hotkeys.register(&sayso_platform::HotkeyBindings { toggle: None, push_to_talk: None, paste_last: None, cycle_style: None, ..bindings });
        println!("unregistered again");

        println!("== system");
        println!("dark mode: {}", p.prefs.dark_mode());
        println!("reduce motion: {}", p.prefs.reduce_motion());
        println!("login item: {:?}", p.login_item.state());
        println!("fullscreen frontmost app: {}", sayso_platform_macos::window::frontmost_app_is_fullscreen());
        println!("mouse: {:?}", sayso_platform_macos::window::mouse_location());

        println!("== capture 2 s from the default microphone");
        let frames = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let samples = Arc::new(AtomicUsize::new(0));
        let (f, pk, sm) = (frames.clone(), peak.clone(), samples.clone());
        let start = p.audio.start(
            None,
            Box::new(move |frame| {
                f.fetch_add(1, Ordering::Relaxed);
                sm.fetch_add(frame.samples.len(), Ordering::Relaxed);
                pk.fetch_max((frame.level * 1000.0) as usize, Ordering::Relaxed);
            }),
        );
        match start {
            Ok(handle) => {
                std::thread::sleep(Duration::from_secs(2));
                handle.stop();
                println!(
                    "frames: {}, samples: {}, peak level: {:.3}",
                    frames.load(Ordering::Relaxed),
                    samples.load(Ordering::Relaxed),
                    peak.load(Ordering::Relaxed) as f32 / 1000.0
                );
            }
            Err(err) => println!("capture failed: {err}"),
        }

        println!("== sounds");
        for (name, sound) in
            [("start", SoundKind::Start), ("stop", SoundKind::Stop), ("cancel", SoundKind::Cancel), ("insert", SoundKind::Insert)]
        {
            println!("play {name}");
            p.sounds.play(sound, 0.5);
            std::thread::sleep(Duration::from_millis(500));
        }
        println!("done");
    }
}
