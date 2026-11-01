//! The microphone of this computer as a [`Microphone`] of live sessions.

use crossbeam_channel::unbounded;
use parking_lot::Mutex;
use sayso_capture::mic;
use sayso_speech::live::{AudioFeed, CapturedAudio, FeedItem, Microphone, OpenError};
use std::sync::Arc;

/// Gets the display level of each frame, from 0 to 1.
pub type LevelFn = Arc<dyn Fn(f32) + Send + Sync>;

/// Captures one input device in the process of the app, with `sayso-capture`.
#[derive(Default)]
pub struct CaptureMicrophone {
    device: Mutex<Option<String>>,
    on_level: Mutex<Option<LevelFn>>,
    early: Mutex<Option<AudioFeed>>,
}

impl CaptureMicrophone {
    /// Use this input device from the next session on. None: the default device.
    pub fn set_device(&self, device: Option<String>) {
        *self.device.lock() = device;
    }

    /// Call `on_level` for each frame of the next sessions.
    pub fn set_level(&self, on_level: Option<LevelFn>) {
        *self.on_level.lock() = on_level;
    }
    /// Start the capture now, before the models of a session load. The next
    /// session gets this audio first, so the first words are not lost. A
    /// session that does not come needs [`CaptureMicrophone::close_early`].
    pub fn open_early(&self) -> Result<(), String> {
        let feed = self.start().map_err(|e| match e {
            OpenError::Permission(message)
            | OpenError::Unsupported(message)
            | OpenError::Failed(message) => message,
        })?;
        *self.early.lock() = Some(feed);
        Ok(())
    }

    /// Stop a capture from [`CaptureMicrophone::open_early`] that no session took.
    pub fn close_early(&self) {
        self.early.lock().take();
    }

    fn start(&self) -> Result<AudioFeed, OpenError> {
        let (tx, items) = unbounded();
        let on_level = self.on_level.lock().clone();
        let device = self.device.lock().clone();
        let capture = mic::start(
            device.as_deref(),
            Box::new(move |frame| {
                if let Some(on_level) = &on_level {
                    on_level(frame.level);
                }
                // The session ended. The capture stops soon.
                let _ = tx.send(FeedItem::Audio(CapturedAudio {
                    samples: frame.samples,
                    time: Some(frame.time),
                }));
            }),
        )
        .map_err(|e| match e {
            mic::Error::PermissionDenied => OpenError::Permission(e.to_string()),
            mic::Error::Failed(message) => OpenError::Failed(message),
        })?;
        Ok(AudioFeed {
            items,
            lease: Box::new(Lease(capture)),
            excludes: Vec::new(),
        })
    }
}

impl Microphone for CaptureMicrophone {
    fn open(&self) -> Result<AudioFeed, OpenError> {
        if let Some(feed) = self.early.lock().take() {
            return Ok(feed);
        }
        self.start()
    }
}

/// Dropping it stops the capture, and the channel closes after the last frame.
struct Lease(#[allow(dead_code)] mic::Capture);
