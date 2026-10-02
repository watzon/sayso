//! Client for the `SaysoEngine` speech sidecar (plan §4, "Engine protocol").
//!
//! [`EngineClient`] spawns the Swift sidecar, talks NDJSON to it over stdio, and
//! implements [`sayso_platform::SttBackend`]. It restarts the sidecar after a crash
//! and loads the models again. See `NOTES.md` for the protocol reference.

mod client;
mod protocol;

pub use client::{EngineClient, Options};
