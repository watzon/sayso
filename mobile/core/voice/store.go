package voice

import (
	"errors"
	"slices"
	"strconv"
	"sync"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/journal"
	"github.com/watzon/sayso/mobile/core/styles"
)

// The voice store keeps custom styles and voice history in a journal (see
// package journal) of these records:
//
//	S <id> <name> <prompt>                         put a custom style
//	SD <id>                                        remove a custom style
//	H <id> <ms> <duration ms> <engine> <style> <raw> <final>   a history entry
//	N <next id>                                    the next entry id (after a rewrite)
//
// Deleting or clearing history rewrites the file, so deleted text leaves it
// at once. Entries dropped for MaxEntries leave it at the next compaction.
// Unknown records are skipped, so newer journals can be read.

// Bounds that keep the store, and so its file, small.
const (
	MaxEntries      = 500     // the oldest history entries go first
	MaxStyles       = 100     // custom styles
	MaxEntryText    = 8 << 10 // bytes of raw or final text kept per entry
	MaxIDLen        = 64      // style and engine ids
	MaxStyleName    = 128
	MaxStylePrompt  = 4 << 10
	storeCompactMin = 256 << 10 // never compact a file smaller than this
)

// Errors of PutStyle and RemoveStyle.
var (
	ErrBuiltinStyle = errors.New("built-in styles cannot be changed")
	ErrBadStyle     = errors.New("a style needs an id of a-z, 0-9 and - (at most 64), a name of at most 128 bytes and a prompt of at most 4 KB")
	ErrTooManyStyle = errors.New("too many custom styles")
)

// Style is a built-in or custom style.
type Style struct {
	ID, Name string
	// Prompt is the instruction sent to the cloud model; empty: text rules
	// only, no cloud call.
	Prompt  string
	BuiltIn bool
}

// Entry is one dictation in the history.
type Entry struct {
	ID                      int64 // assigned by the store
	TimestampMs, DurationMs int64
	EngineID, StyleID       string
	Raw, Final              string
}

// Store holds custom styles and voice history. It is safe for concurrent
// use.
type Store struct {
	mu      sync.Mutex
	j       *journal.File // nil: kept in memory only
	err     error         // the first write error
	custom  []Style       // in the order they were first put
	entries []Entry       // oldest first
	nextID  int64
	// compactAt is the file size that triggers a rewrite: twice the size
	// after the last one, and at least storeCompactMin.
	compactAt int64
}

// OpenStore replays the journal at path and appends to it from then on.
// An empty path keeps everything in memory.
func OpenStore(path string) (*Store, error) {
	s := &Store{nextID: 1}
	if path == "" {
		return s, nil
	}
	if err := journal.Read(path, s.apply); err != nil {
		return nil, err
	}
	j, err := journal.Open(path)
	if err != nil {
		return nil, err
	}
	s.j = j
	n := int64(0)
	for _, l := range s.stateLines() {
		n += int64(len(l))
	}
	s.compactAt = max(storeCompactMin, 2*n)
	s.maybeCompact()
	return s, nil
}

// Close closes the journal.
func (s *Store) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.j == nil {
		return nil
	}
	err := s.j.Close()
	s.j = nil
	return err
}

// Err is the first journal write error, if any.
func (s *Store) Err() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.err
}

func (s *Store) apply(f []string) {
	switch {
	case f[0] == "S" && len(f) >= 4:
		s.putStyle(Style{ID: f[1], Name: f[2], Prompt: f[3]})
	case f[0] == "SD" && len(f) >= 2:
		s.removeStyle(f[1])
	case f[0] == "H" && len(f) >= 8:
		id, err1 := strconv.ParseInt(f[1], 10, 64)
		ms, err2 := strconv.ParseInt(f[2], 10, 64)
		dur, err3 := strconv.ParseInt(f[3], 10, 64)
		if err1 == nil && err2 == nil && err3 == nil && id > 0 {
			s.addEntry(Entry{ID: id, TimestampMs: ms, DurationMs: dur, EngineID: f[4], StyleID: f[5], Raw: f[6], Final: f[7]})
		}
	case f[0] == "N" && len(f) >= 2:
		if n, err := strconv.ParseInt(f[1], 10, 64); err == nil && n > s.nextID {
			s.nextID = n
		}
	}
}

// record applies a record and appends it to the journal.
func (s *Store) record(fields ...string) {
	s.apply(fields)
	if s.j == nil {
		return
	}
	if err := s.j.Append(journal.Record(fields...)); err != nil && s.err == nil {
		s.err = err
	}
	s.maybeCompact()
}

// maybeCompact rewrites the journal as the current state once it has
// doubled since the last rewrite, which keeps it within about twice the
// state's size.
func (s *Store) maybeCompact() {
	if s.j != nil && s.j.Size() > s.compactAt {
		s.rewrite()
	}
}

