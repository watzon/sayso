//! Linux backend: `sayso-platform-linux`.
//!
//! GPUI runs on X11 (an X11 session, or XWayland on a compositor without the
//! layer shell, such as GNOME) or on Wayland with the layer shell (KDE,
//! sway, Hyprland). On X11 the app places its windows like on macOS. On
//! Wayland the compositor places them: the overlay is a layer surface at the
//! bottom center, and the popover a layer surface at the top right.

use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sayso_core::models::ModelInfo;
use sayso_platform::Platform;
use sayso_platform_linux::ipc::{self, Claim, Command};
use sayso_platform_linux::session::{self, Session, UiBackend};
use sayso_platform_linux::window as xw;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub use xw::{MonitorToken, Point, Rect, ScreenInfo};

/// The system's handle of a GPUI window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeWindow {
    /// An X11 window id.
    X11(u32),
    /// A Wayland surface. GPUI owns it; the calls here do nothing with it.
    Wayland,
}

fn session() -> &'static Session {
    session::current()
}

fn on_x11() -> bool {
    session().ui == UiBackend::X11
}

/// The first startup step, before logging starts: send a `--toggle` style
/// command to the running Sayso, or become the one Sayso. Exits the process
/// when another Sayso already runs, so it never touches that Sayso's log.
pub fn hand_off() {
    if let Some(command) = Command::from_args(std::env::args()) {
        match ipc::send(command) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("sayso: Sayso is not running, so it cannot {}: {e}", command.word());
                std::process::exit(1);
            }
        }
    }
    if let Claim::Running = ipc::claim() {
        let _ = ipc::send(Command::Open);
        eprintln!("sayso: Sayso is already running. Its window opens.");
        std::process::exit(0);
    }
}

/// Startup work before GPUI starts: pick GPUI's display server.
pub fn prepare() {
    session::prepare();
}

pub fn platform(sounds_dir: PathBuf) -> Platform {
    sayso_platform_linux::LinuxPlatform::new(sounds_dir)
}

/// True once for each `sayso` start that asked the running Sayso to open.
pub fn open_requested() -> bool {
    let mut asked = false;
    while let Ok(Command::Open) = ipc::app_commands().try_recv() {
        asked = true;
    }
    asked
}

pub fn native(window: &Window) -> Option<NativeWindow> {
    xw::set_scale(f64::from(window.scale_factor()));
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(NativeWindow::X11(h.window as u32)),
        RawWindowHandle::Xcb(h) => Some(NativeWindow::X11(h.window.get())),
        RawWindowHandle::Wayland(_) => Some(NativeWindow::Wayland),
        _ => None,
    }
}

// App -----------------------------------------------------------------------
// Linux has no Dock and no app activation apart from the windows.

pub fn become_accessory() {}

pub fn set_dock_icon_visible(_visible: bool) {}

pub fn show_app_and_activate() {}

pub fn deactivate_app() {}

// Windows -------------------------------------------------------------------

pub fn configure_overlay(_window: NativeWindow) {}

pub fn make_never_key(_window: NativeWindow) {}

pub fn set_frame_origin(window: NativeWindow, x: f64, y: f64) {
    if let NativeWindow::X11(id) = window {
        xw::set_frame_origin(id, x, y);
    }
}

pub fn set_frame(window: NativeWindow, x: f64, y: f64, width: f64, height: f64) {
    if let NativeWindow::X11(id) = window {
        xw::set_frame(id, x, y, width, height);
    }
}

pub fn frame(window: NativeWindow) -> Option<Rect> {
    match window {
        NativeWindow::X11(id) => xw::frame(id),
        NativeWindow::Wayland => None,
    }
}

/// GPUI draws a transparent background already.
pub fn make_clear(_window: NativeWindow) {}

pub fn add_child_window(parent: NativeWindow, child: NativeWindow) {
    if let (NativeWindow::X11(parent), NativeWindow::X11(child)) = (parent, child) {
        xw::set_transient_for(child, parent);
    }
}

pub fn order_front_without_activating(window: NativeWindow) {
    if let NativeWindow::X11(id) = window {
        xw::show(id);
    }
}

pub fn order_out(window: NativeWindow) {
    if let NativeWindow::X11(id) = window {
        xw::hide(id);
    }
}

pub fn show_and_focus(window: NativeWindow) {
    if let NativeWindow::X11(id) = window {
        xw::show_and_focus(id);
    }
}

