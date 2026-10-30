//! Sayso domain logic.
//!
//! This crate must not depend on GPUI, on platform crates, or on other Sayso
//! crates, except the catalogs (`sayso-i18n`) and the MIT crates `sayso-ink`
//! and `sayso-speech`. A CI check enforces this (see `scripts/check-deps.sh`).

/// Basic VAD is in `sayso-speech`, so the jobs of the SDK use it too.
pub use sayso_speech::basic_vad;
pub mod cleanup;
pub mod config;
pub mod dictation;
pub mod dictionary;
pub mod engine_catalog;
pub mod enhance;
pub mod extensions;
pub mod flatpak;
pub mod history;
pub mod hotkey;
pub mod ink;
pub mod languages;
pub mod models;
pub mod model_store;
pub mod models_onnx;
pub mod paths;
pub mod pipeline;
/// The resampler is in `sayso-speech`, so the SDK reads files at any rate too.
pub use sayso_speech::resample;
pub mod speech;
mod speech_types;
pub mod spelled;
pub mod spoken_punctuation;
pub mod stats;
pub mod style;
pub mod style_rules;
pub mod stt;
