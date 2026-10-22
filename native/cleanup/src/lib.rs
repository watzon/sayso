//! `sayso-cleanup`: a local text-completion runtime.
//!
//! It knows no cleanup model. It loads one GGUF file with llama.cpp and
//! completes raw text with greedy decoding. The protocol types are in
//! `sayso_core::cleanup::protocol`. See `NOTES.md`.

pub mod runtime;
pub mod serve;
pub mod stdio;

pub use runtime::Runtime;
pub use serve::serve;