// Overlay -------------------------------------------------------------------

/// The overlay: an X11 pop-up, or a layer surface at the bottom center.
pub fn overlay_kind() -> WindowKind {
    if on_x11() {
        return WindowKind::PopUp;
    }
    WindowKind::LayerShell(LayerShellOptions {
        namespace: "sayso-overlay".into(),
        layer: Layer::Overlay,
        anchor: Anchor::BOTTOM,
        keyboard_interactivity: KeyboardInteractivity::None,
        ..Default::default()
    })
}

/// The popover: an X11 pop-up, or a layer surface at the top right.
pub fn popover_kind() -> WindowKind {
    if on_x11() {
        return WindowKind::PopUp;
    }
    WindowKind::LayerShell(LayerShellOptions {
        namespace: "sayso-popover".into(),
        layer: Layer::Overlay,
        anchor: Anchor::TOP | Anchor::RIGHT,
        margin: Some((px(6.), px(6.), px(0.), px(0.))),
        keyboard_interactivity: KeyboardInteractivity::OnDemand,
        ..Default::default()
    })
}

/// A list that opens from a trigger: a Wayland popup under it, which the
/// compositor places and flips. None on X11, where the app places it.
pub fn popup_kind(parent: AnyWindowHandle, anchor: Bounds<Pixels>, offset: gpui_kit::Point<Pixels>) -> Option<WindowKind> {
    use gpui_kit::popup::{PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupOptions};
    if on_x11() {
        return None;
    }
    Some(WindowKind::AnchoredPopup(PopupOptions {
        parent,
        anchor_rect: anchor,
        anchor: PopupAnchor::BottomLeft,
        gravity: PopupGravity::BottomRight,
        constraint_adjustment: PopupConstraintAdjustment::FLIP_Y | PopupConstraintAdjustment::SLIDE_X | PopupConstraintAdjustment::RESIZE_Y,
        offset,
        grab: true,
    }))
}

thread_local! {
    /// The input region last given to each X11 window, and when it was raised.
    static REGIONS: RefCell<HashMap<u32, (Bounds<Pixels>, Instant)>> = RefCell::new(HashMap::new());
}

/// How often the overlay goes back on top on X11. Another window raised
/// later can cover an override-redirect window.
const RAISE_EVERY: Duration = Duration::from_secs(2);

/// Let the mouse reach the overlay only over the card. The empty part of the
/// window lets clicks through to the app underneath.
pub fn set_click_region(window: &Window, native: NativeWindow, card: Bounds<Pixels>, _inside: bool) {
    match native {
        NativeWindow::Wayland => window.set_input_region(Some(&[card])),
        NativeWindow::X11(id) => REGIONS.with(|regions| {
            let mut regions = regions.borrow_mut();
            let now = Instant::now();
            let entry = regions.entry(id).or_insert((Bounds::default(), now - RAISE_EVERY));
            if entry.0 != card {
                let rect = Rect {
                    x: card.origin.x.as_f32().into(),
                    y: card.origin.y.as_f32().into(),
                    width: card.size.width.as_f32().into(),
                    height: card.size.height.as_f32().into(),
                };
                xw::set_input_rect(id, Some(rect));
                entry.0 = card;
            }
            if now.duration_since(entry.1) >= RAISE_EVERY {
                xw::raise(id);
                entry.1 = now;
            }
        }),
    }
}

/// True when the app can move its windows to a screen position.
pub fn can_place_windows() -> bool {
    on_x11()
}

/// True when [`order_out`] hides a window and [`show_and_focus`] shows it
/// again. A layer surface cannot be hidden, so it is closed instead.
pub fn can_hide_windows() -> bool {
    on_x11()
}

// Screens and mouse ---------------------------------------------------------

pub fn screens() -> Vec<ScreenInfo> {
    if on_x11() { xw::screens() } else { Vec::new() }
}

pub fn primary_screen_height() -> f64 {
    xw::primary_screen_height()
}

pub fn mouse_location() -> Point {
    xw::mouse_location()
}

pub fn mouse_button_down() -> bool {
    xw::mouse_button_down()
}

/// Only an X11 session can tell. Elsewhere the overlay stays.
pub fn frontmost_app_is_fullscreen() -> bool {
    session().x11_reaches_all_apps() && xw::active_window_is_fullscreen()
}

