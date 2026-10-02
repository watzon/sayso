//! Platform seams (plan §4, "Platform traits").
//!
//! `sayso-platform-macos` implements these. Linux and Windows add their own
//! crates later. Only `sayso-app` depends on an implementation crate.
//!
//! All methods may be called from any thread unless the doc says "main thread".

use crossbeam_channel::Receiver;
use sayso_core::hotkey::Hotkey;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions, Transcript};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("permission missing: {0:?}")]
    PermissionDenied(Permission),
    #[error("{0}")]
    Failed(String),
    #[error("not supported on this platform")]
    Unsupported,
}

pub type Result<T, E = PlatformError> = std::result::Result<T, E>;

// ---------------------------------------------------------------------------
// Hotkeys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBindings {
    pub toggle: Option<Hotkey>,
    pub push_to_talk: Option<Hotkey>,
    pub paste_last: Option<Hotkey>,
    pub cycle_style: Option<Hotkey>,
    /// Cancel on one Esc instead of two.
    pub single_escape: bool,
    pub double_escape_window: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Toggle,
    PushToTalkDown,
    PushToTalkUp,
    /// Esc (once or twice, per the bindings) while recording.
    Cancel,
    PasteLast,
    CycleStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictSource {
    /// A macOS keyboard shortcut, for example Spotlight.
    System { name: String, enabled: bool },
    /// A running app that uses this key, for example ChatGPT.
    /// `confirmed` is true when Sayso read the app's setting and it matches.
    /// False means the app uses this key by default, but its setting cannot be read.
    App { name: String, bundle_id: String, confirmed: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConflict {
    pub hotkey: Hotkey,
    pub source: ConflictSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyRegistration {
    /// A binding that could not be registered, with the reason.
    pub failed: Vec<(Hotkey, String)>,
    /// Push-to-talk needs Input Monitoring. True when it is missing.
    pub needs_input_monitoring: bool,
}

pub trait HotkeySource {
    /// Replace all bindings. Main thread.
    fn register(&self, bindings: &HotkeyBindings) -> HotkeyRegistration;
    /// While true, Esc presses produce `Cancel`. Main thread.
    fn set_recording(&self, recording: bool);
    fn events(&self) -> Receiver<HotkeyEvent>;
    fn conflicts(&self, hotkey: &Hotkey) -> Vec<HotkeyConflict>;
    /// The app holding Secure Event Input (a password field), if any.
    fn secure_input_holder(&self) -> Option<String>;
    /// Start recording the next key press for a recorder field. The next key
    /// combination (or held modifier) arrives on the returned channel. Main thread.
    fn capture_next(&self) -> Receiver<Hotkey>;
}

// ---------------------------------------------------------------------------
// Text insertion and context
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
    pub pid: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertMethod {
    /// Clipboard plus Cmd+V, then restore the old clipboard if it still holds
    /// our text. `restore_delay` counts from the last time the app read the text.
    Paste { restore_clipboard: bool, restore_delay: Duration },
    /// Synthetic key events. Slow for long text, but works where paste is blocked.
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertResult {
    Pasted { clipboard_restored: bool },
    Typed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InsertError {
    #[error("No text field is focused")]
    NoFocusedField,
    /// The paste key was sent, but no app read the text.
    #[error("The app did not take the text")]
    NotTaken,
    #[error("Sayso needs Accessibility permission to insert text")]
    NotTrusted,
    #[error("A password field is active")]
    SecureInput,
    #[error("{0}")]
    Failed(String),
}

pub trait TextInserter: Send + Sync {
    fn insert(&self, text: &str, method: InsertMethod) -> Result<InsertResult, InsertError>;
    fn copy_to_clipboard(&self, text: &str);
}

pub trait ContextProvider: Send + Sync {
    fn frontmost_app(&self) -> Option<AppInfo>;
    /// Bundle ids of running apps (for hotkey conflict hints).
    fn running_apps(&self) -> Vec<AppInfo>;
    /// PNG bytes of an app icon, for History rows.
    fn app_icon_png(&self, bundle_id: &str, size: u32) -> Option<Vec<u8>>;
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Microphone,
    Accessibility,
    InputMonitoring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
}

pub trait PermissionGuide: Send + Sync {
    fn status(&self, permission: Permission) -> PermissionState;
    /// Show the system prompt where macOS has one, else open System Settings.
    fn request(&self, permission: Permission);
    fn open_settings(&self, permission: Permission);
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// 16 kHz mono samples plus a level for the waveform.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub samples: Vec<f32>,
    /// RMS of this frame, 0.0 to 1.0, scaled for display.
    pub level: f32,
}

/// Stops capture when dropped.
pub trait CaptureHandle: Send {
    fn stop(self: Box<Self>);
}

pub trait AudioCapture: Send + Sync {
    fn devices(&self) -> Vec<AudioDevice>;
    /// Start capture. `on_frame` runs on an audio thread every ~20 ms. Keep it cheap.
    fn start(
        &self,
        device_id: Option<&str>,
        on_frame: Box<dyn FnMut(AudioFrame) + Send>,
    ) -> Result<Box<dyn CaptureHandle>>;
}

// ---------------------------------------------------------------------------
// Speech engine
// ---------------------------------------------------------------------------

pub trait SttBackend: Send + Sync {
    fn catalog(&self) -> Vec<ModelInfo>;
    fn status(&self, model: &ModelId) -> ModelStatus;
    /// Start a download. Progress arrives as `EngineEvent::ModelStatus`.
    fn download(&self, model: &ModelId) -> Result<()>;
    fn delete(&self, model: &ModelId) -> Result<()>;
    /// Load into memory (and compile for the Neural Engine on first use). Blocks.
    fn load(&self, model: &ModelId) -> Result<()>;
    /// Open a live preview stream. Partials arrive as `EngineEvent::Partial`.
    fn start_stream(&self, session: u64, options: &SessionOptions) -> Result<()>;
    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<()>;
    fn end_stream(&self, session: u64);
    /// Final pass on complete audio. Blocks until the text is ready.
    fn transcribe(&self, samples: &[f32], options: &SessionOptions) -> Result<Transcript>;
    /// A new receiver that gets every event from now on.
    fn subscribe(&self) -> Receiver<EngineEvent>;
}

// ---------------------------------------------------------------------------
// Small services
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    Start,
    Stop,
    Cancel,
    Insert,
}

pub trait SoundPlayer: Send + Sync {
    /// `volume` is 0.0 to 1.0. Must not block.
    fn play(&self, sound: SoundKind, volume: f32);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemState {
    Enabled,
    Disabled,
    /// macOS needs the user to approve it in System Settings.
    RequiresApproval,
}

pub trait LoginItem: Send + Sync {
    fn state(&self) -> LoginItemState;
    fn set_enabled(&self, enabled: bool) -> Result<LoginItemState>;
}

/// System appearance and accessibility settings the UI follows.
pub trait SystemPrefs: Send + Sync {
    fn dark_mode(&self) -> bool;
    fn reduce_motion(&self) -> bool;
}

/// Everything the app needs from the platform, in one place.
///
/// The thread-safe services are `Arc` so background tasks can hold them.
/// `hotkeys` stays on the main thread.
pub struct Platform {
    pub hotkeys: Box<dyn HotkeySource>,
    pub inserter: std::sync::Arc<dyn TextInserter>,
    pub context: std::sync::Arc<dyn ContextProvider>,
    pub permissions: std::sync::Arc<dyn PermissionGuide>,
    pub audio: std::sync::Arc<dyn AudioCapture>,
    pub sounds: std::sync::Arc<dyn SoundPlayer>,
    pub login_item: std::sync::Arc<dyn LoginItem>,
    pub prefs: std::sync::Arc<dyn SystemPrefs>,
}
