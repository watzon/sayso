package voice

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/watzon/sayso/mobile/core/styles"
)

func openStore(t *testing.T, path string) *Store {
	t.Helper()
	s, err := OpenStore(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func TestStylesPersist(t *testing.T) {
	path := filepath.Join(t.TempDir(), "voice.journal")
	s := openStore(t, path)
	if err := s.PutStyle(Style{ID: "pirate", Name: "Pirate", Prompt: "Talk like a pirate.\nArr."}); err != nil {
		t.Fatal(err)
	}
	if err := s.PutStyle(Style{ID: "tidy", Name: "Tidy"}); err != nil {
		t.Fatal(err)
	}
	if err := s.PutStyle(Style{ID: "pirate", Name: "Pirate 2", Prompt: "Arr."}); err != nil {
		t.Fatal(err)
	}
	if err := s.RemoveStyle("tidy"); err != nil {
		t.Fatal(err)
	}
	s.Close()

	s = openStore(t, path)
	all := s.Styles()
	if len(all) != len(styles.Builtins)+1 {
		t.Fatalf("%d styles: %+v", len(all), all)
	}
	for i, b := range styles.Builtins {
		if all[i].ID != b.ID || !all[i].BuiltIn {
			t.Fatalf("built-ins first, in order: %+v", all[i])
		}
	}
	if got := all[len(all)-1]; got != (Style{ID: "pirate", Name: "Pirate 2", Prompt: "Arr."}) {
		t.Fatalf("custom style %+v", got)
	}
	if st, ok := s.Style("email"); !ok || !st.BuiltIn || st.Prompt == "" {
		t.Fatalf("built-in lookup %+v", st)
	}
}

func TestStyleRules(t *testing.T) {
	s := openStore(t, "")
	if s.PutStyle(Style{ID: styles.DefaultID, Name: "Mine"}) != ErrBuiltinStyle || s.RemoveStyle(styles.RawID) != ErrBuiltinStyle {
		t.Fatal("built-in styles must be refused")
	}
	for _, bad := range []Style{
		{ID: "", Name: "x"}, {ID: "Has Caps", Name: "x"}, {ID: "ok", Name: ""},
		{ID: strings.Repeat("a", MaxIDLen+1), Name: "x"}, {ID: "ok", Name: "x", Prompt: strings.Repeat("p", MaxStylePrompt+1)},
	} {
		if s.PutStyle(bad) != ErrBadStyle {
			t.Errorf("accepted %+v", bad)
		}
	}
	for i := 0; i < MaxStyles; i++ {
		if err := s.PutStyle(Style{ID: "s" + strings.Repeat("x", i%40) + string(rune('a'+i%26)) + string(rune('a'+i/26)), Name: "n"}); err != nil {
			t.Fatal(i, err)
		}
	}
	if s.PutStyle(Style{ID: "one-more", Name: "n"}) != ErrTooManyStyle {
		t.Fatal("no limit on custom styles")
	}
}

func TestHistory(t *testing.T) {
	path := filepath.Join(t.TempDir(), "voice.journal")
	s := openStore(t, path)
	a := s.AddEntry(Entry{TimestampMs: 1000, DurationMs: 1500, EngineID: "whisper-base", StyleID: "clean", Raw: "hello world", Final: "Hello, world."})
	b := s.AddEntry(Entry{TimestampMs: 2000, DurationMs: 700, EngineID: "cloud:groq", StyleID: "raw", Raw: "tab\there\nnew line", Final: "x"})
	c := s.AddEntry(Entry{TimestampMs: 3000, Raw: "secret third", Final: "secret third"})
	if a != 1 || b != 2 || c != 3 {
		t.Fatal("ids", a, b, c)
	}
	s.DeleteEntry(c)
	s.Close()

	data, _ := os.ReadFile(path)
	if strings.Contains(string(data), "secret third") {
		t.Fatal("a deleted entry's text is still in the file")
	}
	s = openStore(t, path)
	got := s.Entries(0)
	if len(got) != 2 || got[0].ID != b || got[1].ID != a {
		t.Fatalf("newest first: %+v", got)
	}
	if got[0].Raw != "tab\there\nnew line" || got[1] != (Entry{ID: a, TimestampMs: 1000, DurationMs: 1500, EngineID: "whisper-base", StyleID: "clean", Raw: "hello world", Final: "Hello, world."}) {
		t.Fatalf("round trip: %+v", got)
	}
	if one := s.Entries(1); len(one) != 1 || one[0].ID != b {
		t.Fatalf("limit: %+v", one)
	}
	if id := s.AddEntry(Entry{Final: "after reopen"}); id != 4 {
		t.Fatalf("ids are not reused: %d", id)
	}
	s.ClearEntries()
	if len(s.Entries(0)) != 0 {
		t.Fatal("clear")
	}
	s.Close()
	data, _ = os.ReadFile(path)
	if strings.Contains(string(data), "hello world") || strings.Contains(string(data), "after reopen") {
		t.Fatal("cleared text is still in the file")
	}
	s = openStore(t, path)
	if id := s.AddEntry(Entry{}); id != 5 {
		t.Fatalf("ids survive a clear: %d", id)
	}
}

func TestHistoryBounds(t *testing.T) {
	path := filepath.Join(t.TempDir(), "voice.journal")
	s := openStore(t, path)
	long := strings.Repeat("é", MaxEntryText) // 2 bytes each
	for i := 0; i < 3*MaxEntries; i++ {
		s.AddEntry(Entry{TimestampMs: int64(i), EngineID: strings.Repeat("e", 200), Raw: long, Final: long})
	}
	if err := s.Err(); err != nil {
		t.Fatal(err)
	}
	got := s.Entries(0)
	if len(got) != MaxEntries || got[0].TimestampMs != 3*MaxEntries-1 {
		t.Fatalf("%d entries, newest %d", len(got), got[0].TimestampMs)
	}
	if len(got[0].Raw) > MaxEntryText || !strings.HasPrefix(long, got[0].Raw) || len(got[0].EngineID) != MaxIDLen {
		t.Fatalf("not cut on a rune boundary: %d bytes", len(got[0].Raw))
	}
	st, _ := os.Stat(path)
	state := int64(MaxEntries * (2*MaxEntryText + MaxIDLen + 40))
	if st.Size() > 2*state+storeCompactMin {
		t.Fatalf("journal %d bytes: compaction does not bound it (state about %d)", st.Size(), state)
	}
	s.Close()
	if got := openStore(t, path).Entries(0); len(got) != MaxEntries {
		t.Fatalf("%d entries after reopen", len(got))
	}
}