/// Linux has no global click monitor. The popover closes when it loses focus.
pub fn install_global_click_monitor(_callback: impl Fn(Point) + 'static) -> MonitorToken {
    MonitorToken
}

// Tray ----------------------------------------------------------------------

use super::TrayEvent;

/// The tray icon (StatusNotifierItem).
pub struct Tray {
    _icon: sayso_platform_linux::tray::Tray,
    /// The last click, in X11 pixels.
    last_click: std::sync::Arc<parking_lot::Mutex<Option<(i32, i32)>>>,
}

impl Tray {
    /// Show the icon. `on_event` runs on the tray's thread.
    pub fn install(on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Option<Tray> {
        let last_click = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let clicks = last_click.clone();
        let icon = sayso_platform_linux::tray::Tray::install(tray_image(), move |event| {
            use sayso_platform_linux::tray::TrayEvent as E;
            match event {
                E::Click { x, y } => {
                    *clicks.lock() = (x != 0 || y != 0).then_some((x, y));
                    on_event(TrayEvent::Click);
                }
                E::ToggleDictation => on_event(TrayEvent::ToggleDictation),
                E::OpenHub => on_event(TrayEvent::OpenHub),
                E::Quit => on_event(TrayEvent::Quit),
            }
        })?;
        Some(Tray { _icon: icon, last_click })
    }

    /// A small rectangle around the last click on the icon, when the panel
    /// said where it was. Only X11 can use it.
    pub fn icon_rect(&self) -> Option<Rect> {
        if !on_x11() {
            return None;
        }
        let (x, y) = (*self.last_click.lock())?;
        let scale = xw::scale();
        let (x, y) = (f64::from(x) / scale, xw::primary_screen_height() - f64::from(y) / scale);
        Some(Rect { x: x - 12.0, y: y - 12.0, width: 24.0, height: 24.0 })
    }
}

/// The colored app icon: a template image like the macOS one would vanish on
/// a dark panel.
fn tray_image() -> Option<(Vec<u8>, u32, u32)> {
    let bytes = sayso_ui::assets::bytes("app/icon-64.png").or_else(|| sayso_ui::assets::bytes("app/tray.png"))?;
    let img = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

// Files and system ----------------------------------------------------------

/// Open a file, a folder, or a web page with its default app.
pub fn open(target: impl AsRef<OsStr>) {
    sayso_platform_linux::permissions::open_uri(session(), target);
}

/// Show a file in the file manager: open its folder.
pub fn reveal(path: &Path) {
    open(path.parent().unwrap_or(path));
}

/// A command that plays a WAV file given as its last argument: PipeWire,
/// PulseAudio, or ALSA, whichever is installed.
pub fn audio_player() -> Option<std::process::Command> {
    ["pw-play", "paplay", "aplay"].into_iter().find(|p| on_path(p)).map(std::process::Command::new)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The user's name for the greeting: the full name from the passwd database
/// (the first GECOS field), or the username when the account has none.
pub fn user_full_name() -> Option<String> {
    // SAFETY: getuid has no failure mode.
    let uid = unsafe { libc::getuid() };
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    passwd_name(&passwd, uid).or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()).map(|u| capitalized(&u)))
}

/// The name of `uid` in passwd text: the full name, else the username with a
/// capital first letter ("ana" gives "Ana").
pub fn passwd_name(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.get(2)?.parse::<u32>().ok()? != uid {
            return None;
        }
        let full = fields.get(4).and_then(|gecos| gecos.split(',').next()).map(str::trim).filter(|n| !n.is_empty());
        let login = fields.first().map(|l| l.trim()).filter(|l| !l.is_empty());
        full.map(str::to_string).or_else(|| login.map(capitalized))
    })
}

fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Every model in the Linux catalog runs on the CPU.
pub fn model_runs_here(_info: &ModelInfo) -> bool {
    true
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's own `test` macro into scope.
    use super::passwd_name;

    #[test]
    fn name_is_the_full_name_or_the_username() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nana:x:1000:1000:Ana Lima,,,:/home/ana:/bin/bash\nbob:x:1001:1001::/home/bob:/bin/sh\n";
        assert_eq!(passwd_name(passwd, 1000).as_deref(), Some("Ana Lima"));
        assert_eq!(passwd_name(passwd, 1001).as_deref(), Some("Bob"), "an empty GECOS gives the username");
        assert_eq!(passwd_name(passwd, 4242), None);
    }
}
