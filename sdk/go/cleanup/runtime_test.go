package cleanup_test

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/cleanup"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
)

func TestMain(m *testing.M) {
	code := m.Run()
	testbin.Cleanup()
	os.Exit(code)
}

// fakeRuntime is a client of sayso-fake-cleanup with a script of faults.
func fakeRuntime(t *testing.T, script string) *cleanup.Client {
	t.Helper()
	t.Setenv("SAYSO_FAKE_SCRIPT", script)
	c := cleanup.NewClient(testbin.Path(t, testbin.FakeCleanup))
	t.Cleanup(c.Stop)
	return c
}

// readyModel writes a model file and its ready.json, as a download does.
func readyModel(t *testing.T, id string) (string, *cleanup.ModelInfo) {
	t.Helper()
	dir := t.TempDir()
	m, _ := cleanup.Find(id)
	mdir := cleanup.ModelDir(dir, m)
	os.MkdirAll(mdir, 0o755)
	os.WriteFile(filepath.Join(mdir, m.File), []byte("gguf"), 0o644)
	ready, _ := json.Marshal(map[string]string{"revision": m.Revision, "sha256": m.SHA256, "file": m.File})
	os.WriteFile(filepath.Join(mdir, "ready.json"), ready, 0o644)
	if !cleanup.IsReady(dir, m) {
		t.Fatal("not ready")
	}
	return dir, m
}

func TestTheRuntimeSaysHelloLoadsCountsCompletesAndUnloads(t *testing.T) {
	testbin.Passed(t, "C03")
	c := fakeRuntime(t, "")
	dir, m := readyModel(t, "spokenly-cleanup-1")
	path, _ := cleanup.ModelPath(dir, m)
	err := c.With(m.ID, path, cleanup.NCtx, func(s *cleanup.Session) error {
		n, err := s.Count("send the report by friday", time.Second)
		if err != nil || n != 5 {
			t.Fatal(n, err)
		}
		done, err := s.Complete("text: um send it\nout:", []string{}, 20, time.Second)
		if err != nil || strings.TrimSpace(done.Text) != "Send it." || done.Stop != cleanup.StopEnd {
			t.Fatalf("%+v %v", done, err)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if h, ok := c.Hello(); !ok || h.Version != cleanup.ProtocolVersion || h.Libraries != "found" {
		t.Fatalf("%+v", h)
	}
	if err := c.Unload(); err != nil {
		t.Fatal(err)
	}
	// After the unload the next session loads the model again.
	if err := c.With(m.ID, path, cleanup.NCtx, func(s *cleanup.Session) error {
		_, err := s.Count("x", time.Second)
		return err
	}); err != nil {
		t.Fatal(err)
	}
}

func TestACleanerPlansChecksAndJoins(t *testing.T) {
	c := fakeRuntime(t, "")
	dir, _ := readyModel(t, "spokenly-cleanup-1")
	cl := &cleanup.Cleaner{Runtime: c, ModelsDir: dir}
	resp, err := cl.Clean(context.Background(), cleanup.Request{Model: "spokenly-cleanup-1", Text: "um send the report by friday", Timeout: 5 * time.Second})
	if err != nil || resp.Text != "Send the report by friday." || resp.Parts != 1 || resp.Model != "spokenly-cleanup-1" {
		t.Fatalf("%+v %v", resp, err)
	}
	var ce *cleanup.Error
	if _, err := cl.Clean(context.Background(), cleanup.Request{Model: "s1-mini", Text: "x"}); !errors.As(err, &ce) || ce.Kind != "not_configured" {
		t.Fatal(err)
	}
	broken := &cleanup.Cleaner{Runtime: cleanup.NewClient(filepath.Join(dir, "missing")), ModelsDir: dir}
	if _, err := broken.Clean(context.Background(), cleanup.Request{Model: "spokenly-cleanup-1", Text: "x"}); !errors.As(err, &ce) || ce.Kind != "not_configured" {
		t.Fatal(err)
	}
}

func TestARuntimeWithoutLibrariesSaysSo(t *testing.T) {
	c := fakeRuntime(t, `{"libraries":"missing"}`)
	dir, m := readyModel(t, "spokenly-cleanup-1")
	path, _ := cleanup.ModelPath(dir, m)
	if err := c.EnsureLoaded(m.ID, path, cleanup.NCtx); !errors.Is(err, cleanup.ErrNoLibraries) || c.IsRunning() {
		t.Fatal(err)
	}
	cl := &cleanup.Cleaner{Runtime: c, ModelsDir: dir}
	var ce *cleanup.Error
	if _, err := cl.Clean(context.Background(), cleanup.Request{Model: m.ID, Text: "x"}); !errors.As(err, &ce) || !ce.NoLibraries {
		t.Fatal(err)
	}
}

func TestARequestPastItsDeadlineKillsTheRuntime(t *testing.T) {
	c := fakeRuntime(t, `{"delay_ms":{"complete":3000}}`)
	dir, m := readyModel(t, "spokenly-cleanup-1")
	path, _ := cleanup.ModelPath(dir, m)
	started := time.Now()
	err := c.With(m.ID, path, cleanup.NCtx, func(s *cleanup.Session) error {
		_, err := s.Complete("text: hi\nout:", nil, 10, 200*time.Millisecond)
		return err
	})
	if !errors.Is(err, cleanup.ErrTimeout) || time.Since(started) > 2*time.Second || c.IsRunning() {
		t.Fatal(err, time.Since(started))
	}
}

func TestACrashSaysWhy(t *testing.T) {
	c := fakeRuntime(t, `{"stderr":["cannot load libffi.so.8"],"crash_at":2}`)
	dir, m := readyModel(t, "spokenly-cleanup-1")
	path, _ := cleanup.ModelPath(dir, m)
	err := c.With(m.ID, path, cleanup.NCtx, func(s *cleanup.Session) error {
		_, err := s.Complete("text: hi\nout:", nil, 10, time.Second)
		return err
	})
	if err == nil || !strings.Contains(err.Error(), "cannot load libffi.so.8") || c.IsRunning() {
		t.Fatal(err)
	}
}

func TestThePathOfTheRuntimeBesideTheApp(t *testing.T) {
	for _, c := range []struct{ exe, goos, want string }{
		{"/A/Sayso.app/Contents/MacOS/Sayso", "darwin", "/A/Sayso.app/Contents/MacOS/sayso-cleanup"},
		{"/usr/lib/sayso/sayso", "linux", "/usr/lib/sayso/llama/sayso-cleanup"},
		{"/P/Sayso/Sayso.exe", "windows", "/P/Sayso/llama/sayso-cleanup.exe"},
	} {
		if got := filepath.ToSlash(cleanup.PathFor(filepath.FromSlash(c.exe), c.goos)); got != c.want {
			t.Errorf("%s: %s, want %s", c.goos, got, c.want)
		}
	}
	t.Setenv("SAYSO_CLEANUP_PATH", "/opt/sayso-cleanup")
	if cleanup.DefaultPath() != "/opt/sayso-cleanup" {
		t.Fatal(cleanup.DefaultPath())
	}
}
