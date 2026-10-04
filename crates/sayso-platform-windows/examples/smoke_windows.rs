//! Hardware smoke test for the Windows platform layer.
//!
//! Run: `cargo run -p sayso-platform-windows --example smoke_windows`
//!
//! It reads state, registers and releases the hotkeys, records 2 s from the
//! default microphone, and plays each sound once. It never changes system
//! settings, never touches the clipboard, and never sends a key.

#[cfg(windows)]
use sayso_core::hotkey::Hotkey;
#[cfg(windows)]
use sayso_platform::{Permission, SoundKind};
#[cfg(windows)]
use sayso_platform_windows::WinPlatform;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
fn main() {
    env_logger::init();
    let sounds_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/sounds");
    let p = WinPlatform::new(sounds_dir);

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
    let running = p.context.running_apps();
    println!("running apps: {}", running.len());
    for app in running.iter().take(8) {
        println!("  {} ({}, pid {})", app.name, app.bundle_id, app.pid);
    }
    let toggle = Hotkey::toggle_default();
    println!("conflicts for {toggle}:");
    for c in p.hotkeys.conflicts(&toggle) {
        println!("  {:?}", c.source);
    }
    for key in ["cmd+space", "cmd+v", "ctrl+opt+space", "opt+shift+v", "ctrl+cmd+v", "opt+cmd+space"] {
        let hk: Hotkey = key.parse().unwrap();
        println!("conflicts for {key}: {:?}", p.hotkeys.conflicts(&hk).into_iter().map(|c| c.source).collect::<Vec<_>>());
    }
    println!("secure input holder: {:?}", p.hotkeys.secure_input_holder());
    if let Some(app) = p.context.frontmost_app() {
        let png = p.context.app_icon_png(&app.bundle_id, 64);
        println!(
            "icon of {}: {:?} bytes, PNG header ok: {}",
            app.name,
            png.as_ref().map(Vec::len),
            png.as_deref().is_some_and(|b| b.starts_with(&[0x89, b'P', b'N', b'G']))
        );
    }
    if let Some(app) = running.iter().find(|a| a.bundle_id != "explorer.exe") {
        let png = p.context.app_icon_png(&app.bundle_id, 32);
        println!("icon of {}: {:?} bytes", app.name, png.as_ref().map(Vec::len));
    }

    println!("== hotkey registration (main thread)");
    let bindings = sayso_platform::HotkeyBindings {
        toggle: Some(Hotkey::toggle_default()),
        push_to_talk: Some(Hotkey::Solo(sayso_core::hotkey::SoloModifier::RightOption)),
        paste_last: Some(Hotkey::paste_last_default()),
        cycle_style: Some("ctrl+opt+s".parse().unwrap()),
        incognito: None,
        single_escape: false,
        double_escape_window: Duration::from_millis(400),
    };
    let registration = p.hotkeys.register(&bindings);
    println!("failed: {:?}, needs input monitoring: {}", registration.failed, registration.needs_input_monitoring);
    println!("conflicts for {toggle} while Sayso holds it: {}", p.hotkeys.conflicts(&toggle).len());
    let fn_ptt = sayso_platform::HotkeyBindings { push_to_talk: Some("fn".parse().unwrap()), ..bindings.clone() };
    println!("fn as push-to-talk: {:?}", p.hotkeys.register(&fn_ptt).failed);
    p.hotkeys.set_recording(true);
    p.hotkeys.set_recording(false);
    p.hotkeys.register(&sayso_platform::HotkeyBindings {
        toggle: None,
        push_to_talk: None,
        paste_last: None,
        cycle_style: None,
        incognito: None,
        ..bindings
    });
    println!("unregistered again");

    println!("== system");
    println!("dark mode: {}", p.prefs.dark_mode());
    println!("reduce motion: {}", p.prefs.reduce_motion());
    println!("login item: {:?}", p.login_item.state());

    println!("== raw RegisterHotKey probes");
    for key in ["opt+space", "cmd+v", "cmd+space", "ctrl+cmd+v", "opt+shift+v", "ctrl+opt+space"] {
        let hk: Hotkey = key.parse().unwrap();
        println!("{key}: {:?}", sayso_platform_windows::conflicts::probe(&hk));
    }

    println!("== capture 2 s from the default microphone");
    let capture = |device: Option<&str>, seconds: u64| {
        let frames = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let samples = Arc::new(AtomicUsize::new(0));
        let (f, pk, sm) = (frames.clone(), peak.clone(), samples.clone());
        let start = p.audio.start(
            device,
            Box::new(move |frame| {
                f.fetch_add(1, Ordering::Relaxed);
                sm.fetch_add(frame.samples.len(), Ordering::Relaxed);
                pk.fetch_max((frame.level * 1000.0) as usize, Ordering::Relaxed);
            }),
        );
        match start {
            Ok(handle) => {
                std::thread::sleep(Duration::from_secs(seconds));
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
    };
    capture(None, 2);
    for device in p.audio.devices().into_iter().filter(|d| !d.is_default) {
        print!("1 s from {}: ", device.name);
        capture(Some(&device.id), 1);
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

#[cfg(not(windows))]
fn main() {}
