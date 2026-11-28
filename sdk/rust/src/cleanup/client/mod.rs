//! Runs cleanup models for an app: the client of the cleanup runtime
//! (`native/cleanup`), the [`Cleaner`](crate::cleanup::models::Cleaner) on top of
//! it, and the download of the model files. See `docs/cleanup-models.md`.

pub mod cleaner;
pub mod download;
pub mod runtime;
