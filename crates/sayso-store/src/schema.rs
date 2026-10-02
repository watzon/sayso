//! Connection settings and schema migrations.
//!
//! The schema version lives in `PRAGMA user_version`. `MIGRATIONS[n]` moves the
//! database from version `n` to `n + 1`. Never edit a released migration: add a
//! new one at the end.

use crate::error::{Result, StoreError};
use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    // Version 1: history with full-text search, and the dictionary.
    r#"
    CREATE TABLE history (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        created_at   INTEGER NOT NULL,           -- Unix time in milliseconds
        duration_ms  INTEGER NOT NULL,
        app          TEXT,                       -- JSON TargetApp, or NULL
        transcript   TEXT NOT NULL,
        final_text   TEXT NOT NULL,
        style_id     TEXT NOT NULL,
        model        TEXT NOT NULL,
        transcribe_ms INTEGER NOT NULL,
        replacements TEXT NOT NULL,              -- JSON array of AppliedReplacement
        enhance      TEXT NOT NULL,              -- JSON EnhanceOutcome
        insert_result TEXT NOT NULL,             -- JSON InsertOutcome
        audio_file   TEXT,                       -- file name in the audio directory
        waveform     BLOB NOT NULL
    );
    CREATE INDEX history_created_at ON history(created_at);
    CREATE INDEX history_app ON history(json_extract(app, '$.bundle_id'));

    CREATE VIRTUAL TABLE history_fts USING fts5(
        transcript, final_text,
        content = 'history', content_rowid = 'id',
        tokenize = 'unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER history_ai AFTER INSERT ON history BEGIN
        INSERT INTO history_fts(rowid, transcript, final_text)
        VALUES (new.id, new.transcript, new.final_text);
    END;
    CREATE TRIGGER history_ad AFTER DELETE ON history BEGIN
        INSERT INTO history_fts(history_fts, rowid, transcript, final_text)
        VALUES ('delete', old.id, old.transcript, old.final_text);
    END;
    CREATE TRIGGER history_au AFTER UPDATE OF transcript, final_text ON history BEGIN
        INSERT INTO history_fts(history_fts, rowid, transcript, final_text)
        VALUES ('delete', old.id, old.transcript, old.final_text);
        INSERT INTO history_fts(rowid, transcript, final_text)
        VALUES (new.id, new.transcript, new.final_text);
    END;

    CREATE TABLE words (
        id   INTEGER PRIMARY KEY AUTOINCREMENT,
        text TEXT NOT NULL UNIQUE COLLATE NOCASE
    );
    CREATE TABLE replacements (
        id             INTEGER PRIMARY KEY AUTOINCREMENT,
        from_text      TEXT NOT NULL,
        to_text        TEXT NOT NULL,
        case_sensitive INTEGER NOT NULL DEFAULT 0,
        uses           INTEGER NOT NULL DEFAULT 0
    );
    "#,
    // Version 2: where an imported entry came from, for example
    // "pindrop:<record id>". A second import skips the entries it finds here.
    r#"
    ALTER TABLE history ADD COLUMN import_id TEXT;
    CREATE UNIQUE INDEX history_import_id ON history(import_id) WHERE import_id IS NOT NULL;
    "#,
];

/// The newest schema version this build knows.
pub const CURRENT_VERSION: u32 = MIGRATIONS.len() as u32;

/// Set the pragmas every connection needs.
pub fn configure(conn: &Connection) -> Result<()> {
    // `journal_mode` returns a row, so it cannot go through `execute_batch`.
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(())
}

pub fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Apply all missing migrations. Each one runs in its own transaction.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let found = version(conn)?;
    if found > CURRENT_VERSION {
        return Err(StoreError::SchemaTooNew { found, supported: CURRENT_VERSION });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as u32 + 1)?;
        tx.commit()?;
        log::info!("migrated the database to schema version {}", index + 1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_1_database_keeps_its_history_when_it_migrates() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO history (created_at, duration_ms, transcript, final_text, style_id, model, transcribe_ms, replacements, enhance, insert_result, waveform)
             VALUES (1, 2, 'raw', 'final', 'clean', 'parakeet', 3, '[]', '{}', '{}', x'')",
            [],
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        assert_eq!(version(&conn).unwrap(), CURRENT_VERSION);
        let (text, import_id): (String, Option<String>) =
            conn.query_row("SELECT final_text, import_id FROM history", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((text.as_str(), import_id), ("final", None), "an entry from before the import column has no import id");
    }
}
