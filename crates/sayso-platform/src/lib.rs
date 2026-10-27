//! Platform seams (plan §4, "Platform traits").
//!
//! `sayso-platform-macos` implements these. Linux and Windows add their own
//! crates later. Only `sayso-app` depends on an implementation crate.
//!
//! All methods may be called from any thread unless the doc says "main thread".

use crossbeam_channel::{Receiver, RecvTimeoutError};
use sayso_core::hotkey::Hotkey;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::stt::{
    Diarization, EngineEvent, ModelStatus, SessionOptions, SpeechPart, TimedTranscript, Transcript,
};
use std::time::{Duration, Instant};

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

/// The error of a speech engine that does not have an optional part of the
/// engine protocol, for example `vad` (see `SttBackend::features`).
pub fn unsupported_feature(feature: &str) -> PlatformError {
    PlatformError::Failed(format!("this speech engine does not support {feature}"))
}

// ---------------------------------------------------------------------------
// Hotkeys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBindings {
    pub toggle: Option<Hotkey>,
    pub push_to_talk: Option<Hotkey>,
    pub paste_last: Option<Hotkey>,
    pub cycle_style: Option<Hotkey>,
    pub incognito: Option<Hotkey>,
    /// Enter while recording sends `Submit`, and no app gets that Enter.
    pub enter_sends: bool,
    /// Esc while recording goes to no app.
    pub take_escape: bool,
    /// Cancel on one Esc instead of two.
    pub single_escape: bool,
    pub double_escape_window: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Toggle,
    PushToTalkDown,
    PushToTalkUp,
    /// Push-to-talk released, and Enter once the text is in: dictate a chat message and send it.
    PushToTalkUpSubmit,
    /// Esc (once or twice, per the bindings) while recording.
    Cancel,
    /// Enter while recording, with `enter_sends` on: stop, insert, and press Enter.
    Submit,
    PasteLast,
    CycleStyle,
    /// Turn incognito on or off.
    ToggleIncognito,
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
    /// While true, Esc presses produce `Cancel`, and with `enter_sends` an
    /// Enter produces `Submit`. With `take_escape`, no app gets those Esc
    /// presses. Main thread.
    fn set_recording(&self, recording: bool);
    fn events(&self) -> Receiver<HotkeyEvent>;
    fn conflicts(&self, hotkey: &Hotkey) -> Vec<HotkeyConflict>;
    /// The app holding Secure Event Input (a password field), if any. For
    /// diagnostics only: it is system-wide, so it must not block a dictation.
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
    /// `clipboard_restored` is the plan: the old clipboard comes back after
    /// the call, when the app stops reading. A copy made before then wins.
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
    /// The app in front runs as administrator, so Windows does not let Sayso
    /// send keys to it. The name of the app, when Sayso can read it.
    #[error("{} runs as administrator, so Windows does not let Sayso type into it.", .app.as_deref().unwrap_or("This app"))]
    Elevated { app: Option<String> },
    /// Sayso cannot put the text on the clipboard. The text of the system.
    #[error("Sayso cannot use the clipboard: {0}")]
    Clipboard(String),
    /// A step of the system failed. The text is a detail for the log and
    /// for a bug report.
    #[error("{0}")]
    Failed(String),
}

pub trait TextInserter: Send + Sync {
    fn insert(&self, text: &str, method: InsertMethod) -> Result<InsertResult, InsertError>;
    fn copy_to_clipboard(&self, text: &str);
    /// The parts of the system that do the insertion, for the diagnostics of
    /// a dictation. None on a system that has one way only.
    fn backend(&self) -> Option<String> {
        None
    }
    /// Insert, then press Enter once the text is in. A failed Enter still counts as inserted.
    fn insert_and_submit(&self, text: &str, method: InsertMethod) -> Result<InsertResult, InsertError> {
        let result = self.insert(text, method)?;
        let _ = self.press_enter();
        Ok(result)
    }
    /// Press Enter in the focused app.
    fn press_enter(&self) -> Result<(), InsertError> {
        Err(InsertError::Failed("Sayso cannot press Enter on this system".into()))
    }
}

