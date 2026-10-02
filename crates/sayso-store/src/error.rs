//! The error type of the store.

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("stored JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("audio error: {0}")]
    Audio(String),
    #[error("no row with id {0}")]
    NotFound(i64),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("the database has schema version {found}, but this Sayso supports up to {supported}")]
    SchemaTooNew { found: u32, supported: u32 },
}
