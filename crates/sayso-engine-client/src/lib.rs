//! Client for the `SaysoEngine` speech sidecar (plan §4, "Engine protocol").
//!
//! [`EngineClient`] spawns the Swift sidecar, talks NDJSON to it over stdio, and
//! implements [`sayso_platform::SttBackend`]. It restarts the sidecar after a crash
//! and loads the models again. The protocol is in `docs/engine-protocol.md`.

mod client;
mod protocol;
mod writer;

pub use client::{BeforeLaunch, EngineClient, Options};
