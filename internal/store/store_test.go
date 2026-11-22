package store

import (
	"database/sql"
	"encoding/json"
	"errors"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"howett.net/plist"
)

func tempStore(t *testing.T) (*Store, string) {
	t.Helper()
	dir := t.TempDir()
	s, err := Open(filepath.Join(dir, "sayso.db"), filepath.Join(dir, "audio"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s, dir
}

var baseTime = time.Date(2026, 10, 1, 12, 0, 0, 0, time.UTC)

func entry(text string, age time.Duration) HistoryEntry {
	return HistoryEntry{
		CreatedAt: baseTime.Add(-age), DurationMs: 2000, Transcript: text, FinalText: text, StyleID: "clean",
		Model: "parakeet", TranscribeMs: 80, Enhance: EnhanceOutcome{Result: EnhanceNotUsed},
		Insert: InsertOutcome{Result: InsertTyped}, Waveform: []byte{1, 2, 3},
	}
}

func TestCreatesFromScratchReopensAndUsesWAL(t *testing.T) {
	dir := t.TempDir()
	db := filepath.Join(dir, "nested", "dir", "sayso.db")
	audio := filepath.Join(dir, "nested", "audio")
	s, err := Open(db, audio)
	if err != nil {
		t.Fatal(err)
	}
	if v, _ := s.SchemaVersion(); v != 3 {
		t.Fatal("version", v)
	}
	s.AddWord("Sayso")
	id, err := s.InsertEntry(entry("kept across restarts", 0))
	if err != nil {
		t.Fatal(err)
	}
	s.Close()
	s, err = Open(db, audio)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	e, ok, _ := s.Get(id)
	if !ok || e.FinalText != "kept across restarts" {
		t.Fatal(e)
	}
	if n, _ := s.Count(HistoryQuery{Text: "restarts"}); n != 1 {
		t.Fatal("search index after reopen", n)
	}
	var mode string
	s.db.QueryRow("PRAGMA journal_mode").Scan(&mode)
	var fk, busy int
	s.db.QueryRow("PRAGMA foreign_keys").Scan(&fk)
	s.db.QueryRow("PRAGMA busy_timeout").Scan(&busy)
	if mode != "wal" || fk != 1 || busy != 5000 {
		t.Fatal(mode, fk, busy)
	}
}

func TestRefusesANewerDatabase(t *testing.T) {
	dir := t.TempDir()
	db := filepath.Join(dir, "sayso.db")
	raw, _ := sql.Open("sqlite", "file:"+filepath.ToSlash(db))
	raw.Exec("PRAGMA user_version = 99")
	raw.Close()
	_, err := Open(db, filepath.Join(dir, "audio"))
	var tooNew *SchemaTooNewError
	if !errors.As(err, &tooNew) || tooNew.Found != 99 {
		t.Fatal(err)
	}
}

func TestVersion1DatabaseKeepsItsHistory(t *testing.T) {
	dir := t.TempDir()
	db := filepath.Join(dir, "sayso.db")
	raw, _ := sql.Open("sqlite", "file:"+filepath.ToSlash(db))
	if _, err := raw.Exec(migrations[0]); err != nil {
		t.Fatal(err)
	}
	raw.Exec("PRAGMA user_version = 1")
	raw.Exec(`INSERT INTO history (created_at, duration_ms, transcript, final_text, style_id, model, transcribe_ms, replacements, enhance, insert_result, waveform)
             VALUES (1, 2, 'raw', 'final', 'clean', 'parakeet', 3, '[]', '{"result":"not_used"}', '{"result":"typed"}', x'')`)
	raw.Close()
	s, err := Open(db, filepath.Join(dir, "audio"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	list, err := s.List(HistoryQuery{})
	if err != nil || len(list) != 1 || list[0].FinalText != "final" || list[0].Diagnostics != nil {
		t.Fatal(list, err)
	}
}

func TestJSONColumnsMatchSerde(t *testing.T) {
	s, _ := tempStore(t)
	e := entry("zq <you> & me", 0)
	e.App = &TargetApp{BundleID: "com.apple.mail", Name: "Mail"}
	e.Enhance = EnhanceOutcome{Result: EnhanceApplied, ProviderID: "p", Model: "m", ElapsedMs: 5}
	e.Insert = InsertOutcome{Result: InsertPasted, ClipboardRestored: true}
	e.Replacements = []AppliedReplacement{{ReplacementID: 1, From: "a", To: "b", Count: 2}}
	e.Diagnostics = &Diagnostics{AppVersion: "1.0", Enhance: &EnhanceTimings{ProviderKind: "chat_gpt"}}
	id, err := s.InsertEntry(e)
	if err != nil {
		t.Fatal(err)
	}
	var app, enh, ins, repl, diag string
	s.db.QueryRow("SELECT app, enhance, insert_result, replacements, diagnostics FROM history WHERE id = ?", id).Scan(&app, &enh, &ins, &repl, &diag)
	want := map[string]string{
		app:  `{"bundle_id":"com.apple.mail","name":"Mail"}`,
		enh:  `{"result":"applied","provider_id":"p","model":"m","elapsed_ms":5}`,
		ins:  `{"result":"pasted","clipboard_restored":true}`,
		repl: `[{"replacement_id":1,"from":"a","to":"b","count":2}]`,
	}
	for got, w := range want {
		if got != w {
			t.Errorf("got %s want %s", got, w)
		}
	}
	if !strings.Contains(diag, `"attempts":[]`) || !strings.Contains(diag, `"start_to_capture_ms":null`) || strings.Contains(diag, `"cleanup"`) {
		t.Fatal(diag)
	}
	got, _, _ := s.Get(id)
	if got.Enhance != e.Enhance || got.Insert != e.Insert || got.App.Name != "Mail" || got.Diagnostics.AppVersion != "1.0" {
		t.Fatalf("%+v", got)
	}
	meta := got.MetadataJSON()
	if strings.Contains(meta, "zq") || !strings.Contains(meta, `"transcript_chars": 13`) || !strings.Contains(meta, `"created_at": "2026-10-01T12:00:00Z"`) || !strings.Contains(meta, `"replacements": 2`) {
		t.Fatal(meta)
	}
	var m map[string]any
	if json.Unmarshal([]byte(meta), &m) != nil || m["app_id"] != "com.apple.mail" {
		t.Fatal(meta)
	}
}

func TestSearchFiltersAndOrder(t *testing.T) {
	s, _ := tempStore(t)
	a := entry("the quick brown fox", 3*time.Hour)
	b := entry("café au lait", 2*time.Hour)
	b.Enhance = EnhanceOutcome{Result: EnhanceApplied, ProviderID: "p", Model: "m"}
	b.App = &TargetApp{BundleID: "com.x", Name: "X"}
	c := entry("quiet evening", time.Hour)
	for _, e := range []HistoryEntry{a, b, c} {
		if _, err := s.InsertEntry(e); err != nil {
			t.Fatal(err)
		}
	}
	list, _ := s.List(HistoryQuery{})
	if list[0].FinalText != "quiet evening" {
		t.Fatal("order")
	}
	if n, _ := s.Count(HistoryQuery{Text: "qui"}); n != 2 {
		t.Fatal("prefix search", n)
	}
	if n, _ := s.Count(HistoryQuery{Text: "cafe"}); n != 1 {
		t.Fatal("diacritics", n)
	}
	if n, err := s.Count(HistoryQuery{Text: `"fox`}); err != nil || n != 1 {
		t.Fatal("quote escaping", n)
	}
	if n, _ := s.Count(HistoryQuery{EnhancedOnly: true}); n != 1 {
		t.Fatal("enhanced", n)
	}
	if apps, _ := s.AppList(); len(apps) != 1 || apps[0].BundleID != "com.x" {
		t.Fatal(apps)
	}
	page, _ := s.List(HistoryQuery{Limit: 1, Offset: 1})
	if len(page) != 1 || page[0].FinalText != "café au lait" {
		t.Fatal(page)
	}
	// An update re-indexes the text.
	list[0].FinalText = "updated words"
	list[0].Transcript = "updated words"
	if err := s.UpdateEntry(list[0]); err != nil {
		t.Fatal(err)
	}
	if n, _ := s.Count(HistoryQuery{Text: "evening"}); n != 0 {
		t.Fatal("stale index")
	}
	if err := s.UpdateEntry(HistoryEntry{ID: 999, Enhance: EnhanceOutcome{Result: EnhanceNotUsed}, Insert: InsertOutcome{Result: InsertTyped}}); !errors.Is(err, ErrNotFound) {
		t.Fatal(err)
	}
	stats, _ := s.StatsSince(baseTime.Add(-150 * time.Minute))
	if len(stats) != 2 || stats[0].Words != 2 {
		t.Fatal(stats)
	}
}

func TestFLACRoundTrip(t *testing.T) {
	samples := make([]float32, 16000+123)
	for i := range samples {
		samples[i] = float32(0.5 * math.Sin(float64(i)*2*math.Pi*440/16000))
	}
	b, err := EncodeFLAC(samples)
	if err != nil {
		t.Fatal(err)
	}
	if string(b[:4]) != "fLaC" || len(b) >= len(samples)*2 {
		t.Fatal("not compressed FLAC", len(b))
	}
	back, err := DecodeFLAC(b)
	if err != nil || len(back) != len(samples) {
		t.Fatal(len(back), err)
	}
	for i := range samples {
		if math.Abs(float64(back[i]-samples[i])) > 1.0/32768 {
			t.Fatal(i, back[i], samples[i])
		}
	}
	short, _ := EncodeFLAC([]float32{0.1, 0.2})
	if s, _ := DecodeFLAC(short); len(s) != minFLACSamples {
		t.Fatal("short audio is padded", len(s))
	}
}

func TestAudioFilesAndDeletes(t *testing.T) {
	s, _ := tempStore(t)
	if _, err := s.SaveAudio(nil); err == nil {
		t.Fatal("empty audio saved")
	}
	name, err := s.SaveAudio([]float32{0.1, -0.1, 0.2, 0.3})
	if err != nil || !strings.HasSuffix(name, ".flac") {
		t.Fatal(name, err)
	}
	if s.AudioPath("../../etc/"+name) != filepath.Join(s.AudioDir(), name) {
		t.Fatal("path escapes the audio folder")
	}
	e := entry("with audio", 0)
	e.AudioFile = name
	id, _ := s.InsertEntry(e)
	if u, _ := s.StorageUse(); u.Entries != 1 || u.AudioFiles != 1 || u.DatabaseBytes == 0 {
		t.Fatal(u)
	}
	if ok, _ := s.Delete(id); !ok {
		t.Fatal("delete")
	}
	if _, err := os.Stat(s.AudioPath(name)); err == nil {
		t.Fatal("audio file stays after delete")
	}
	if ok, _ := s.Delete(id); ok {
		t.Fatal("second delete")
	}
	s.InsertEntry(entry("x", 0))
	if n, err := s.ClearAll(); err != nil || n != 1 {
		t.Fatal(n, err)
	}
}

func TestRetention(t *testing.T) {
	s, _ := tempStore(t)
	for _, age := range []time.Duration{0, 10 * 24 * time.Hour, 40 * 24 * time.Hour} {
		name, _ := s.SaveAudio([]float32{0.1, 0.2})
		e := entry("x", age)
		e.AudioFile = name
		s.InsertEntry(e)
	}
	audioDays, textDays := uint32(7), uint32(30)
	r, err := s.ApplyRetention(nil, &audioDays, baseTime)
	if err != nil || r.EntriesDeleted != 0 || r.AudioDeleted != 2 {
		t.Fatal(r, err)
	}
	if n, _ := s.Count(HistoryQuery{HasAudio: true}); n != 1 {
		t.Fatal("audio left", n)
	}
	r, _ = s.ApplyRetention(&textDays, nil, baseTime)
	if r.EntriesDeleted != 1 {
		t.Fatal(r)
	}
}

func TestDictionary(t *testing.T) {
	s, _ := tempStore(t)
	a, _ := s.AddWord("  Sayso ")
	b, _ := s.AddWord("sayso")
	if a.ID != b.ID || a.Text != "Sayso" {
		t.Fatal(a, b)
	}
	if _, err := s.AddWord(" "); err == nil {
		t.Fatal("empty word")
	}
	r, _ := s.AddReplacement(Replacement{From: " git hub ", To: "GitHub"})
	if r.From != "git hub" || r.ID == 0 {
		t.Fatal(r)
	}
	if _, err := s.AddReplacement(Replacement{From: "  "}); err == nil {
		t.Fatal("empty from")
	}
	s.RecordReplacementUses([]AppliedReplacement{{ReplacementID: r.ID, Count: 3}})
	got, ok, _ := s.GetReplacement(r.ID)
	if !ok || got.Uses != 3 {
		t.Fatal(got)
	}
	r.To = "Github"
	if err := s.UpdateReplacement(r); err != nil {
		t.Fatal(err)
	}
	list, _ := s.ListReplacements()
	if len(list) != 1 || list[0].To != "Github" || list[0].Uses != 3 {
		t.Fatal(list)
	}
}

func TestWaveformSummary(t *testing.T) {
	if w := WaveformSummary(nil, 4); len(w) != 4 || w[0] != 0 {
		t.Fatal(w)
	}
	samples := []float32{0.5, -0.5, 1, -1, 0, 0, 0.25, 0.25}
	w := WaveformSummary(samples, 4)
	if w[1] != 255 || w[2] != 0 || w[0] != 128 || w[3] != 64 {
		t.Fatal(w)
	}
}

func TestPindropImport(t *testing.T) {
	dir := t.TempDir()
	db, _ := sql.Open("sqlite", "file:"+filepath.ToSlash(filepath.Join(dir, "default.store")))
	originals, _ := plist.Marshal(map[string]any{
		"$archiver": "NSKeyedArchiver",
		"$top":      map[string]any{"root": plist.UID(1)},
		"$objects":  []any{"$null", map[string]any{"NS.objects": []any{plist.UID(2), plist.UID(3)}}, "git hub", "get hub"},
	}, plist.BinaryFormat)
	for _, q := range []string{
		"CREATE TABLE ZVOCABULARYWORD (Z_PK INTEGER PRIMARY KEY, ZWORD TEXT)",
		"INSERT INTO ZVOCABULARYWORD (ZWORD) VALUES ('Fenneko'), ('  ')",
		"CREATE TABLE ZWORDREPLACEMENT (Z_PK INTEGER PRIMARY KEY, ZORIGINALS BLOB, ZREPLACEMENT TEXT, ZMATCHMODERAWVALUE TEXT)",
		"CREATE TABLE ZTRANSCRIPTIONRECORD (Z_PK INTEGER PRIMARY KEY, ZID BLOB, ZTEXT TEXT, ZTIMESTAMP REAL, ZDURATION REAL, ZENHANCEDWITH TEXT, ZDESTINATIONAPPNAME TEXT, ZDESTINATIONAPPBUNDLEID TEXT)",
		"INSERT INTO ZTRANSCRIPTIONRECORD (ZID, ZTEXT, ZTIMESTAMP, ZDURATION, ZENHANCEDWITH, ZDESTINATIONAPPNAME, ZDESTINATIONAPPBUNDLEID) VALUES (x'ABCD', 'Hello there', 800000000.0, 2.5, 'gpt', 'Mail', 'com.apple.mail')",
	} {
		if _, err := db.Exec(q); err != nil {
			t.Fatal(err)
		}
	}
	db.Exec("INSERT INTO ZWORDREPLACEMENT (ZORIGINALS, ZREPLACEMENT, ZMATCHMODERAWVALUE) VALUES (?, 'GitHub', 'exact')", originals)
	db.Close()
	data, err := ReadPindrop(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(data.Words) != 1 || len(data.Replacements) != 2 || !data.Replacements[0].CaseSensitive || len(data.Dictations) != 1 {
		t.Fatalf("%+v", data)
	}
	d := data.Dictations[0]
	if d.ID != "abcd" || d.DurationMs != 2500 || d.CreatedAt.UnixMilli() != 1778307197500 {
		t.Fatalf("%+v", d)
	}
	s, _ := tempStore(t)
	r, err := s.ImportPindrop(data)
	if err != nil || r != (ImportReport{1, 2, 1}) {
		t.Fatal(r, err)
	}
	again, _ := s.ImportPindrop(data)
	if again != (ImportReport{}) {
		t.Fatal("second import added", again)
	}
	list, _ := s.List(HistoryQuery{})
	if list[0].StyleID != PindropID || list[0].Enhance.Model != "gpt" {
		t.Fatalf("%+v", list[0])
	}
	var importID string
	s.db.QueryRow("SELECT import_id FROM history").Scan(&importID)
	if importID != "pindrop:abcd" {
		t.Fatal(importID)
	}
}
