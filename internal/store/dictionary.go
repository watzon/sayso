package store

import (
	"database/sql"
	"errors"
	"fmt"
	"strings"
	"time"
)

// ListWords gives the words, sorted without letter case.
func (s *Store) ListWords() ([]Word, error) {
	rows, err := s.db.Query("SELECT id, text FROM words ORDER BY text COLLATE NOCASE")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Word{}
	for rows.Next() {
		var w Word
		if err := rows.Scan(&w.ID, &w.Text); err != nil {
			return nil, err
		}
		out = append(out, w)
	}
	return out, rows.Err()
}

// AddWord adds a word, or gives the word that is there (letter case ignored).
func (s *Store) AddWord(text string) (Word, error) {
	text = strings.TrimSpace(text)
	if text == "" {
		return Word{}, &InvalidInputError{"a word cannot be empty"}
	}
	if _, err := s.db.Exec("INSERT OR IGNORE INTO words (text) VALUES (?)", text); err != nil {
		return Word{}, err
	}
	var w Word
	err := s.db.QueryRow("SELECT id, text FROM words WHERE text = ?", text).Scan(&w.ID, &w.Text)
	return w, err
}

// RemoveWord deletes a word.
func (s *Store) RemoveWord(id int64) (bool, error) {
	res, err := s.db.Exec("DELETE FROM words WHERE id = ?", id)
	if err != nil {
		return false, err
	}
	n, _ := res.RowsAffected()
	return n > 0, nil
}

func scanReplacement(row scanner) (Replacement, error) {
	var r Replacement
	var uses int64
	err := row.Scan(&r.ID, &r.From, &r.To, &r.CaseSensitive, &uses)
	r.Uses = uint64(uses)
	return r, err
}

func requireFrom(from string) error {
	if strings.TrimSpace(from) == "" {
		return &InvalidInputError{"a replacement needs text to find"}
	}
	return nil
}

// ListReplacements gives the rules in id order.
func (s *Store) ListReplacements() ([]Replacement, error) {
	rows, err := s.db.Query("SELECT id, from_text, to_text, case_sensitive, uses FROM replacements ORDER BY id")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []Replacement{}
	for rows.Next() {
		r, err := scanReplacement(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}

// AddReplacement adds a rule; its id and from (trimmed) come back.
func (s *Store) AddReplacement(r Replacement) (Replacement, error) {
	if err := requireFrom(r.From); err != nil {
		return Replacement{}, err
	}
	from := strings.TrimSpace(r.From)
	res, err := s.db.Exec("INSERT INTO replacements (from_text, to_text, case_sensitive) VALUES (?, ?, ?)", from, r.To, r.CaseSensitive)
	if err != nil {
		return Replacement{}, err
	}
	id, _ := res.LastInsertId()
	r.ID, r.From, r.Uses = id, from, 0
	return r, nil
}

// UpdateReplacement changes a rule's text and case setting.
func (s *Store) UpdateReplacement(r Replacement) error {
	if err := requireFrom(r.From); err != nil {
		return err
	}
	res, err := s.db.Exec("UPDATE replacements SET from_text = ?, to_text = ?, case_sensitive = ? WHERE id = ?",
		strings.TrimSpace(r.From), r.To, r.CaseSensitive, r.ID)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return fmt.Errorf("%w with id %d", ErrNotFound, r.ID)
	}
	return nil
}

// RemoveReplacement deletes a rule.
func (s *Store) RemoveReplacement(id int64) (bool, error) {
	res, err := s.db.Exec("DELETE FROM replacements WHERE id = ?", id)
	if err != nil {
		return false, err
	}
	n, _ := res.RowsAffected()
	return n > 0, nil
}

// GetReplacement gives one rule.
func (s *Store) GetReplacement(id int64) (Replacement, bool, error) {
	r, err := scanReplacement(s.db.QueryRow("SELECT id, from_text, to_text, case_sensitive, uses FROM replacements WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return Replacement{}, false, nil
	}
	return r, err == nil, err
}

// RecordReplacementUses adds the counts of applied rules to their uses.
func (s *Store) RecordReplacementUses(applied []AppliedReplacement) error {
	tx, err := s.db.Begin()
	if err != nil {
		return err
	}
	for _, a := range applied {
		if _, err := tx.Exec("UPDATE replacements SET uses = uses + ? WHERE id = ?", int64(a.Count), a.ReplacementID); err != nil {
			tx.Rollback()
			return err
		}
	}
	return tx.Commit()
}

// RetentionReport is what ApplyRetention removed.
type RetentionReport struct {
	EntriesDeleted int
	AudioDeleted   int
}

func cutoffMillis(now time.Time, days uint32) int64 {
	return now.Add(-time.Duration(days) * 24 * time.Hour).UnixMilli()
}

// ApplyRetention deletes entries older than keepTextDays and audio older than
// keepAudioDays (nil keeps forever). When audio expires only the file goes.
func (s *Store) ApplyRetention(keepTextDays, keepAudioDays *uint32, now time.Time) (RetentionReport, error) {
	var report RetentionReport
	if keepTextDays != nil {
		cutoff := cutoffMillis(now, *keepTextDays)
		files, n, err := s.expire(cutoff, "DELETE FROM history WHERE created_at < ?")
		if err != nil {
			return report, err
		}
		report.EntriesDeleted = n
		for _, f := range files {
			if s.removeAudioFile(f) {
				report.AudioDeleted++
			}
		}
	}
	if keepAudioDays != nil {
		cutoff := cutoffMillis(now, *keepAudioDays)
		files, _, err := s.expire(cutoff, "UPDATE history SET audio_file = NULL WHERE audio_file IS NOT NULL AND created_at < ?")
		if err != nil {
			return report, err
		}
		for _, f := range files {
			if s.removeAudioFile(f) {
				report.AudioDeleted++
			}
		}
	}
	return report, nil
}

func (s *Store) expire(cutoff int64, change string) ([]string, int, error) {
	tx, err := s.db.Begin()
	if err != nil {
		return nil, 0, err
	}
	rows, err := tx.Query("SELECT audio_file FROM history WHERE audio_file IS NOT NULL AND created_at < ?", cutoff)
	if err != nil {
		tx.Rollback()
		return nil, 0, err
	}
	var files []string
	for rows.Next() {
		var f string
		if err := rows.Scan(&f); err != nil {
			rows.Close()
			tx.Rollback()
			return nil, 0, err
		}
		files = append(files, f)
	}
	rows.Close()
	res, err := tx.Exec(change, cutoff)
	if err != nil {
		tx.Rollback()
		return nil, 0, err
	}
	n, _ := res.RowsAffected()
	return files, int(n), tx.Commit()
}
