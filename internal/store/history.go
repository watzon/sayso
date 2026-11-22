package store

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"
)

const columns = "id, created_at, duration_ms, app, transcript, final_text, style_id, model, " +
	"transcribe_ms, replacements, enhance, insert_result, audio_file, waveform, diagnostics"

// valueColumns is columns without the id.
var valueColumns = columns[len("id, "):]

// HistoryQuery filters the history list.
type HistoryQuery struct {
	// Text: full-text search; the last word matches as a prefix.
	Text         string
	EnhancedOnly bool
	HasAudio     bool
	AppBundleID  string
	// Limit: 0 means no limit.
	Limit  int
	Offset int
}

// AppCount is an app of the history and its number of entries.
type AppCount struct {
	BundleID, Name string
	Count          int
}

func (q HistoryQuery) conditions() ([]string, []any) {
	var sqlParts []string
	var args []any
	if fts, ok := ftsQuery(q.Text); ok {
		sqlParts = append(sqlParts, "id IN (SELECT rowid FROM history_fts WHERE history_fts MATCH ?)")
		args = append(args, fts)
	}
	if q.EnhancedOnly {
		sqlParts = append(sqlParts, "json_extract(enhance, '$.result') = 'applied'")
	}
	if q.HasAudio {
		sqlParts = append(sqlParts, "audio_file IS NOT NULL")
	}
	if q.AppBundleID != "" {
		sqlParts = append(sqlParts, "json_extract(app, '$.bundle_id') = ?")
		args = append(args, q.AppBundleID)
	}
	return sqlParts, args
}

func whereClause(parts []string) string {
	if len(parts) == 0 {
		return ""
	}
	return " WHERE " + strings.Join(parts, " AND ")
}

// ftsQuery quotes each word, and makes the last one a prefix.
func ftsQuery(text string) (string, bool) {
	words := strings.Fields(text)
	if len(words) == 0 {
		return "", false
	}
	for i, w := range words {
		words[i] = `"` + strings.ReplaceAll(w, `"`, `""`) + `"`
	}
	words[len(words)-1] += "*"
	return strings.Join(words, " "), true
}

func millisToTime(ms int64) time.Time { return time.UnixMilli(ms).UTC() }

type scanner interface{ Scan(dest ...any) error }

func entryFromRow(row scanner) (HistoryEntry, error) {
	var e HistoryEntry
	var created, duration, transcribe int64
	var app, audio, diagnostics sql.NullString
	var model, replacements, enhance, insert string
	if err := row.Scan(&e.ID, &created, &duration, &app, &e.Transcript, &e.FinalText, &e.StyleID, &model,
		&transcribe, &replacements, &enhance, &insert, &audio, &e.Waveform, &diagnostics); err != nil {
		return e, err
	}
	e.CreatedAt, e.DurationMs, e.Model, e.TranscribeMs = millisToTime(created), uint64(duration), model, uint64(transcribe)
	if app.Valid {
		var a TargetApp
		if err := json.Unmarshal([]byte(app.String), &a); err != nil {
			return e, fmt.Errorf("stored JSON is invalid: %w", err)
		}
		e.App = &a
	}
	if err := json.Unmarshal([]byte(replacements), &e.Replacements); err != nil {
		return e, fmt.Errorf("stored JSON is invalid: %w", err)
	}
	if e.Replacements == nil {
		e.Replacements = []AppliedReplacement{}
	}
	if err := json.Unmarshal([]byte(enhance), &e.Enhance); err != nil {
		return e, fmt.Errorf("stored JSON is invalid: %w", err)
	}
	if err := json.Unmarshal([]byte(insert), &e.Insert); err != nil {
		return e, fmt.Errorf("stored JSON is invalid: %w", err)
	}
	if audio.Valid {
		e.AudioFile = audio.String
	}
	if e.Waveform == nil {
		e.Waveform = []byte{}
	}
	// Diagnostics that do not parse count as none.
	if diagnostics.Valid {
		var d Diagnostics
		if json.Unmarshal([]byte(diagnostics.String), &d) == nil {
			e.Diagnostics = &d
		}
	}
	return e, nil
}

func entryValues(e HistoryEntry) ([]any, error) {
	var app any
	if e.App != nil {
		s, err := compactJSON(e.App)
		if err != nil {
			return nil, err
		}
		app = s
	}
	repl := e.Replacements
	if repl == nil {
		repl = []AppliedReplacement{}
	}
	replJSON, err := compactJSON(repl)
	if err != nil {
		return nil, err
	}
	enh, err := compactJSON(e.Enhance)
	if err != nil {
		return nil, err
	}
	ins, err := compactJSON(e.Insert)
	if err != nil {
		return nil, err
	}
	var audio any
	if e.AudioFile != "" {
		audio = e.AudioFile
	}
	var diag any
	if e.Diagnostics != nil {
		s, err := compactJSON(e.Diagnostics)
		if err != nil {
			return nil, err
		}
		diag = s
	}
	wave := e.Waveform
	if wave == nil {
		wave = []byte{}
	}
	return []any{
		e.CreatedAt.UnixMilli(), int64(e.DurationMs), app, e.Transcript, e.FinalText, e.StyleID, e.Model,
		int64(e.TranscribeMs), replJSON, enh, ins, audio, wave, diag,
	}, nil
}

func marks(n int) string { return strings.TrimSuffix(strings.Repeat("?, ", n), ", ") }

// InsertEntry adds an entry and returns its id.
func (s *Store) InsertEntry(e HistoryEntry) (int64, error) {
	values, err := entryValues(e)
	if err != nil {
		return 0, err
	}
	res, err := s.db.Exec("INSERT INTO history ("+valueColumns+") VALUES ("+marks(len(values))+")", values...)
	if err != nil {
		return 0, fmt.Errorf("database error: %w", err)
	}
	return res.LastInsertId()
}

