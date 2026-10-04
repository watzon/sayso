//! A backend that does nothing, for a system without its own backend yet.
//!
//! The app builds and its windows open, with portable audio capture and
//! sounds. Hotkeys, insertion, the tray, and window placement do nothing.
//! A port replaces this file with its own backend (see `mod.rs`).

use super::TrayEvent;
use crossbeam_channel::{Receiver, unbounded};
use gpui_kit::*;
use sayso_core::hotkey::Hotkey;
use sayso_core::models::ModelInfo;
use sayso_platform::*;
use sayso_platform_common::{audio, sounds};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenInfo {
    pub id: u32,
    pub frame: Rect,
    pub visible_frame: Rect,
    pub scale: f64,
    pub has_notch: bool,
}

pub struct MonitorToken;

pub type NativeWindow = ();

pub fn hand_off() {}
pub fn prepare() {}

pub fn open_requested() -> bool {
    false
}

pub fn platform(sounds_dir: PathBuf) -> Platform {
    Platform {
        hotkeys: Box::new(NoHotkeys(unbounded().1)),
        inserter: Arc::new(Nothing),
        context: Arc::new(Nothing),
        permissions: Arc::new(Nothing),
        audio: Arc::new(audio::CpalAudio::new(audio::always_granted)),
        sounds: Arc::new(sounds::RodioSounds::new(sounds_dir)),
        login_item: Arc::new(Nothing),
        prefs: Arc::new(Nothing),
    }
}

pub fn native(_window: &Window) -> Option<NativeWindow> {
    Some(())
}

pub fn become_accessory() {}
pub fn set_dock_icon_visible(_visible: bool) {}
pub fn show_app_and_activate() {}
pub fn deactivate_app() {}

pub fn configure_overlay(_window: NativeWindow) {}
pub fn make_never_key(_window: NativeWindow) {}
pub fn set_frame_origin(_window: NativeWindow, _x: f64, _y: f64) {}
pub fn set_frame(_window: NativeWindow, _x: f64, _y: f64, _width: f64, _height: f64) {}
pub fn frame(_window: NativeWindow) -> Option<Rect> {
    None
}
pub fn make_clear(_window: NativeWindow) {}
pub fn add_child_window(_parent: NativeWindow, _child: NativeWindow) {}
pub fn order_front_without_activating(_window: NativeWindow) {}
pub fn order_out(_window: NativeWindow) {}
pub fn show_and_focus(_window: NativeWindow) {}

pub fn overlay_kind() -> WindowKind {
    WindowKind::PopUp
}
pub fn popover_kind() -> WindowKind {
    WindowKind::PopUp
}

pub fn popup_kind(_parent: AnyWindowHandle, _anchor: Bounds<Pixels>, _offset: gpui_kit::Point<Pixels>) -> Option<WindowKind> {
    None
}
pub fn set_click_region(_window: &Window, _native: NativeWindow, _card: Bounds<Pixels>, _inside: bool) {}
pub fn can_place_windows() -> bool {
    false
}
pub fn can_hide_windows() -> bool {
    false
}

pub fn screens() -> Vec<ScreenInfo> {
    Vec::new()
}
pub fn primary_screen_height() -> f64 {
    1080.0
}
pub fn mouse_location() -> Point {
    Point { x: 0.0, y: 0.0 }
}
pub fn mouse_button_down() -> bool {
    false
}
pub fn frontmost_app_is_fullscreen() -> bool {
    false
}
pub fn focused_window_frame() -> Option<Rect> {
    None
}
pub fn install_global_click_monitor(_callback: impl Fn(Point) + 'static) -> MonitorToken {
    MonitorToken
}

pub struct Tray;

impl Tray {
    pub fn set_incognito(&self, _on: bool) {}

    pub fn install(_on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Option<Tray> {
        None
    }
    pub fn icon_rect(&self) -> Option<Rect> {
        None
    }
}

pub fn open(_target: impl AsRef<OsStr>) {}
pub fn reveal(_path: &Path) {}
pub fn audio_player() -> Option<std::process::Command> {
    None
}
pub fn user_full_name() -> Option<String> {
    None
}
pub fn model_runs_here(_info: &ModelInfo) -> bool {
    true
}

struct NoHotkeys(Receiver<HotkeyEvent>);

impl HotkeySource for NoHotkeys {
    fn register(&self, bindings: &HotkeyBindings) -> HotkeyRegistration {
        let failed = [bindings.toggle, bindings.push_to_talk, bindings.paste_last, bindings.cycle_style, bindings.incognito]
            .into_iter()
            .flatten()
            .map(|h| (h, "Hotkeys are not built for this system yet.".to_string()))
            .collect();
        HotkeyRegistration { failed, needs_input_monitoring: false }
    }
    fn set_recording(&self, _recording: bool) {}
    fn events(&self) -> Receiver<HotkeyEvent> {
        self.0.clone()
    }
    fn conflicts(&self, _hotkey: &Hotkey) -> Vec<HotkeyConflict> {
        Vec::new()
    }
    fn secure_input_holder(&self) -> Option<String> {
        None
    }
    fn capture_next(&self) -> Receiver<Hotkey> {
        unbounded().1
    }
}

struct Nothing;

impl TextInserter for Nothing {
    fn insert(&self, _text: &str, _method: InsertMethod) -> Result<InsertResult, InsertError> {
        Err(InsertError::Failed("Insertion is not built for this system yet.".into()))
    }
    fn copy_to_clipboard(&self, _text: &str) {}
}

impl ContextProvider for Nothing {
    fn frontmost_app(&self) -> Option<AppInfo> {
        None
    }
    fn running_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
    fn app_icon_png(&self, _bundle_id: &str, _size: u32) -> Option<Vec<u8>> {
        None
    }
}

impl PermissionGuide for Nothing {
    fn status(&self, _permission: Permission) -> PermissionState {
        PermissionState::Granted
    }
    fn request(&self, _permission: Permission) {}
    fn open_settings(&self, _permission: Permission) {}
}

impl LoginItem for Nothing {
    fn state(&self) -> LoginItemState {
        LoginItemState::Disabled
    }
    fn set_enabled(&self, _enabled: bool) -> sayso_platform::Result<LoginItemState> {
        Err(PlatformError::Unsupported)
    }
}

impl SystemPrefs for Nothing {
    fn dark_mode(&self) -> bool {
        false
    }
    fn reduce_motion(&self) -> bool {
        false
    }
}
