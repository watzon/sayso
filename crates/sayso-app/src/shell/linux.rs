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

/// The overlay: an X11 pop-up, or a layer surface at the bottom center. With
/// `at_top` (the notch), the layer surface is at the top center, on the edge
/// of the display over the panels.
pub fn overlay_kind(at_top: bool) -> WindowKind {
    if on_x11() {
        return WindowKind::PopUp;
    }
    WindowKind::LayerShell(LayerShellOptions {
        namespace: "sayso-overlay".into(),
        layer: Layer::Overlay,
        anchor: if at_top { Anchor::TOP } else { Anchor::BOTTOM },
        // -1: do not move away from the exclusive zone of a top panel.
        exclusive_zone: at_top.then_some(px(-1.)),
        keyboard_interactivity: KeyboardInteractivity::None,
        ..Default::default()
    })
}

/// The system does not keep windows out of a top bar, so there is nothing to do.
pub fn allow_over_menu_bar(_window: NativeWindow) {}

/// The popover: an X11 pop-up, or a layer surface over the whole screen with
/// the sheet at the top right.
pub fn popover_kind() -> WindowKind {
    if on_x11() {
        return WindowKind::PopUp;
    }
    WindowKind::LayerShell(LayerShellOptions {
        namespace: "sayso-popover".into(),
        layer: Layer::Overlay,
        anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        keyboard_interactivity: KeyboardInteractivity::OnDemand,
        ..Default::default()
    })
}