func (s *Store) stateLines() []string {
	lines := []string{journal.Record("N", strconv.FormatInt(s.nextID, 10))}
	for _, st := range s.custom {
		lines = append(lines, journal.Record("S", st.ID, st.Name, st.Prompt))
	}
	for _, e := range s.entries {
		lines = append(lines, journal.Record(entryFields(e)...))
	}
	return lines
}

func entryFields(e Entry) []string {
	return []string{"H", strconv.FormatInt(e.ID, 10), strconv.FormatInt(e.TimestampMs, 10), strconv.FormatInt(e.DurationMs, 10),
		e.EngineID, e.StyleID, e.Raw, e.Final}
}

// ---- Styles ----

// Styles are the built-in styles, then the custom ones in the order they
// were added.
func (s *Store) Styles() []Style {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]Style, 0, len(styles.Builtins)+len(s.custom))
	for _, b := range styles.Builtins {
		out = append(out, Style{ID: b.ID, Name: b.Name, Prompt: b.Prompt, BuiltIn: true})
	}
	return append(out, s.custom...)
}

// Style is the built-in or custom style with id.
func (s *Store) Style(id string) (Style, bool) {
	if b, ok := styles.Find(id); ok {
		return Style{ID: b.ID, Name: b.Name, Prompt: b.Prompt, BuiltIn: true}, true
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if i := s.styleIndex(id); i >= 0 {
		return s.custom[i], true
	}
	return Style{}, false
}

// PutStyle adds a custom style or replaces the one with its id.
func (s *Store) PutStyle(st Style) error {
	if _, ok := styles.Find(st.ID); ok {
		return ErrBuiltinStyle
	}
	if !styles.ValidID(st.ID) || len(st.ID) > MaxIDLen || st.Name == "" || len(st.Name) > MaxStyleName || len(st.Prompt) > MaxStylePrompt ||
		!utf8.ValidString(st.Name) || !utf8.ValidString(st.Prompt) {
		return ErrBadStyle
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.styleIndex(st.ID) < 0 && len(s.custom) >= MaxStyles {
		return ErrTooManyStyle
	}
	s.record("S", st.ID, st.Name, st.Prompt)
	return nil
}

// RemoveStyle removes a custom style; an unknown id is not an error.
func (s *Store) RemoveStyle(id string) error {
	if _, ok := styles.Find(id); ok {
		return ErrBuiltinStyle
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.styleIndex(id) >= 0 {
		s.record("SD", id)
	}
	return nil
}

func (s *Store) styleIndex(id string) int {
	return slices.IndexFunc(s.custom, func(st Style) bool { return st.ID == id })
}

func (s *Store) putStyle(st Style) {
	st.BuiltIn = false
	if i := s.styleIndex(st.ID); i >= 0 {
		s.custom[i] = st
		return
	}
	s.custom = append(s.custom, st)
}

func (s *Store) removeStyle(id string) {
	if i := s.styleIndex(id); i >= 0 {
		s.custom = slices.Delete(s.custom, i, i+1)
	}
}

// ---- History ----

// AddEntry appends e to the history with a new id and returns it. Ids and
// texts are cut to MaxIDLen and MaxEntryText bytes. Past MaxEntries the
// oldest entries go.
func (s *Store) AddEntry(e Entry) int64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	e.ID = s.nextID
	e.EngineID, e.StyleID = cut(e.EngineID, MaxIDLen), cut(e.StyleID, MaxIDLen)
	e.Raw, e.Final = cut(e.Raw, MaxEntryText), cut(e.Final, MaxEntryText)
	s.record(entryFields(e)...)
	return e.ID
}

// Entries are up to limit history entries, newest first (limit <= 0: all).
func (s *Store) Entries(limit int) []Entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	n := len(s.entries)
	if limit > 0 && limit < n {
		n = limit
	}
	out := make([]Entry, n)
	for i := range out {
		out[i] = s.entries[len(s.entries)-1-i]
	}
	return out
}

// DeleteEntry deletes the entry with id; an unknown id is not an error.
func (s *Store) DeleteEntry(id int64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	n := len(s.entries)
	s.entries = slices.DeleteFunc(s.entries, func(e Entry) bool { return e.ID == id })
	if len(s.entries) != n {
		s.rewrite()
	}
}

// ClearEntries deletes the whole history.
func (s *Store) ClearEntries() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.entries = nil
	s.rewrite()
}

// rewrite replaces the journal with the current state.
func (s *Store) rewrite() {
	if s.j == nil {
		return
	}
	if err := s.j.Rewrite(s.stateLines()); err != nil && s.err == nil {
		s.err = err
	}
	s.compactAt = max(storeCompactMin, 2*s.j.Size())
}

func (s *Store) addEntry(e Entry) {
	s.entries = append(s.entries, e)
	if len(s.entries) > MaxEntries {
		s.entries = slices.Delete(s.entries, 0, len(s.entries)-MaxEntries)
	}
	if e.ID >= s.nextID {
		s.nextID = e.ID + 1
	}
}

// cut shortens s to at most n bytes, on a rune boundary.
func cut(s string, n int) string {
	if len(s) <= n {
		return s
	}
	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}
	return s[:n]
}
