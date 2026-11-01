//! Local cleanup models. The knowledge is in `sayso-cleanup-models`, and the
//! client of the runtime and the download are in `sayso-cleanup-client`.
//! S1-mini must always show as "S1-mini" by "Superwhisper".

pub use sayso_cleanup_client::cleaner::RuntimeCleaner;
pub use sayso_cleanup_client::download;
pub use sayso_cleanup_client::runtime::{Runtime, RuntimeError};
pub use sayso_cleanup_models::*;
