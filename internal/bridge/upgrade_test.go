package bridge

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// apps.toml as the Rust app of 0.16 wrote it (toml::to_string_pretty): one
// record of each identity kind.
const rustApps = `[[apps]]
name = "Notetaker"
team_id = "ABCDE12345"
identifier = "app.justsayso.notetaker"
allowed = true
granted = ["session.live", "audio.mic", "audio.system", "transcribe.file"]
denied = ["dictation.observe"]
first_seen = 2026-10-29T10:00:00Z

[[apps]]
name = "transcribe"
path = "/opt/transcribe/bin/transcribe"
allowed = false
granted = []
denied = ["transcribe.file"]
first_seen = 2026-11-02T08:30:00Z

[[apps]]
name = "Notes"
flatpak = "dev.example.Notes"
allowed = true
granted = ["transcribe.file"]
denied = []
`

// The answers in a Rust apps.toml hold, and a change in Settings keeps the
// other records and the first_seen times.
func TestARustAppsFileKeepsItsAnswersThroughAChange(t *testing.T) {
	path := filepath.Join(t.TempDir(), "apps.toml")
	if err := os.WriteFile(path, []byte(rustApps), 0o600); err != nil {
		t.Fatal(err)
	}
	f := NewAppsFile(path)
	if f.IsBroken() {
		t.Fatal("the file is broken")
	}
	notes := Identity{Kind: IdentityFlatpak, AppID: "dev.example.Notes"}
	tool := Identity{Kind: IdentityPath, Path: "/opt/transcribe/bin/transcribe"}
	for _, id := range []Identity{signedNotetaker, notes, tool} {
		if f.Current().Find(id) == nil {
			t.Fatalf("no record for %+v", id)
		}
	}
	if r := f.Current().Find(tool); r.Allowed || len(r.DeniedCapabilities()) != 1 {
		t.Fatalf("the refusal is gone: %+v", r)
	}

	if err := f.Allow(tool.ID()); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(path)
	again, err := ParseApps(string(b))
	if err != nil {
		t.Fatalf("%v\n%s", err, b)
	}
	if len(again.Apps) != 3 || !again.Find(tool).Allowed || !again.Find(signedNotetaker).Allowed {
		t.Fatalf("after the change:\n%s", b)
	}
	want := map[string]time.Time{
		"Notetaker":  time.Date(2026, 10, 29, 10, 0, 0, 0, time.UTC),
		"transcribe": time.Date(2026, 11, 2, 8, 30, 0, 0, time.UTC),
	}
	for _, r := range again.Apps {
		if w, ok := want[r.Name]; ok {
			if got, isTime := r.FirstSeen.(time.Time); !isTime || !got.Equal(w) {
				t.Fatalf("%s: first_seen is %v", r.Name, r.FirstSeen)
			}
		}
	}
	if !strings.Contains(string(b), `denied = ["dictation.observe"]`) {
		t.Fatalf("the answer of another app changed:\n%s", b)
	}
}
