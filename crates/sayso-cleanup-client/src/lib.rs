//! Runs cleanup models for the app: the client of the cleanup runtime
//! (`native/cleanup`), the [`Cleaner`](sayso_core::cleanup::Cleaner) on top of
//! it, and the download of the model files. See `docs/cleanup-models.md`.

pub mod cpu;
pub mod download;
pub mod cleaner;
pub mod runtime;
