//! [`AudioCapture`] with the microphone of `sayso-capture` (`cpal`).
//!
//! The capture is in `sayso_capture::mic`: the audio callback converts the
//! samples, mixes to mono, resamples to 16 kHz, and cuts 20 ms frames, and a
//! worker thread calls `on_frame` with each frame. This module adds the
//! permission check of the system and the types of `sayso-platform`.

use sayso_capture::mic;
use sayso_platform::{AudioCapture, AudioDevice, AudioFrame, CaptureHandle, Permission, PermissionState, PlatformError, Result};

pub use sayso_capture::mic::{FRAME_SAMPLES, TARGET_RATE, display_level, downmix, rms};

/// Capture through the default `cpal` host.
pub struct CpalAudio {
    /// The microphone permission, checked before each start. A denied
    /// microphone gives silence instead of an error on some systems.
    microphone_status: fn() -> PermissionState,
}

impl CpalAudio {
    /// `microphone_status` reads the system's microphone permission. Pass
    /// [`always_granted`] on a system without one.
    pub fn new(microphone_status: fn() -> PermissionState) -> Self {
        Self { microphone_status }
    }
}

/// For systems that do not gate the microphone per app.
pub fn always_granted() -> PermissionState {
    PermissionState::Granted
}

fn platform_error(error: mic::Error) -> PlatformError {
    match error {
        mic::Error::PermissionDenied => PlatformError::PermissionDenied(Permission::Microphone),
        mic::Error::Failed(message) => PlatformError::Failed(message),
    }
}

/// Dropping the capture ends it.
struct Handle {
    _capture: mic::Capture,
}

impl CaptureHandle for Handle {
    fn stop(self: Box<Self>) {
        // Dropping the handle ends the capture.
    }
}

impl AudioCapture for CpalAudio {
    fn devices(&self) -> Vec<AudioDevice> {
        mic::devices().into_iter().map(|d| AudioDevice { id: d.id, name: d.name, is_default: d.is_default }).collect()
    }

    fn start(
        &self,
        device_id: Option<&str>,
        mut on_frame: Box<dyn FnMut(AudioFrame) + Send>,
    ) -> Result<Box<dyn CaptureHandle>> {
        // A denied microphone gives silence, not an error, so check first.
        if (self.microphone_status)() == PermissionState::Denied {
            return Err(PlatformError::PermissionDenied(Permission::Microphone));
        }
        let on_frame = Box::new(move |frame: mic::Frame| on_frame(AudioFrame { samples: frame.samples, level: frame.level }));
        let capture = mic::start(device_id, on_frame).map_err(platform_error)?;
        Ok(Box::new(Handle { _capture: capture }))
    }
}
