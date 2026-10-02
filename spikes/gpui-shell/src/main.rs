//! Throwaway spike: GPUI overlay (S1) + tray popover (S2).
//! Flags: --overlay | --tray (default both), --no-fix, --fix=shadow, --deactivate, --delay=N, --auto-toggle=N.
mod mac;
mod overlay;
mod tray;

use objc2::MainThreadMarker;
use std::time::Duration;

pub use gpui_kit::open_window;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let (only_overlay, only_tray) = (has("--overlay"), has("--tray"));
    let want_overlay = only_overlay || !only_tray;
    let want_tray = only_tray || !only_overlay;
    let fix = match () {
        _ if has("--no-fix") => overlay::Fix::None,
        _ if has("--fix=shadow") => overlay::Fix::Shadow,
        _ => overlay::Fix::Full,
    };
    // `--auto-toggle=N`: fake a tray click after N s, and again N s later (unattended test).
    let auto_toggle: Option<u64> = args
        .iter()
        .find_map(|a| a.strip_prefix("--auto-toggle=")?.parse().ok());
    let deactivate = has("--deactivate");
    // `--delay=N`: show the overlay N seconds after launch (a real app shows it long after launch).
    let delay: u64 = args
        .iter()
        .find_map(|a| a.strip_prefix("--delay=")?.parse().ok())
        .unwrap_or(0);

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let mtm = MainThreadMarker::new().expect("main thread");
        // GPUI forces Regular in applicationDidFinishLaunching, which runs before this closure.
        println!("[app] activation policy after setActivationPolicy(accessory): {} (0=Regular 1=Accessory)", mac::become_accessory(mtm));
        if want_overlay {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(delay)).await;
                cx.update(|cx| overlay::open(cx, fix, deactivate));
            })
            .detach();
        }
        if want_tray {
            tray::open(cx, auto_toggle);
        }
        // Does the policy stick? Re-read after windows exist. Also trace app state.
        cx.spawn(async move |cx| {
            for i in 0..18 {
                println!("[app +{:.1}s] policy={} active={} frontmost={}", i as f32 * 0.5, mac::policy(mtm), mac::app_is_active(mtm), mac::frontmost_bundle_id());
                cx.background_executor().timer(Duration::from_millis(500)).await;
            }
        })
        .detach();
    });
}
