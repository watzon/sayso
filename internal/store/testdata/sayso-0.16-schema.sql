-- The schema of sayso.db as the Rust app of 0.14 to 0.16 made it: the three
-- migrations of crates/sayso-store/src/schema.rs at v0.16.0, unchanged. The
-- app set PRAGMA user_version = 3 after them.

-- Version 1
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
    

-- Version 2
    ALTER TABLE history ADD COLUMN import_id TEXT;
    CREATE UNIQUE INDEX history_import_id ON history(import_id) WHERE import_id IS NOT NULL;
    

-- Version 3
    ALTER TABLE history ADD COLUMN diagnostics TEXT;
    
