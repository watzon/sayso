//! Windows implementations of the `sayso-platform` traits.
//!
//! The crate is empty on other platforms, so `cargo test --workspace` works there.
#![cfg(windows)]

pub mod audio;
pub mod context;
pub mod esc;
pub mod hook;
pub mod hook_logic;
pub mod input;
pub mod inserter;
pub mod keymap;
pub mod login_item;
pub mod paste_receipt;
pub mod permissions;
pub mod prefs;
pub mod resample;
pub mod sounds;
#[cfg(test)]
mod test_window;
pub mod win32;