// UpdateEntry replaces the stored entry with the same id.
func (s *Store) UpdateEntry(e HistoryEntry) error {
	values, err := entryValues(e)
	if err != nil {
		return err
	}
	var assignments []string
	for _, c := range strings.Split(valueColumns, ", ") {
		assignments = append(assignments, c+" = ?")
	}
	res, err := s.db.Exec("UPDATE history SET "+strings.Join(assignments, ", ")+" WHERE id = ?", append(values, e.ID)...)
	if err != nil {
		return fmt.Errorf("database error: %w", err)
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return fmt.Errorf("%w with id %d", ErrNotFound, e.ID)
	}
	return nil
}

// Get returns the entry with the id; ok is false when there is none.
func (s *Store) Get(id int64) (HistoryEntry, bool, error) {
	e, err := entryFromRow(s.db.QueryRow("SELECT "+columns+" FROM history WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return HistoryEntry{}, false, nil
	}
	if err != nil {
		return HistoryEntry{}, false, err
	}
	return e, true, nil
}

// List gives entries, newest first.
func (s *Store) List(q HistoryQuery) ([]HistoryEntry, error) {
	parts, args := q.conditions()
	limit := int64(-1)
	if q.Limit > 0 {
		limit = int64(q.Limit)
	}
	args = append(args, limit, int64(q.Offset))
	rows, err := s.db.Query("SELECT "+columns+" FROM history"+whereClause(parts)+" ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?", args...)
	if err != nil {
		return nil, fmt.Errorf("database error: %w", err)
	}
	defer rows.Close()
	out := []HistoryEntry{}
	for rows.Next() {
		e, err := entryFromRow(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, e)
	}
	return out, rows.Err()
}

// Count counts the entries of a query (limit and offset are ignored).
func (s *Store) Count(q HistoryQuery) (int, error) {
	parts, args := q.conditions()
	var n int
	err := s.db.QueryRow("SELECT COUNT(*) FROM history"+whereClause(parts), args...).Scan(&n)
	return n, err
}

// Delete removes an entry and its audio file. false when there was none.
func (s *Store) Delete(id int64) (bool, error) {
	var audio sql.NullString
	err := s.db.QueryRow("SELECT audio_file FROM history WHERE id = ?", id).Scan(&audio)
	if errors.Is(err, sql.ErrNoRows) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	if _, err := s.db.Exec("DELETE FROM history WHERE id = ?", id); err != nil {
		return false, err
	}
	if audio.Valid {
		s.removeAudioFile(audio.String)
	}
	return true, nil
}

// ClearAll deletes every entry and audio file, then shrinks the database.
func (s *Store) ClearAll() (int, error) {
	res, err := s.db.Exec("DELETE FROM history")
	if err != nil {
		return 0, err
	}
	n, _ := res.RowsAffected()
	s.removeAllAudioFiles()
	if _, err := s.db.Exec("VACUUM"); err != nil {
		return int(n), err
	}
	var a, b, c int
	if err := s.db.QueryRow("PRAGMA wal_checkpoint(TRUNCATE)").Scan(&a, &b, &c); err != nil {
		return int(n), err
	}
	return int(n), nil
}

// ClearAudio deletes every audio file; the entries stay.
func (s *Store) ClearAudio() (int, error) {
	res, err := s.db.Exec("UPDATE history SET audio_file = NULL WHERE audio_file IS NOT NULL")
	if err != nil {
		return 0, err
	}
	n, _ := res.RowsAffected()
	s.removeAllAudioFiles()
	return int(n), nil
}

// StorageUse reports the database and audio sizes.
func (s *Store) StorageUse() (StorageUse, error) {
	var u StorageUse
	err := s.db.QueryRow("SELECT (SELECT COUNT(*) FROM history), page_count * page_size FROM pragma_page_count(), pragma_page_size()").
		Scan(&u.Entries, &u.DatabaseBytes)
	if err != nil {
		return u, err
	}
	u.AudioFiles, u.AudioBytes = s.audioUse()
	return u, nil
}

// StatsEntry is the input of the Home stats.
type StatsEntry struct {
	CreatedAt  time.Time
	DurationMs uint64
	Words      int
}

// StatsSince gives the stats input of the entries since a time, newest first.
func (s *Store) StatsSince(since time.Time) ([]StatsEntry, error) {
	rows, err := s.db.Query("SELECT created_at, duration_ms, final_text FROM history WHERE created_at >= ? ORDER BY created_at DESC, id DESC", since.UnixMilli())
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []StatsEntry
	for rows.Next() {
		var created, duration int64
		var text string
		if err := rows.Scan(&created, &duration, &text); err != nil {
			return nil, err
		}
		out = append(out, StatsEntry{millisToTime(created), uint64(duration), WordCount(text)})
	}
	return out, rows.Err()
}

// AppList gives the apps of the history, the most used first.
func (s *Store) AppList() ([]AppCount, error) {
	rows, err := s.db.Query(`SELECT json_extract(app, '$.bundle_id') AS bundle, json_extract(app, '$.name'), COUNT(*), max(created_at)
             FROM history WHERE app IS NOT NULL GROUP BY bundle
             ORDER BY COUNT(*) DESC, bundle`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []AppCount
	for rows.Next() {
		var a AppCount
		var latest int64
		if err := rows.Scan(&a.BundleID, &a.Name, &a.Count, &latest); err != nil {
			return nil, err
		}
		out = append(out, a)
	}
	return out, rows.Err()
}
