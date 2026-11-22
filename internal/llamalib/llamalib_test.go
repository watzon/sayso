package llamalib

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"
)

func sum(b []byte) string {
	h := sha256.Sum256(b)
	return hex.EncodeToString(h[:])
}

// testArchive is a tar.gz with the libraries of runtime.GOOS under root/, one
// file Sayso does not keep, and a hostile name.
func testArchive(t *testing.T) ([]byte, *Asset) {
	t.Helper()
	var buf bytes.Buffer
	gz := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gz)
	a := &Asset{GOOS: runtime.GOOS, GOARCH: runtime.GOARCH, Backend: "cpu", Root: "llama-b1/"}
	add := func(name string, body []byte, keep bool) {
		tw.WriteHeader(&tar.Header{Name: name, Mode: 0o755, Size: int64(len(body)), Typeflag: tar.TypeReg})
		tw.Write(body)
		if keep {
			a.Files = append(a.Files, File{Name: strings.TrimPrefix(name, a.Root), SHA256: sum(body), Size: int64(len(body))})
		}
	}
	for _, n := range LibraryNames(runtime.GOOS) {
		add(a.Root+n, []byte("lib "+n), true)
	}
	add(a.Root+"llama-server", []byte("not kept"), false)
	add("../../escape", []byte("never written"), false)
	tw.Close()
	gz.Close()
	a.SHA256, a.Size = sum(buf.Bytes()), int64(buf.Len())
	return buf.Bytes(), a
}

func TestFetchChecksAndKeepsOnlyThePinnedFiles(t *testing.T) {
	body, a := testArchive(t)
	var hits atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		w.Write(body)
	}))
	defer srv.Close()
	a.URL = srv.URL + "/llama.tar.gz"
	dir := t.TempDir()
	dest := filepath.Join(dir, "libs", a.Name())
	archives := filepath.Join(dir, "archives")
	if err := Fetch(context.Background(), srv.Client(), a, dest, archives); err != nil {
		t.Fatal(err)
	}
	if !HasLibraries(dest, runtime.GOOS) || Verify(dest, a) != nil {
		t.Fatal("the libraries are not there")
	}
	entries, _ := os.ReadDir(dest)
	if len(entries) != len(a.Files) {
		t.Fatalf("%d files, want %d", len(entries), len(a.Files))
	}
	if _, err := os.Stat(filepath.Join(dir, "escape")); err == nil {
		t.Fatal("a name in the archive chose a path")
	}
	// The archive cache serves a second fetch, and a fetch of a whole folder
	// does nothing.
	os.RemoveAll(dest)
	if err := Fetch(context.Background(), srv.Client(), a, dest, archives); err != nil {
		t.Fatal(err)
	}
	if err := Fetch(context.Background(), srv.Client(), a, dest, archives); err != nil {
		t.Fatal(err)
	}
	if hits.Load() != 1 {
		t.Fatalf("%d downloads, want 1", hits.Load())
	}
	// A changed file fails the check.
	os.WriteFile(filepath.Join(dest, a.Files[0].Name), []byte("changed"), 0o755)
	if err := Verify(dest, a); err == nil {
		t.Fatal("a changed file passed")
	}
}

// llama.cpp loads every ggml backend in the folder, so a file that is not
// pinned fails the check, and a fetch makes the folder again without it.
func TestAnUnpinnedFileFailsTheCheck(t *testing.T) {
	body, a := testArchive(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write(body) }))
	defer srv.Close()
	a.URL = srv.URL + "/llama.tar.gz"
	dest := filepath.Join(t.TempDir(), a.Name())
	if err := Fetch(context.Background(), srv.Client(), a, dest, ""); err != nil {
		t.Fatal(err)
	}
	extra := filepath.Join(dest, "libggml-evil.so")
	os.WriteFile(extra, []byte("not pinned"), 0o755)
	if err := Verify(dest, a); err == nil || !strings.Contains(err.Error(), "libggml-evil.so") {
		t.Fatal(err)
	}
	if err := Fetch(context.Background(), srv.Client(), a, dest, ""); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(extra); err == nil || Verify(dest, a) != nil {
		t.Fatal("the fetch kept the unpinned file")
	}
}

