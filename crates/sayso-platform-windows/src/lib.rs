//! Windows implementations of the `sayso-platform` traits.
//!
//! TEMPORARY STUB: the services are being written separately. Only `window`
//! is real here.
#![cfg(windows)]

pub mod window;

use sayso_platform::*;
use std::path::PathBuf;
use std::sync::Arc;

pub struct WinPlatform;

struct Stub(crossbeam_channel::Receiver<HotkeyEvent>);

impl HotkeySource for Stub {
    fn register(&self, _: &HotkeyBindings) -> HotkeyRegistration {
        HotkeyRegistration { failed: vec![], needs_input_monitoring: false }
    }
    fn set_recording(&self, _: bool) {}
    fn events(&self) -> crossbeam_channel::Receiver<HotkeyEvent> {
        self.0.clone()
    }
    fn conflicts(&self, _: &sayso_core::hotkey::Hotkey) -> Vec<HotkeyConflict> {
        vec![]
    }
    fn secure_input_holder(&self) -> Option<String> {
        None
    }
    fn capture_next(&self) -> crossbeam_channel::Receiver<sayso_core::hotkey::Hotkey> {
        crossbeam_channel::never()
    }
}

struct S;
impl TextInserter for S {
    fn insert(&self, _: &str, _: InsertMethod) -> Result<InsertResult, InsertError> {
        Err(InsertError::Failed("stub".into()))
    }
    fn copy_to_clipboard(&self, _: &str) {}
}
impl ContextProvider for S {
    fn frontmost_app(&self) -> Option<AppInfo> {
        None
    }
    fn running_apps(&self) -> Vec<AppInfo> {
        vec![]
    }
    fn app_icon_png(&self, _: &str, _: u32) -> Option<Vec<u8>> {
        None
    }
}
impl PermissionGuide for S {
    fn status(&self, _: Permission) -> PermissionState {
        PermissionState::Granted
    }
    fn request(&self, _: Permission) {}
    fn open_settings(&self, _: Permission) {}
}
impl AudioCapture for S {
    fn devices(&self) -> Vec<AudioDevice> {
        vec![]
    }
    fn start(&self, _: Option<&str>, _: Box<dyn FnMut(AudioFrame) + Send>) -> sayso_platform::Result<Box<dyn CaptureHandle>> {
        Err(PlatformError::Unsupported)
    }
}
impl SoundPlayer for S {
    fn play(&self, _: SoundKind, _: f32) {}
}
impl LoginItem for S {
    fn state(&self) -> LoginItemState {
        LoginItemState::Disabled
    }
    fn set_enabled(&self, _: bool) -> sayso_platform::Result<LoginItemState> {
        Err(PlatformError::Unsupported)
    }
}
impl SystemPrefs for S {
    fn dark_mode(&self) -> bool {
        false
    }
    fn reduce_motion(&self) -> bool {
        false
    }
}

impl WinPlatform {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(_sounds_dir: PathBuf) -> Platform {
        let (_tx, rx) = crossbeam_channel::unbounded();
        std::mem::forget(_tx);
        Platform {
            hotkeys: Box::new(Stub(rx)),
            inserter: Arc::new(S),
            context: Arc::new(S),
            permissions: Arc::new(S),
            audio: Arc::new(S),
            sounds: Arc::new(S),
            login_item: Arc::new(S),
            prefs: Arc::new(S),
        }
    }
}
