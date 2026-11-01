//! Capture the audio of this computer as 16 kHz mono frames, the format of
//! the speech engines (`docs/sdk.md`, "Crates").
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

pub mod clock;
pub mod live;
pub mod mic;
pub mod system;
pub mod permission;
