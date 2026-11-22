// Package store is Sayso's storage: SQLite for history and the dictionary
// (pure Go, no cgo), FLAC files for audio, retention, and the Pindrop import
// (crates/sayso-store). The schema is the Rust one, so both apps open the
// same sayso.db.
package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"log/slog"
	"net/url"
	"os"
	"path/filepath"
	"sync"

	_ "modernc.org/sqlite"
)

// Migrations: migrations[n] moves the database from version n to n+1. Never
// edit a released migration; add a new one at the end.
var migrations = []string{
	// Version 1: history with full-text search, and the dictionary.
	`
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
    `,
	// Version 2: where an imported entry came from ("pindrop:<record id>").
	`
    ALTER TABLE history ADD COLUMN import_id TEXT;
    CREATE UNIQUE INDEX history_import_id ON history(import_id) WHERE import_id IS NOT NULL;
    `,
	// Version 3: how the dictation ran (JSON Diagnostics).
	`
    ALTER TABLE history ADD COLUMN diagnostics TEXT;
    `,
}

// CurrentVersion is the newest schema version this build knows.
var CurrentVersion = len(migrations)

// AudioSampleRate is the sample rate of all stored audio.
const AudioSampleRate = 16_000

// ErrNotFound is a missing row.
var ErrNotFound = errors.New("no row")

// SchemaTooNewError: a newer Sayso wrote the database. Sayso never downgrades.
type SchemaTooNewError struct{ Found, Supported int }

func (e *SchemaTooNewError) Error() string {
	return fmt.Sprintf("the database has schema version %d, but this Sayso supports up to %d", e.Found, e.Supported)
}

// InvalidInputError is input the store refuses.
type InvalidInputError struct{ Msg string }

func (e *InvalidInputError) Error() string { return "invalid input: " + e.Msg }

// StorageUse is what the store holds on disk.
type StorageUse struct {
	Entries       int
	DatabaseBytes uint64
	AudioFiles    int
	AudioBytes    uint64
}

// Store is the database and the audio folder. Safe for concurrent use; calls
// block, so run them off the UI thread.
type Store struct {
	db       *sql.DB
	mu       sync.Mutex
	audioDir string
}

func dsn(path string) string {
	q := url.Values{}
	for _, p := range []string{"foreign_keys(1)", "synchronous(NORMAL)", "busy_timeout(5000)"} {
		q.Add("_pragma", p)
	}
	return "file:" + filepath.ToSlash(path) + "?" + q.Encode()
}

// Open opens the database at dbPath (making it and audioDir when missing) and
// migrates it to the current schema.
func Open(dbPath, audioDir string) (*Store, error) {
	if err := os.MkdirAll(filepath.Dir(dbPath), 0o755); err != nil {
		return nil, err
	}
	if err := os.MkdirAll(audioDir, 0o755); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", dsn(dbPath))
	if err != nil {
		return nil, err
	}
	// One connection, as the Rust store keeps one connection behind a mutex.
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)
	db.SetConnMaxLifetime(0)
	var mode string
	if err := db.QueryRow("PRAGMA journal_mode = WAL").Scan(&mode); err != nil {
		db.Close()
		return nil, fmt.Errorf("database error: %w", err)
	}
	s := &Store{db: db, audioDir: audioDir}
	if err := s.migrate(); err != nil {
		db.Close()
		return nil, err
	}
	return s, nil
}

func (s *Store) Close() error { return s.db.Close() }

// AudioDir is the folder of the audio files.
func (s *Store) AudioDir() string { return s.audioDir }

// SchemaVersion is the schema version of the open database.
func (s *Store) SchemaVersion() (int, error) {
	var v int
	err := s.db.QueryRow("PRAGMA user_version").Scan(&v)
	return v, err
}

func (s *Store) migrate() error {
	found, err := s.SchemaVersion()
	if err != nil {
		return err
	}
	if found > CurrentVersion {
		return &SchemaTooNewError{found, CurrentVersion}
	}
	for i := found; i < len(migrations); i++ {
		tx, err := s.db.BeginTx(context.Background(), nil)
		if err != nil {
			return err
		}
		if _, err := tx.Exec(migrations[i]); err != nil {
			tx.Rollback()
			return fmt.Errorf("database error: migration %d: %w", i+1, err)
		}
		if _, err := tx.Exec(fmt.Sprintf("PRAGMA user_version = %d", i+1)); err != nil {
			tx.Rollback()
			return err
		}
		if err := tx.Commit(); err != nil {
			return err
		}
		slog.Info("migrated the database", "version", i+1)
	}
	return nil
}
