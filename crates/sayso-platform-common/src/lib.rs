//! The parts of the platform crates that need no system API of their own.
//!
//! `sayso-platform-macos`, `sayso-platform-linux`, and a Windows crate use
//! these modules, so audio capture, sounds, and the key logic behave the same
//! on every system.

pub mod audio;
pub mod esc;
pub mod paste_receipt;
pub mod resample;
pub mod sounds;
