package enhance

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/engine"
)

// A runtime with no llama.cpp libraries starts one download in the app and
// fails fast, so the dictation gets the text after replacements.
func TestRuntimeCleanerFetchesMissingLibraries(t *testing.T) {
	exe, _ := os.Executable()
	t.Setenv("SAYSO_FAKE", "cleanup")
	t.Setenv("SAYSO_FAKE_CLEANUP_LIBS", "missing")
	dir := t.TempDir()
	m, _ := FindCleanupModel("s1-mini")
	mdir := CleanupModelDir(dir, m)
	os.MkdirAll(mdir, 0o755)
	os.WriteFile(filepath.Join(mdir, m.File), []byte("x"), 0o644)
	ready, _ := json.Marshal(map[string]string{"revision": m.Revision, "sha256": m.SHA256, "file": m.File})
	os.WriteFile(filepath.Join(mdir, "ready.json"), ready, 0o644)
	rt := engine.NewCleanupClient(exe)
	defer rt.Stop()
	var fetches atomic.Int32
	release := make(chan struct{})
	c := &RuntimeCleaner{Runtime: rt, ModelsDir: dir, fetchLibs: func(ctx context.Context) error {
		fetches.Add(1)
		<-release
		return errors.New("offline")
	}}
	for range 2 {
		_, err := c.Clean(CleanupRequest{Model: "s1-mini", Text: "send it", Timeout: 5 * time.Second})
		var ce *CleanError
		if !errors.As(err, &ce) || ce.Kind != "not_configured" {
			t.Fatal(err)
		}
	}
	close(release)
	deadline := time.Now().Add(5 * time.Second)
	for c.fetching.Load() && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if fetches.Load() != 1 {
		t.Fatalf("%d downloads, want 1 while one runs", fetches.Load())
	}
	// With the libraries, the cleanup runs.
	t.Setenv("SAYSO_FAKE_CLEANUP_LIBS", "found")
	if resp, err := c.Clean(CleanupRequest{Model: "s1-mini", Text: "send it", Timeout: 5 * time.Second}); err != nil || resp.Text != "SEND IT" {
		t.Fatal(resp, err)
	}
}