pub trait ContextProvider: Send + Sync {
    fn frontmost_app(&self) -> Option<AppInfo>;
    /// Bundle ids of running apps (for hotkey conflict hints).
    fn running_apps(&self) -> Vec<AppInfo>;
    /// PNG bytes of an app icon, for History rows.
    fn app_icon_png(&self, bundle_id: &str, size: u32) -> Option<Vec<u8>>;
    /// The apps on this computer, for the picker of a style rule. A system
    /// that cannot list them gives the apps that run now.
    fn installed_apps(&self) -> Vec<AppInfo> {
        self.running_apps()
    }
    /// True when [`ContextProvider::page_url`] can give an address on this system.
    fn reads_page_url(&self) -> bool {
        false
    }
    /// The address of the web page in the focused window of `app`, for an
    /// app that shows one. It can wait on the app for a short time, so do
    /// not call it on the main thread.
    fn page_url(&self, _app: &AppInfo) -> Option<String> {
        None
    }
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
    /// Start capture. `on_frame` runs every ~20 ms on a worker thread, not the audio
    /// callback, so a slow `on_frame` delays frames but does not drop them.
    fn start(
        &self,
        device_id: Option<&str>,
        on_frame: Box<dyn FnMut(AudioFrame) + Send>,
    ) -> Result<Box<dyn CaptureHandle>>;
}

// ---------------------------------------------------------------------------
// Speech engine
// ---------------------------------------------------------------------------

/// Readiness of an optional model part. A failed part can still load on demand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PartReadiness {
    #[default]
    Unprepared,
    Preparing,
    Ready,
    Failed,
}

pub trait SttBackend: Send + Sync {
    /// Prepare optional parts. Older backends can leave them lazy.
    fn prepare(&self, _model: &ModelId, _preview: bool, _vocabulary: &[String]) -> Result<()> {
        Ok(())
    }
    /// `preview` selects the preview part; false selects the dictionary helper.
    fn part_readiness(&self, _model: &ModelId, _preview: bool) -> PartReadiness {
        PartReadiness::Unprepared
    }

    fn catalog(&self) -> Vec<ModelInfo>;
    fn status(&self, model: &ModelId) -> ModelStatus;
    /// Start a download. Progress arrives as `EngineEvent::ModelStatus`.
    fn download(&self, model: &ModelId) -> Result<()>;
    fn delete(&self, model: &ModelId) -> Result<()>;
    /// Load into memory (and compile for the Neural Engine on first use). Blocks.
    fn load(&self, model: &ModelId) -> Result<()>;
    /// Take a loaded model out of memory. Its files stay on disk. Returns at
    /// once, and a request sent later runs after it. The status arrives as
    /// `EngineEvent::ModelStatus`.
    ///
    /// With `keep_preview`, an engine that holds the final pass and the live
    /// preview of the model as two parts unloads only the final pass. The
    /// status is `Downloaded` in both cases; see `preview_ready`.
    fn unload(&self, model: &ModelId, keep_preview: bool) -> Result<()>;
    /// True when a stream of this model can start although the model is not
    /// `Ready`: its live preview stayed in memory after an `unload`.
    fn preview_ready(&self, model: &ModelId) -> bool;
    /// Open a live preview stream. Partials arrive as `EngineEvent::Partial`.
    fn start_stream(&self, session: u64, options: &SessionOptions) -> Result<()>;
    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<()>;
    fn end_stream(&self, session: u64);
    /// Final pass on complete audio. Blocks until the text is ready.
    fn transcribe(&self, samples: &[f32], options: &SessionOptions) -> Result<Transcript>;
    /// The optional parts of the engine protocol that the engine has: the
    /// `features` of its `hello`, for example `segments`, `vad`, or
    /// `diarization`. Empty when the backend does not know, or has none.
    fn features(&self) -> Vec<String> {
        Vec::new()
    }
    /// Final pass on complete audio, with the text cut into timed segments.
    /// Needs the feature `segments`. Blocks until the text is ready.
    fn transcribe_segments(&self, _samples: &[f32], _options: &SessionOptions) -> Result<TimedTranscript> {
        Err(unsupported_feature("segments"))
    }
    /// Find the parts of complete audio with speech, with a loaded VAD model.
    /// Needs the feature `vad`. Blocks until the parts are ready.
    fn detect_speech(&self, _model: &ModelId, _samples: &[f32]) -> Result<Vec<SpeechPart>> {
        Err(unsupported_feature("vad"))
    }
    /// Open a VAD session on live audio, with a loaded VAD model. Needs the
    /// feature `vad`. Send the audio with [`SttBackend::push_audio`] and close
    /// the session with [`SttBackend::end_stream`]. The speech arrives as
    /// `EngineEvent::SpeechStart` and `EngineEvent::SpeechEnd`. Unlike the audio
    /// of a preview, VAD audio that waits in the queue is not dropped at the end.
    fn start_vad(&self, _session: u64, _model: &ModelId) -> Result<()> {
        Err(unsupported_feature("vad"))
    }
    /// Find who speaks when in complete audio, with a loaded diarization model.
    /// Needs the feature `diarization`. `speakers` is the number of speakers,
    /// when the caller knows it. Blocks, and it can take some time for a long
    /// recording.
    fn diarize(&self, _model: &ModelId, _samples: &[f32], _speakers: Option<u32>) -> Result<Diarization> {
        Err(unsupported_feature("diarization"))
    }
    /// A new receiver that gets every event from now on.
    fn subscribe(&self) -> Receiver<EngineEvent>;
    /// Wait until `model` is `Ready` or `Failed`, or until `deadline`, and
    /// return its status then. Each event wakes the wait, so it also follows
    /// the model through a restart of the engine. A backend without events
    /// gets a look at the status every 150 ms.
    fn wait_ready(&self, model: &ModelId, deadline: Instant) -> ModelStatus {
        // Subscribe first, so that no change between the look and the wait is lost.
        let events = self.subscribe();
        loop {
            let status = self.status(model);
            let left = deadline.saturating_duration_since(Instant::now());
            if matches!(status, ModelStatus::Ready | ModelStatus::Failed { .. }) || left.is_zero() {
                return status;
            }
            if let Err(RecvTimeoutError::Disconnected) = events.recv_timeout(left) {
                std::thread::sleep(left.min(Duration::from_millis(150)));
            }
        }
    }
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

/// A media player that [`MediaControl::pause_playing`] paused. The value
/// names the player for [`MediaControl::resume`] only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PausedPlayer(pub String);

/// The media players of other apps: music, video, and podcasts.
///
/// Both methods can wait on other processes, so do not call them on the main thread.
pub trait MediaControl: Send + Sync {
    /// Pause each player that plays now, and return the players that it paused.
    /// A player that does not play stays as it is.
    fn pause_playing(&self) -> Vec<PausedPlayer>;
    /// Play the players again. A player that closed in the meantime is skipped.
    fn resume(&self, players: &[PausedPlayer]);
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
    pub media: std::sync::Arc<dyn MediaControl>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_core::models::ModelInfo;

