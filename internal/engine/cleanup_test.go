package engine

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// fakeCleanupClient runs the test binary as the cleanup runtime, and gives
// the file where it notes each graceful stop.
func fakeCleanupClient(t *testing.T) (*CleanupClient, string) {
	t.Helper()
	exe, _ := os.Executable()
	marker := filepath.Join(t.TempDir(), "exits")
	t.Setenv("SAYSO_FAKE", "cleanup")
	t.Setenv("SAYSO_FAKE_CLEANUP_EXIT", marker)
	c := NewCleanupClient(exe)
	t.Cleanup(c.Stop)
	return c, marker
}

func gracefulExits(marker string) int {
	b, _ := os.ReadFile(marker)
	return strings.Count(string(b), "graceful")
}

func TestCleanupStopIsGracefulAndIdempotent(t *testing.T) {
	c, marker := fakeCleanupClient(t)
	if err := c.EnsureLoaded("s1", "/m.gguf", 2048); err != nil {
		t.Fatal(err)
	}
	c.Stop()
	c.Stop()
	if c.IsRunning() || gracefulExits(marker) != 1 {
		t.Fatalf("running %v, graceful exits %d", c.IsRunning(), gracefulExits(marker))
	}
	// Turning AI off and blocking the loaded model stop it gracefully too.
	c.EnsureLoaded("s1", "/m.gguf", 2048)
	c.Block("s1")
	c.Unblock("s1")
	c.EnsureLoaded("s1", "/m.gguf", 2048)
	c.SetEnabled(false)
	if gracefulExits(marker) != 3 {
		t.Fatalf("graceful exits %d, want 3", gracefulExits(marker))
	}
}

func TestCleanupIdleStopsAndTheNextRequestReloads(t *testing.T) {
	c, marker := fakeCleanupClient(t)
	c.idleLimit = 150 * time.Millisecond
	if err := c.EnsureLoaded("s1", "/m.gguf", 2048); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(5 * time.Second)
	for c.IsRunning() && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if c.IsRunning() || gracefulExits(marker) != 1 {
		t.Fatalf("not stopped when idle: running %v, exits %d", c.IsRunning(), gracefulExits(marker))
	}
	err := c.With("s1", "/m.gguf", 2048, func(s *CleanupSession) error {
		r, err := s.Complete("again", nil, 10, time.Second)
		if err == nil && r.Text != " again." {
			err = errors.New(r.Text)
		}
		return err
	})
	if err != nil || !c.IsRunning() {
		t.Fatal(err)
	}
}

func TestCleanupContextCancelKillsTheRequest(t *testing.T) {
	c, marker := fakeCleanupClient(t)
	ctx, cancel := context.WithCancel(context.Background())
	time.AfterFunc(100*time.Millisecond, cancel)
	started := time.Now()
	err := c.WithContext(ctx, "s1", "/m.gguf", 2048, func(s *CleanupSession) error {
		_, err := s.Complete("hang", nil, 10, 0)
		return err
	})
	if !errors.Is(err, ErrCleanupCancelled) || time.Since(started) > 3*time.Second {
		t.Fatal(err, time.Since(started))
	}
	if c.IsRunning() || gracefulExits(marker) != 0 {
		t.Fatal("a cancelled request must kill the process")
	}
	// An ended context stops a session before it starts.
	if err := c.WithContext(ctx, "s1", "/m.gguf", 2048, func(*CleanupSession) error { return nil }); !errors.Is(err, ErrCleanupCancelled) {
		t.Fatal(err)
	}
	if err := c.EnsureLoaded("s1", "/m.gguf", 2048); err != nil {
		t.Fatal(err)
	}
}

func TestCleanupCrashSaysWhy(t *testing.T) {
	c, _ := fakeCleanupClient(t)
	err := c.With("s1", "/m.gguf", 2048, func(s *CleanupSession) error {
		_, err := s.Complete("crash", nil, 10, time.Second)
		return err
	})
	if err == nil || !strings.Contains(err.Error(), "cannot load libffi.so.8") || c.IsRunning() {
		t.Fatal(err)
	}
}

func TestCleanupWithoutLibraries(t *testing.T) {
	c, _ := fakeCleanupClient(t)
	t.Setenv("SAYSO_FAKE_CLEANUP_LIBS", "missing")
	if err := c.EnsureLoaded("s1", "/m.gguf", 2048); !errors.Is(err, ErrCleanupNoLibraries) || c.IsRunning() {
		t.Fatal(err)
	}
	t.Setenv("SAYSO_FAKE_CLEANUP_LIBS", "found")
	if err := c.EnsureLoaded("s1", "/m.gguf", 2048); err != nil {
		t.Fatal(err)
	}
}

func TestCleanupPathFor(t *testing.T) {
	for _, c := range []struct{ exe, goos, want string }{
		{"/A/Sayso.app/Contents/MacOS/Sayso", "darwin", "/A/Sayso.app/Contents/MacOS/sayso-cleanup"},
		{"/usr/lib/sayso/sayso", "linux", "/usr/lib/sayso/llama/sayso-cleanup"},
		{"/P/Sayso/Sayso.exe", "windows", "/P/Sayso/llama/sayso-cleanup.exe"},
	} {
		if got := filepath.ToSlash(CleanupPathFor(filepath.FromSlash(c.exe), c.goos)); got != c.want {
			t.Errorf("%s: %s, want %s", c.goos, got, c.want)
		}
	}
	t.Setenv("SAYSO_CLEANUP_PATH", "/opt/sayso-cleanup")
	if DefaultCleanupPath() != "/opt/sayso-cleanup" {
		t.Fatal(DefaultCleanupPath())
	}
}
