package main

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"os"
	"path"
	"path/filepath"
	"strings"
	"testing"
)

// The libraries that internal/localengine opens on each platform
// (sherpa_capi.go, whisper_windows.go), and the ONNX Runtime that sherpa-onnx
// links.
func TestPinsHaveTheLibrariesTheEngineOpens(t *testing.T) {
	need := map[string][]string{
		"linux/amd64":   {"libsherpa-onnx-c-api.so", "libonnxruntime.so"},
		"linux/arm64":   {"libsherpa-onnx-c-api.so", "libonnxruntime.so"},
		"windows/amd64": {"sherpa-onnx-c-api.dll", "onnxruntime.dll", "whisper.dll", "ggml.dll", "ggml-base.dll", "ggml-cpu.dll"},
	}
	for platform, names := range need {
		have := map[string]bool{}
		for _, a := range pins[platform] {
			for _, f := range a.files {
				have[path.Base(f.path)] = true
			}
		}
		for _, n := range names {
			if !have[n] {
				t.Errorf("%s: %s is not pinned", platform, n)
			}
		}
	}
}

func sum(b []byte) string {
	h := sha256.Sum256(b)
	return hex.EncodeToString(h[:])
}

func testArchive(t *testing.T) ([]byte, archive) {
	t.Helper()
	var buf bytes.Buffer
	zw := zip.NewWriter(&buf)
	files := map[string]string{"Release/a.dll": "a", "Release/b.dll": "bb", "Release/tool.exe": "x", "../escape.dll": "e"}
	for name, body := range files {
		w, err := zw.Create(name)
		if err != nil {
			t.Fatal(err)
		}
		w.Write([]byte(body))
	}
	if err := zw.Close(); err != nil {
		t.Fatal(err)
	}
	b := buf.Bytes()
	return b, archive{
		url:    "/libs.zip",
		sha256: sum(b), size: int64(len(b)),
		files: []file{
			{"Release/a.dll", sum([]byte("a")), 1},
			{"Release/b.dll", sum([]byte("bb")), 2},
		},
	}
}

func TestFetchTakesOnlyThePinnedFiles(t *testing.T) {
	body, a := testArchive(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write(body) }))
	defer srv.Close()
	a.url = srv.URL + a.url
	dir := t.TempDir()
	dest := filepath.Join(dir, "out", "windows-amd64")
	if err := fetch(context.Background(), srv.Client(), []archive{a}, dest, filepath.Join(dir, "archives")); err != nil {
		t.Fatal(err)
	}
	entries, err := os.ReadDir(dest)
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, e := range entries {
		names = append(names, e.Name())
	}
	if strings.Join(names, ",") != "a.dll,b.dll" {
		t.Fatalf("dest has %v, want a.dll and b.dll", names)
	}
	if _, err := os.Stat(filepath.Join(dir, "out", "escape.dll")); err == nil {
		t.Fatal("an archive name chose a path outside dest")
	}
	// A second run takes the cached archive: the server is gone.
	srv.Close()
	os.RemoveAll(dest)
	if err := fetch(context.Background(), srv.Client(), []archive{a}, dest, filepath.Join(dir, "archives")); err != nil {
		t.Fatalf("second fetch from the cache: %v", err)
	}
}

func TestFetchRefusesAnArchiveWithAnotherSHA256(t *testing.T) {
	body, a := testArchive(t)
	a.sha256 = sum([]byte("something else"))
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write(body) }))
	defer srv.Close()
	a.url = srv.URL + a.url
	dir := t.TempDir()
	dest := filepath.Join(dir, "windows-amd64")
	err := fetch(context.Background(), srv.Client(), []archive{a}, dest, filepath.Join(dir, "archives"))
	if err == nil || !strings.Contains(err.Error(), "SHA-256") {
		t.Fatalf("fetch = %v, want a SHA-256 error", err)
	}
	if _, err := os.Stat(dest); err == nil {
		t.Fatal("dest exists after a failed fetch")
	}
}