    /// A backend that implements only the required methods.
    struct Basic;

    impl SttBackend for Basic {
        fn catalog(&self) -> Vec<ModelInfo> {
            Vec::new()
        }
        fn status(&self, _model: &ModelId) -> ModelStatus {
            ModelStatus::NotDownloaded
        }
        fn download(&self, _model: &ModelId) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _model: &ModelId) -> Result<()> {
            Ok(())
        }
        fn load(&self, _model: &ModelId) -> Result<()> {
            Ok(())
        }
        fn unload(&self, _model: &ModelId, _keep_preview: bool) -> Result<()> {
            Ok(())
        }
        fn preview_ready(&self, _model: &ModelId) -> bool {
            false
        }
        fn start_stream(&self, _session: u64, _options: &SessionOptions) -> Result<()> {
            Ok(())
        }
        fn push_audio(&self, _session: u64, _samples: &[f32]) -> Result<()> {
            Ok(())
        }
        fn end_stream(&self, _session: u64) {}
        fn transcribe(&self, _samples: &[f32], options: &SessionOptions) -> Result<Transcript> {
            Ok(Transcript { text: String::new(), model: options.final_model.clone(), elapsed_ms: 0 })
        }
        fn subscribe(&self) -> Receiver<EngineEvent> {
            crossbeam_channel::unbounded().1
        }
    }

    #[test]
    fn a_backend_without_the_timing_features_says_so() {
        let backend = Basic;
        let model = ModelId::new("m");
        let options = SessionOptions {
            final_model: model.clone(),
            preview_model: None,
            language: "en".into(),
            vocabulary: Vec::new(),
        };
        assert!(backend.features().is_empty());
        let message = |error: PlatformError| error.to_string();
        assert_eq!(
            message(backend.transcribe_segments(&[0.0], &options).unwrap_err()),
            "this speech engine does not support segments"
        );
        assert_eq!(
            message(backend.detect_speech(&model, &[0.0]).unwrap_err()),
            "this speech engine does not support vad"
        );
        assert_eq!(
            message(backend.start_vad(1, &model).unwrap_err()),
            "this speech engine does not support vad"
        );
        assert_eq!(
            message(backend.diarize(&model, &[0.0], Some(2)).unwrap_err()),
            "this speech engine does not support diarization"
        );
    }
}
