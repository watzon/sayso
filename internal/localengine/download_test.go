package localengine

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// fetchFixture serves testdata/archives/<name>.tar.bz2 and fetches it.
func fetchFixture(t *testing.T, name string) error {
	t.Helper()
	body, err := os.ReadFile(filepath.Join("testdata", "archives", name+".tar.bz2"))
	if err != nil {
		t.Fatal(err)
	}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write(body) }))
	defer srv.Close()
	dir := t.TempDir()
	return fetchArchive(newHTTPClient(), srv.URL+"/a.tar.bz2", "", filepath.Join(dir, "work"), filepath.Join(dir, "dest"), newProgress(0))
}

func TestAnArchiveWithALinkOrAnUnsafePathIsRefused(t *testing.T) {
	if err := fetchFixture(t, "plain"); err != nil {
		t.Fatalf("plain: %v", err)
	}
	for _, name := range []string{"symlink", "hardlink", "dotdot", "absolute"} {
		if err := fetchFixture(t, name); err == nil || !strings.Contains(err.Error(), "not a plain file or folder") {
			t.Errorf("%s: %v", name, err)
		}
	}
}

func TestProgressIsMonotonicCappedAndThrottled(t *testing.T) {
	p := newProgress(1000)
	start := time.Now()
	if !p.due(start) {
		t.Fatal("first message not due")
	}
	p.add(500)
	if f, _, _ := p.snapshot(); f != 0.5 {
		t.Fatalf("%v", f)
	}
	if p.due(start.Add(100 * time.Millisecond)) {
		t.Fatal("due after 100 ms")
	}
	if !p.due(start.Add(300 * time.Millisecond)) {
		t.Fatal("not due after 300 ms")
	}
	p.add(700)
	if f, _, total := p.snapshot(); f != 0.99 || total != 1200 {
		t.Fatalf("below 1 until done: %v %d", f, total)
	}
	u := newProgress(0)
	u.add(10)
	if f, _, total := u.snapshot(); f != 0 || total != 10 {
		t.Fatalf("%v %d", f, total)
	}
}

func TestURLsJoinBaseAndArchive(t *testing.T) {
	for got, want := range map[string]string{
		archiveURL("http://h/x/", "a-b"):                    "http://h/x/a-b.tar.bz2",
		hfFileURL("http://h/", "o/r", "c0ffee", "m.gguf"):   "http://h/o/r/resolve/c0ffee/m.gguf",
		fileURL("http://h/x/", "silero_vad.onnx"):           "http://h/x/silero_vad.onnx",
		siblingReleaseURL(defaultBaseURL, embeddingRelease): "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models",
		siblingReleaseURL("http://h/redirect/", "r"):        "http://h/r",
	} {
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	}
	if !strings.HasPrefix(archiveURL(defaultBaseURL, "a"), "https://github.com/") {
		t.Error("default base")
	}
}

func TestAnExtractedFolderMovesWhileAFileInItIsStillOpenForAMoment(t *testing.T) {
	dir := t.TempDir()
	staging, dest := filepath.Join(dir, "staging"), filepath.Join(dir, "dest")
	if err := os.MkdirAll(filepath.Join(staging, "model"), 0o755); err != nil {
		t.Fatal(err)
	}
	held, err := os.Create(filepath.Join(staging, "model", "encoder.onnx"))
	if err != nil {
		t.Fatal(err)
	}
	go func() {
		time.Sleep(300 * time.Millisecond)
		held.Close()
	}()
	if err := moveIntoPlace(staging, dest); err != nil {
		t.Fatal(err)
	}
	if !isFile(filepath.Join(dest, "encoder.onnx")) {
		t.Fatal("the file is not in place")
	}
}

func TestAnArtifactWithAFileThatItsDescriptorLacksIsRefusedByName(t *testing.T) {
	folder := t.TempDir()
	for _, name := range []string{"README.md", "tokens.txt"} {
		if err := os.WriteFile(filepath.Join(folder, name), []byte(name), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	sum, err := hashFile(filepath.Join(folder, "tokens.txt"))
	if err != nil {
		t.Fatal(err)
	}
	a := &artifact{Digest: "d", Files: []storeFile{
		{Path: "tokens.txt", Size: uint64(len("tokens.txt")), SHA256: sum},
		{Path: "test_wavs/en.wav", Size: 1, SHA256: sum},
	}}
	err = verifyArtifact(folder, a)
	if err == nil {
		t.Fatal("accepted")
	}
	want := `not in the descriptor ["README.md"], missing ["test_wavs/en.wav"]`
	if !strings.Contains(err.Error(), want) {
		t.Fatalf("%v, want it to contain %s", err, want)
	}
}