func TestFetchRefusesAWrongArchive(t *testing.T) {
	body, a := testArchive(t)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write(append(body[:len(body)-1:len(body)-1], body[len(body)-1]^1))
	}))
	defer srv.Close()
	a.URL = srv.URL + "/llama.tar.gz"
	dir := t.TempDir()
	dest := filepath.Join(dir, a.Name())
	err := Fetch(context.Background(), srv.Client(), a, dest, "")
	if err == nil || !strings.Contains(err.Error(), "SHA-256") {
		t.Fatal(err)
	}
	if _, err := os.Stat(dest); err == nil {
		t.Fatal("a wrong archive left a folder")
	}
	if left, _ := os.ReadDir(dir); len(left) != 0 {
		t.Fatalf("temporary files left: %v", left)
	}
}

func TestLocateOrder(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv(EnvLibDir, "")
	libs := func() string {
		d := t.TempDir()
		for _, n := range LibraryNames(runtime.GOOS) {
			os.WriteFile(filepath.Join(d, n), []byte("x"), 0o755)
		}
		return d
	}
	// The package folder of the runtime.
	pkg := t.TempDir()
	exe := filepath.Join(pkg, "sayso-cleanup")
	bundled := BundledDir(exe, runtime.GOOS)
	os.MkdirAll(bundled, 0o755)
	if _, _, err := Locate(exe); err != ErrNotFound {
		t.Fatalf("empty: %v", err)
	}
	for _, n := range LibraryNames(runtime.GOOS) {
		os.WriteFile(filepath.Join(bundled, n), []byte("x"), 0o755)
	}
	if dir, from, err := Locate(exe); err != nil || from != FromBundle || dir != filepath.Clean(bundled) {
		t.Fatal(dir, from, err)
	}
	// The variable wins, and a folder without libraries is an error.
	env := libs()
	t.Setenv(EnvLibDir, env)
	if dir, from, err := Locate(exe); err != nil || from != FromEnv || dir != env {
		t.Fatal(dir, from, err)
	}
	t.Setenv(EnvLibDir, t.TempDir())
	if _, _, err := Locate(exe); err == nil || err == ErrNotFound {
		t.Fatal(err)
	}
}

func TestBundledDirAndPins(t *testing.T) {
	if got := BundledDir("/A/Sayso.app/Contents/MacOS/sayso-cleanup", "darwin"); filepath.ToSlash(got) != "/A/Sayso.app/Contents/Frameworks/llama" {
		t.Fatal(got)
	}
	if got := BundledDir("/usr/lib/sayso/llama/sayso-cleanup", "linux"); filepath.ToSlash(got) != "/usr/lib/sayso/llama" {
		t.Fatal(got)
	}
	for _, p := range [][2]string{{"darwin", "arm64"}, {"darwin", "amd64"}, {"linux", "amd64"}, {"linux", "arm64"}, {"windows", "amd64"}} {
		a, ok := For(p[0], p[1])
		if !ok || len(a.SHA256) != 64 || a.Size == 0 || !strings.HasPrefix(a.URL, "https://github.com/") {
			t.Fatalf("%v: no pin", p)
		}
		have := map[string]bool{}
		for _, f := range a.Files {
			have[f.Name] = true
			if len(f.SHA256) != 64 {
				t.Fatalf("%v %s: no hash", p, f.Name)
			}
		}
		for _, l := range a.Links {
			have[l.Name] = true
		}
		for _, n := range LibraryNames(p[0]) {
			if !have[n] {
				t.Fatalf("%v: the pins have no %s", p, n)
			}
		}
	}
}
