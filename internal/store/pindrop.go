package store

import (
	"database/sql"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"strings"
	"time"

	"howett.net/plist"
)

// The import from Pindrop, the app Sayso replaced.
// Pindrop keeps a Core Data SQLite
// store; the import reads a copy of it, so Pindrop can keep running.

const (
	pindropStoreFile = "default.store"
	coreDataEpoch    = 978_307_200.0
	// PindropID is the provider id, the style id, and the import id prefix of imported entries.
	PindropID = "pindrop"
	// RawStyleID is the style of an entry without enhancement.
	RawStyleID = "raw"
)

var pindropSideFiles = []string{"default.store-wal", "default.store-shm"}

// PindropReplacement is a replacement rule of Pindrop.
type PindropReplacement struct {
	From, To      string
	CaseSensitive bool
}

// PindropPreset is a prompt the user made in Pindrop.
type PindropPreset struct{ Name, Prompt string }

// PindropDictation is one microphone dictation of Pindrop.
type PindropDictation struct {
	ID           string
	CreatedAt    time.Time
	DurationMs   uint64
	Transcript   string
	FinalText    string
	EnhancedWith string
	Model        string
	App          *TargetApp
}

// PindropData is what an import reads.
type PindropData struct {
	Words        []string
	Replacements []PindropReplacement
	Presets      []PindropPreset
	Dictations   []PindropDictation
}

// ImportReport counts what an import added.
type ImportReport struct{ Words, Replacements, Dictations int }

// PindropFound is true when dir holds a Pindrop store.
func PindropFound(dir string) bool {
	fi, err := os.Stat(filepath.Join(dir, pindropStoreFile))
	return err == nil && fi.Mode().IsRegular()
}

func copyFile(from, to string) error {
	src, err := os.Open(from)
	if err != nil {
		return err
	}
	defer src.Close()
	dst, err := os.Create(to)
	if err != nil {
		return err
	}
	if _, err := io.Copy(dst, src); err != nil {
		dst.Close()
		return err
	}
	return dst.Close()
}

// ReadPindrop reads a copy of the Pindrop store in dir.
func ReadPindrop(dir string) (PindropData, error) {
	if !PindropFound(dir) {
		return PindropData{}, &InvalidInputError{fmt.Sprintf("no Pindrop data in %s", dir)}
	}
	tmp, err := os.MkdirTemp("", "sayso-pindrop-")
	if err != nil {
		return PindropData{}, err
	}
	defer os.RemoveAll(tmp)
	if err := copyFile(filepath.Join(dir, pindropStoreFile), filepath.Join(tmp, pindropStoreFile)); err != nil {
		return PindropData{}, err
	}
	for _, name := range pindropSideFiles {
		if fi, err := os.Stat(filepath.Join(dir, name)); err == nil && fi.Mode().IsRegular() {
			if err := copyFile(filepath.Join(dir, name), filepath.Join(tmp, name)); err != nil {
				return PindropData{}, err
			}
		}
	}
	db, err := sql.Open("sqlite", "file:"+filepath.ToSlash(filepath.Join(tmp, pindropStoreFile)))
	if err != nil {
		return PindropData{}, err
	}
	defer db.Close()
	db.SetMaxOpenConns(1)
	var data PindropData
	if data.Words, err = readPindropWords(db); err != nil {
		return data, err
	}
	if data.Replacements, err = readPindropReplacements(db); err != nil {
		return data, err
	}
	if data.Presets, err = readPindropPresets(db); err != nil {
		return data, err
	}
	data.Dictations, err = readPindropDictations(db)
	return data, err
}

func tableColumns(db *sql.DB, table string) (map[string]bool, error) {
	rows, err := db.Query("SELECT name FROM pragma_table_info(?)", table)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	cols := map[string]bool{}
	for rows.Next() {
		var n string
		if err := rows.Scan(&n); err != nil {
			return nil, err
		}
		cols[n] = true
	}
	return cols, rows.Err()
}

func optionalColumn(cols map[string]bool, col string) string {
	if cols[col] {
		return col
	}
	return "NULL AS " + col
}

func readPindropWords(db *sql.DB) ([]string, error) {
	cols, err := tableColumns(db, "ZVOCABULARYWORD")
	if err != nil || !cols["ZWORD"] {
		return nil, err
	}
	rows, err := db.Query("SELECT ZWORD FROM ZVOCABULARYWORD WHERE length(trim(ZWORD)) > 0 ORDER BY Z_PK")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []string
	for rows.Next() {
		var w string
		if err := rows.Scan(&w); err != nil {
			return nil, err
		}
		out = append(out, w)
	}
	return out, rows.Err()
}

