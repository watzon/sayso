//! Local cleanup models. The knowledge is in [`models`], and the client of the
//! runtime and the download are in [`client`].
//! S1-mini must always show as "S1-mini" by "Superwhisper".

pub mod client;
pub mod models;

pub use client::cleaner::RuntimeCleaner;
pub use client::download;
pub use client::runtime::{Runtime, RuntimeError};
pub use models::*;
