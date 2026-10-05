//! Sayso domain logic.
//!
//! This crate must not depend on GPUI, on platform crates, or on other Sayso
//! crates. A CI check enforces this (see `scripts/check-deps.sh`).

pub mod config;
pub mod dictation;
pub mod dictionary;
pub mod enhance;
pub mod flatpak;
pub mod history;
pub mod hotkey;
pub mod ink;
pub mod languages;
pub mod models;
pub mod models_onnx;
pub mod paths;
pub mod pipeline;
pub mod speech;
pub mod spelled;
pub mod spoken_punctuation;
pub mod stats;
pub mod style;
pub mod stt;