func readPindropReplacements(db *sql.DB) ([]PindropReplacement, error) {
	cols, err := tableColumns(db, "ZWORDREPLACEMENT")
	if err != nil || !cols["ZORIGINALS"] || !cols["ZREPLACEMENT"] {
		return nil, err
	}
	rows, err := db.Query("SELECT ZORIGINALS, ZREPLACEMENT, " + optionalColumn(cols, "ZMATCHMODERAWVALUE") + " FROM ZWORDREPLACEMENT ORDER BY Z_PK")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []PindropReplacement
	for rows.Next() {
		var originals []byte
		var to, mode sql.NullString
		if err := rows.Scan(&originals, &to, &mode); err != nil {
			return nil, err
		}
		if originals == nil || !to.Valid || (mode.Valid && mode.String == "command") {
			continue
		}
		cs := mode.Valid && mode.String == "exact"
		for _, from := range archivedStrings(originals) {
			if strings.TrimSpace(from) != "" {
				out = append(out, PindropReplacement{from, to.String, cs})
			}
		}
	}
	return out, rows.Err()
}

// archivedStrings reads the strings of an NSKeyedArchiver array.
func archivedStrings(blob []byte) []string {
	var archive map[string]any
	if _, err := plist.Unmarshal(blob, &archive); err != nil {
		return nil
	}
	objects, ok := archive["$objects"].([]any)
	if !ok {
		return nil
	}
	object := func(v any) any {
		uid, ok := v.(plist.UID)
		if !ok || int(uid) >= len(objects) {
			return nil
		}
		return objects[uid]
	}
	top, ok := archive["$top"].(map[string]any)
	if !ok {
		return nil
	}
	root, ok := object(top["root"]).(map[string]any)
	if !ok {
		return nil
	}
	items, ok := root["NS.objects"].([]any)
	if !ok {
		return nil
	}
	var out []string
	for _, it := range items {
		if s, ok := object(it).(string); ok {
			out = append(out, s)
		}
	}
	return out
}

