//! Sayso storage: SQLite for history and the dictionary, FLAC files for audio,
//! and retention jobs (plan §3, "History and retention").
//!
//! [`Store`] is the only entry point. It is `Send + Sync`: the SQLite
//! connection sits behind a mutex, and every method takes the lock for the
//! shortest time it can. Calls block, so run them off the UI thread.
//!
//! The code is split by topic. Each module adds methods to [`Store`]:
//! - `history`: entries, full-text search, filters, stats input.
//! - `audio`: FLAC encode and decode, audio file paths.
//! - `dictionary`: words and replacement rules.
//! - `retention`: the two expiry limits for text and audio.

mod audio;
mod dictionary;
mod error;
mod history;
mod retention;
mod schema;

pub use error::{Result, StoreError};
pub use history::{AppCount, HistoryQuery};
pub use retention::RetentionReport;

use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Sample rate of all stored audio, in Hz.
pub const AUDIO_SAMPLE_RATE: u32 = 16_000;

/// The Sayso database and audio directory.
pub struct Store {
    conn: Mutex<Connection>,
    audio_dir: PathBuf,
}

impl Store {
    /// Open the database at `db_path`, creating it and `audio_dir` when they
    /// do not exist, and migrate it to the current schema.
    ///
    /// Fails with [`StoreError::SchemaTooNew`] when a newer Sayso wrote the
    /// database. Sayso never downgrades a schema.
    pub fn open(db_path: impl AsRef<Path>, audio_dir: impl AsRef<Path>) -> Result<Store> {
        let db_path = db_path.as_ref();
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::create_dir_all(audio_dir.as_ref())?;
        let mut conn = Connection::open(db_path)?;
        schema::configure(&conn)?;
        schema::migrate(&mut conn)?;
        Ok(Store { conn: Mutex::new(conn), audio_dir: audio_dir.as_ref().to_path_buf() })
    }

    /// The directory that holds the audio files.
    pub fn audio_dir(&self) -> &Path {
        &self.audio_dir
    }

    /// The schema version of the open database. Mostly useful in tests.
    pub fn schema_version(&self) -> Result<u32> {
        schema::version(&self.conn.lock())
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use sayso_core::history::{EnhanceOutcome, HistoryEntry, InsertOutcome, TargetApp};
    use sayso_core::models::ModelId;

    /// A store in a temporary directory. Keep the guard alive for the test.
    pub fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("sayso.db"), dir.path().join("audio")).unwrap();
        (dir, store)
    }

    pub fn base_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap()
    }

    /// An entry that is `age` older than [`base_time`].
    pub fn entry(text: &str, age: Duration) -> HistoryEntry {
        HistoryEntry {
            id: 0,
            created_at: base_time() - age,
            duration_ms: 2_000,
            app: None,
            transcript: text.into(),
            final_text: text.into(),
            style_id: "clean".into(),
            model: ModelId::new("parakeet"),
            transcribe_ms: 80,
            replacements: Vec::new(),
            enhance: EnhanceOutcome::NotUsed,
            insert: InsertOutcome::Typed,
            audio_file: None,
            waveform: vec![1, 2, 3],
        }
    }

    pub fn app(bundle_id: &str, name: &str) -> Option<TargetApp> {
        Some(TargetApp { bundle_id: bundle_id.into(), name: name.into() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use chrono::Duration;

    #[test]
    fn store_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Store>();
    }

    #[test]
    fn creates_the_database_from_scratch_and_reopens_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("nested/dir/sayso.db");
        let audio = dir.path().join("nested/audio");
        let id = {
            let store = Store::open(&db, &audio).unwrap();
            assert_eq!(store.schema_version().unwrap(), schema::CURRENT_VERSION);
            assert!(audio.is_dir());
            store.add_word("Sayso").unwrap();
            store.insert_entry(&entry("kept across restarts", Duration::zero())).unwrap()
        };
        let store = Store::open(&db, &audio).unwrap();
        assert_eq!(store.schema_version().unwrap(), schema::CURRENT_VERSION);
        assert_eq!(store.get(id).unwrap().unwrap().final_text, "kept across restarts");
        assert_eq!(store.list_words().unwrap().len(), 1);
        let hits = store.count(&HistoryQuery { text: Some("restarts".into()), ..Default::default() }).unwrap();
        assert_eq!(hits, 1, "the search index survives a reopen");
    }

    #[test]
    fn uses_wal_mode() {
        let (_dir, store) = temp_store();
        let mode: String = store.conn.lock().query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn refuses_a_database_from_a_newer_sayso() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sayso.db");
        Connection::open(&db).unwrap().pragma_update(None, "user_version", 99).unwrap();
        match Store::open(&db, dir.path().join("audio")) {
            Err(StoreError::SchemaTooNew { found: 99, .. }) => {}
            other => panic!("expected SchemaTooNew, got {:?}", other.map(|_| ())),
        }
    }
}