/// True when the popover window covers the screen. A Wayland app cannot see
/// a click in another app, and a layer surface does not always get the
/// keyboard focus, so the clear part of the popover takes the click that
/// closes it.
pub fn popover_covers_screen() -> bool {
    !on_x11()
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

/// The frame of the window that has the keyboard focus. Only an X11 session
/// can tell. Under XWayland, only X11 apps are known.
pub fn focused_window_frame() -> Option<Rect> {
    if on_x11() { xw::active_window_frame() } else { None }
}

/// Linux has no global click monitor. The popover closes when it loses
/// focus, and on Wayland also on a click on its clear part
/// ([`popover_covers_screen`]).
pub fn install_global_click_monitor(_callback: impl Fn(Point) + 'static) -> MonitorToken {
    MonitorToken
}

// Tray ----------------------------------------------------------------------

use super::TrayEvent;

/// The tray icon (StatusNotifierItem).
pub struct Tray {
    icon: sayso_platform_linux::tray::Tray,
    /// The last click, in X11 pixels.
    last_click: std::sync::Arc<parking_lot::Mutex<Option<(i32, i32)>>>,
}

impl Tray {
    /// Show the icon. `on_event` runs on the tray's thread.
    pub fn install(on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Option<Tray> {
        let last_click = std::sync::Arc::new(parking_lot::Mutex::new(None));
        let clicks = last_click.clone();
        let icon = sayso_platform_linux::tray::Tray::install(tray_image(false), tray_labels(), move |event| {
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
        Some(Tray { icon, last_click })
    }

    /// Gray out the icon while incognito is on.
    pub fn set_incognito(&self, on: bool) {
        self.icon.set_image(tray_image(on));
    }

    /// Show the menu labels again, in the current interface language.
    pub fn refresh_labels(&self) {
        self.icon.set_labels(tray_labels());
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

fn tray_labels() -> sayso_platform_linux::tray::TrayLabels {
    sayso_platform_linux::tray::TrayLabels {
        toggle_dictation: sayso_i18n::t!("tray.toggle_dictation"),
        open_hub: sayso_i18n::t!("tray.open_hub"),
        quit: sayso_i18n::t!("tray.quit"),
    }
}

/// The colored app icon: a template image like the macOS one would vanish on
/// a dark panel.
fn tray_image(incognito: bool) -> Option<(Vec<u8>, u32, u32)> {
    let bytes = sayso_ui::assets::bytes("app/icon-64.png").or_else(|| sayso_ui::assets::bytes("app/tray.png"))?;
    let mut img = image::load_from_memory(&bytes).ok()?.into_rgba8();
    if incognito {
        super::dim_icon(&mut img);
    }
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

/// A CPU model runs everywhere, a GPU model (`EngineKind::needs_gpu`) only with a discrete Vulkan GPU.
pub fn model_runs_here(info: &ModelInfo) -> bool {
    !info.engine.needs_gpu() || vulkan_gpu().is_some()
}

/// Environment for the engine: the GPU that `vulkan_gpu` chose, for ggml. ggml's own default is the first
/// GPU in the driver's list, which can be the integrated one. A user's own setting stays.
pub fn engine_env() -> Vec<(String, String)> {
    const VISIBLE: &str = "GGML_VK_VISIBLE_DEVICES";
    match vulkan_gpu() {
        Some(index) if std::env::var_os(VISIBLE).is_none() => vec![(VISIBLE.into(), index.to_string())],
        _ => Vec::new(),
    }
}

/// The Vulkan device for a GPU model, as its index in the driver's device list: the first discrete GPU.
/// On an integrated GPU Fluid Speech Mini ran about 3 times slower than Parakeet TDT v2 on the same CPU
/// (docs/linux.md), and Mesa's llvmpipe and lavapipe are CPU devices, slower still.
fn vulkan_gpu() -> Option<usize> {
    static GPU: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *GPU.get_or_init(|| {
        let kinds = vulkan_device_types().unwrap_or_default();
        let gpu = discrete_gpu(&kinds);
        log::info!("Vulkan devices {kinds:?}, GPU for GPU models: {gpu:?}");
        gpu
    })
}

fn discrete_gpu(kinds: &[ash::vk::PhysicalDeviceType]) -> Option<usize> {
    kinds.iter().position(|k| *k == ash::vk::PhysicalDeviceType::DISCRETE_GPU)
}

/// The type of each Vulkan device, in the order that ggml's `GGML_VK_VISIBLE_DEVICES` counts them.
fn vulkan_device_types() -> Option<Vec<ash::vk::PhysicalDeviceType>> {
    // SAFETY: loads the system's libvulkan.so.1, whose entry points ash checks as it loads them.
    let entry = unsafe { ash::Entry::load() }.ok()?;
    let app = ash::vk::ApplicationInfo::default().api_version(ash::vk::API_VERSION_1_0);
    // SAFETY: a valid create info with no layers or extensions; the instance is destroyed below.
    let instance = unsafe { entry.create_instance(&ash::vk::InstanceCreateInfo::default().application_info(&app), None) }.ok()?;
    // SAFETY: the instance is live; the device handles are used only before it is destroyed.
    let kinds = unsafe {
        let devices = instance.enumerate_physical_devices().unwrap_or_default();
        let kinds = devices.iter().map(|d| instance.get_physical_device_properties(*d).device_type).collect();
        instance.destroy_instance(None);
        kinds
    };
    Some(kinds)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's own `test` macro into scope.
    use super::{discrete_gpu, passwd_name};
    use ash::vk::PhysicalDeviceType as Kind;

    #[test]
    fn name_is_the_full_name_or_the_username() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nana:x:1000:1000:Ana Lima,,,:/home/ana:/bin/bash\nbob:x:1001:1001::/home/bob:/bin/sh\n";
        assert_eq!(passwd_name(passwd, 1000).as_deref(), Some("Ana Lima"));
        assert_eq!(passwd_name(passwd, 1001).as_deref(), Some("Bob"), "an empty GECOS gives the username");
        assert_eq!(passwd_name(passwd, 4242), None);
    }

    #[test]
    fn only_a_discrete_gpu_counts() {
        assert_eq!(discrete_gpu(&[Kind::INTEGRATED_GPU, Kind::DISCRETE_GPU]), Some(1));
        assert_eq!(discrete_gpu(&[Kind::DISCRETE_GPU, Kind::DISCRETE_GPU]), Some(0));
        assert_eq!(discrete_gpu(&[Kind::CPU, Kind::INTEGRATED_GPU]), None, "an integrated GPU only");
        assert_eq!(discrete_gpu(&[Kind::CPU]), None, "llvmpipe or lavapipe only");
        assert_eq!(discrete_gpu(&[Kind::VIRTUAL_GPU, Kind::OTHER]), None);
        assert_eq!(discrete_gpu(&[]), None);
    }
}