func readPindropPresets(db *sql.DB) ([]PindropPreset, error) {
	cols, err := tableColumns(db, "ZPROMPTPRESET")
	if err != nil || !cols["ZNAME"] || !cols["ZPROMPT"] {
		return nil, err
	}
	userMade := "1"
	if cols["ZISBUILTIN"] {
		userMade = "COALESCE(ZISBUILTIN, 0) = 0"
	}
	rows, err := db.Query("SELECT ZNAME, ZPROMPT FROM ZPROMPTPRESET WHERE " + userMade +
		" AND length(trim(ZNAME)) > 0 AND length(trim(ZPROMPT)) > 0 ORDER BY Z_PK")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []PindropPreset
	for rows.Next() {
		var p PindropPreset
		if err := rows.Scan(&p.Name, &p.Prompt); err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func readPindropDictations(db *sql.DB) ([]PindropDictation, error) {
	cols, err := tableColumns(db, "ZTRANSCRIPTIONRECORD")
	if err != nil {
		return nil, err
	}
	for _, c := range []string{"ZID", "ZTEXT", "ZTIMESTAMP"} {
		if !cols[c] {
			return nil, nil
		}
	}
	fromMic := "1"
	if cols["ZSOURCEKINDRAWVALUE"] {
		fromMic = "(r.ZSOURCEKINDRAWVALUE IS NULL OR r.ZSOURCEKINDRAWVALUE = 'voiceRecording')"
	}
	noSession := "1"
	sessionCols, err := tableColumns(db, "ZCAPTURESESSIONMODEL")
	if err != nil {
		return nil, err
	}
	if sessionCols["ZTRANSCRIPTIONRECORDID"] {
		noSession = "NOT EXISTS (SELECT 1 FROM ZCAPTURESESSIONMODEL s WHERE s.ZTRANSCRIPTIONRECORDID = r.ZID)"
	}
	q := fmt.Sprintf(`SELECT hex(r.ZID), r.ZTIMESTAMP, %s, %s, r.ZTEXT, %s, %s, %s, %s
         FROM ZTRANSCRIPTIONRECORD r
         WHERE %s AND %s
           AND r.ZID IS NOT NULL AND r.ZTIMESTAMP IS NOT NULL AND length(trim(r.ZTEXT)) > 0
         ORDER BY r.ZTIMESTAMP`,
		optionalColumn(cols, "ZDURATION"), optionalColumn(cols, "ZORIGINALTEXT"), optionalColumn(cols, "ZENHANCEDWITH"),
		optionalColumn(cols, "ZMODELUSED"), optionalColumn(cols, "ZDESTINATIONAPPNAME"), optionalColumn(cols, "ZDESTINATIONAPPBUNDLEID"),
		fromMic, noSession)
	rows, err := db.Query(q)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []PindropDictation
	for rows.Next() {
		var id, finalText string
		var savedAt float64
		var duration sql.NullFloat64
		var original, enhanced, model, appName, bundle sql.NullString
		if err := rows.Scan(&id, &savedAt, &duration, &original, &finalText, &enhanced, &model, &appName, &bundle); err != nil {
			return nil, err
		}
		durS := math.Max(duration.Float64, 0)
		d := PindropDictation{
			ID:         strings.ToLower(id),
			CreatedAt:  time.UnixMilli(int64((savedAt + coreDataEpoch - durS) * 1000)).UTC(),
			DurationMs: uint64(durS * 1000),
			Transcript: finalText,
			FinalText:  finalText,
			Model:      PindropID,
		}
		if original.Valid {
			d.Transcript = original.String
		}
		if enhanced.Valid && enhanced.String != "" {
			d.EnhancedWith = enhanced.String
		}
		if model.Valid && model.String != "" {
			d.Model = model.String
		}
		if appName.Valid && bundle.Valid && bundle.String != "" {
			d.App = &TargetApp{BundleID: bundle.String, Name: appName.String}
		}
		out = append(out, d)
	}
	return out, rows.Err()
}

func pindropHistoryEntry(d PindropDictation) HistoryEntry {
	style, enhance := RawStyleID, EnhanceOutcome{Result: EnhanceNotUsed}
	if d.EnhancedWith != "" {
		style = PindropID
		enhance = EnhanceOutcome{Result: EnhanceApplied, ProviderID: PindropID, Model: d.EnhancedWith}
	}
	return HistoryEntry{
		CreatedAt: d.CreatedAt, DurationMs: d.DurationMs, App: d.App, Transcript: d.Transcript, FinalText: d.FinalText,
		StyleID: style, Model: d.Model, Replacements: []AppliedReplacement{}, Enhance: enhance,
		Insert: InsertOutcome{Result: InsertPasted, ClipboardRestored: true}, Waveform: []byte{},
	}
}

// ImportPindrop adds what is new: a second import skips what it finds.
func (s *Store) ImportPindrop(data PindropData) (ImportReport, error) {
	var report ImportReport
	tx, err := s.db.Begin()
	if err != nil {
		return report, err
	}
	fail := func(err error) (ImportReport, error) { tx.Rollback(); return ImportReport{}, err }
	for _, w := range data.Words {
		res, err := tx.Exec("INSERT OR IGNORE INTO words (text) VALUES (?)", strings.TrimSpace(w))
		if err != nil {
			return fail(err)
		}
		n, _ := res.RowsAffected()
		report.Words += int(n)
	}
	for _, r := range data.Replacements {
		res, err := tx.Exec(`INSERT INTO replacements (from_text, to_text, case_sensitive)
                 SELECT ?1, ?2, ?3 WHERE NOT EXISTS
                   (SELECT 1 FROM replacements WHERE from_text = ?1 COLLATE NOCASE AND to_text = ?2)`,
			strings.TrimSpace(r.From), r.To, r.CaseSensitive)
		if err != nil {
			return fail(err)
		}
		n, _ := res.RowsAffected()
		report.Replacements += int(n)
	}
	q := "INSERT OR IGNORE INTO history (" + valueColumns + ", import_id) VALUES (" + marks(len(strings.Split(valueColumns, ", "))+1) + ")"
	for _, d := range data.Dictations {
		values, err := entryValues(pindropHistoryEntry(d))
		if err != nil {
			return fail(err)
		}
		res, err := tx.Exec(q, append(values, PindropID+":"+d.ID)...)
		if err != nil {
			return fail(err)
		}
		n, _ := res.RowsAffected()
		report.Dictations += int(n)
	}
	return report, tx.Commit()
}
