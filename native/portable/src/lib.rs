//! SaysoEngine for Windows and Linux: the speech sidecar of Sayso, speaking
//! the NDJSON protocol v1 of `crates/sayso-engine-client` on stdin and stdout.
//!
//! Parakeet runs through ONNX Runtime and Whisper through whisper.cpp, both
//! with transcribe-rs. See `NOTES.md`.

pub mod audio;
pub mod download;
pub mod engine;
pub mod preview;
pub mod protocol;
pub mod recognizer;
pub mod stdio;
pub mod store;
