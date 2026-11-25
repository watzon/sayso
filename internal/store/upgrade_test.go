package store

import (
	"database/sql"
	"os"
	"path/filepath"
	"testing"
)

// A sayso.db that the Rust app of 0.14 to 0.16 made (its own schema, at
// user_version 3, with rows in its JSON shapes) opens without a migration
// and keeps its history, its search index, and its dictionary.
func TestARustDatabaseOpensWithItsHistoryAndDictionary(t *testing.T) {
	schema, err := os.ReadFile(filepath.Join("testdata", "sayso-0.16-schema.sql"))
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	path := filepath.Join(dir, "sayso.db")
	raw, err := sql.Open("sqlite", "file:"+filepath.ToSlash(path))
	if err != nil {
		t.Fatal(err)
	}
	for _, stmt := range []string{
		string(schema),
		"PRAGMA user_version = 3",
		`INSERT INTO history (created_at, duration_ms, app, transcript, final_text, style_id, model, transcribe_ms, replacements, enhance, insert_result, audio_file, waveform, diagnostics)
		 VALUES (1763900000000, 4200, '{"bundle_id":"com.apple.mail","name":"Mail"}', 'send the report to git hub', 'Send the report to GitHub.', 'email', 'parakeet-tdt-v3', 140,
		         '[{"replacement_id":1,"from":"git hub","to":"GitHub","count":1}]',
		         '{"result":"applied","provider_id":"claude-code","model":"sonnet","elapsed_ms":1450}',
		         '{"result":"pasted","clipboard_restored":true}', '1763900000000.flac', x'0102030405',
		         '{"app_version":"0.16.0","system":{"os":"macos","os_version":"27.0","arch":"aarch64","session":null,"ui":null,"desktop":null},"start_to_capture_ms":35,"stop_to_capture_end_ms":12,"stop_to_request_ms":20,"stop_to_insert_ms":1700,"transcribe":{"step_ms":140,"model_wait_ms":0,"cloud":false,"fallback":false,"language":"en","live_preview":true},"enhance":null,"insert":null,"changed_later":false}')`,
		`INSERT INTO history (created_at, duration_ms, app, transcript, final_text, style_id, model, transcribe_ms, replacements, enhance, insert_result, waveform, import_id)
		 VALUES (1700000000000, 2500, NULL, 'hello from pindrop', 'Hello from Pindrop.', 'raw', 'pindrop', 0, '[]', '{"result":"not_used"}', '{"result":"typed"}', x'', 'pindrop:ABCD')`,
		"INSERT INTO words (text) VALUES ('Fenneko'), ('Sayso')",
		"INSERT INTO replacements (from_text, to_text, case_sensitive, uses) VALUES ('git hub', 'GitHub', 0, 12)",
	} {
		if _, err := raw.Exec(stmt); err != nil {
			t.Fatalf("%v\n%s", err, stmt)
		}
	}
	raw.Close()

	s, err := Open(path, filepath.Join(dir, "audio"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if v, err := s.SchemaVersion(); err != nil || v != 3 || CurrentVersion != 3 {
		t.Fatalf("schema version %d (%v), this build knows %d", v, err, CurrentVersion)
	}

	list, err := s.List(HistoryQuery{})
	if err != nil || len(list) != 2 {
		t.Fatalf("history: %v %v", list, err)
	}
	e := list[0]
	if e.FinalText != "Send the report to GitHub." || e.StyleID != "email" || e.App == nil || e.App.BundleID != "com.apple.mail" ||
		e.AudioFile != "1763900000000.flac" || len(e.Replacements) != 1 || e.Replacements[0].To != "GitHub" ||
		e.Enhance.Result != EnhanceApplied || e.Enhance.ProviderID != "claude-code" || e.Insert.Result != InsertPasted || !e.Insert.ClipboardRestored {
		t.Fatalf("the newest entry changed: %+v", e)
	}
	if e.Diagnostics == nil || e.Diagnostics.AppVersion != "0.16.0" || e.Diagnostics.Transcribe.StepMs != 140 {
		t.Fatalf("diagnostics: %+v", e.Diagnostics)
	}
	if list[1].Diagnostics != nil || list[1].FinalText != "Hello from Pindrop." {
		t.Fatalf("the imported entry changed: %+v", list[1])
	}

	found, err := s.List(HistoryQuery{Text: "pindr"})
	if err != nil || len(found) != 1 || found[0].ID != list[1].ID {
		t.Fatalf("search in the old index: %v %v", found, err)
	}

	words, err := s.ListWords()
	if err != nil || len(words) != 2 {
		t.Fatalf("words: %v %v", words, err)
	}
	reps, err := s.ListReplacements()
	if err != nil || len(reps) != 1 || reps[0].From != "git hub" || reps[0].To != "GitHub" || reps[0].Uses != 12 {
		t.Fatalf("replacements: %v %v", reps, err)
	}

	// A new entry after the upgrade goes into the same index.
	if _, err := s.InsertEntry(entry("written by the new app", 0)); err != nil {
		t.Fatal(err)
	}
	if found, err := s.List(HistoryQuery{Text: "written"}); err != nil || len(found) != 1 {
		t.Fatalf("search after an insert: %v %v", found, err)
	}
}
