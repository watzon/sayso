//! Capture the audio of this computer as 16 kHz mono frames, the format of
//! the speech engines (`docs/sdk.md`, "Crates").
//!
//! - [`mic`]: the microphone, with `cpal`. [`mic::start`] gives the frames of
//!   one input device to a function.
//!
//! The system audio (the sound of other apps and calls) comes in step 13 of
//! `docs/sdk.md` ("System audio"), as the module `system`. It gives frames of
//! the same format, with the same clock as the microphone.
//!
//! This crate gives the device and the frames. One capture for more than one
//! user (Sayso shares the microphone between a dictation and a live session)
//! is the work of the app.

pub mod mic;
