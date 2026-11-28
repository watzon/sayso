//! Capture the audio of this computer as 16 kHz mono frames, the format of
//! the speech engines. The types of a source that an app gives itself (with
//! `Options::capture`) are always here; the captures need the feature `capture`.
//!
//! - [`mic`]: the microphone, with `cpal`. [`mic::start`] gives the frames of
//!   one input device to a function.
//! - [`permission`]: the permission of the system to use the microphone.
//! - [`system`]: the system audio (the sound of other apps and calls).
//!   [`system::start`] gives its frames and what happens to the capture to a function.
//! - [`live`]: both as the sources of a live session of `sayso-speech`.
//! - [`clock`]: the clock of the times of the frames.
//!
//! This crate gives the device and the frames. One capture for more than one
//! user (Sayso shares the microphone between a dictation and a live session)
//! is the work of the app.

pub use crate::speech::live::{
    AudioFeed, CapturedAudio, FeedItem, Microphone, OpenError, SourceEnd, SystemAudio,
};

#[cfg(feature = "capture")]
pub mod clock;
#[cfg(feature = "capture")]
pub mod live;
#[cfg(feature = "capture")]
pub mod mic;
#[cfg(feature = "capture")]
pub mod permission;
#[cfg(feature = "capture")]
pub mod system;
