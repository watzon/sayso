//! Windows implementations of the `sayso-platform` traits.
//!
//! The crate is empty on other platforms, so `cargo test --workspace` works there.
#![cfg(windows)]

pub mod audio;
pub mod context;
pub mod esc;
pub mod login_item;
pub mod paste_receipt;
pub mod permissions;
pub mod prefs;
pub mod resample;
pub mod sounds;
pub mod win32;
