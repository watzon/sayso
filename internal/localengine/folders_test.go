package localengine

import (
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"testing"
)

func releaseSpec() *modelSpec {
	return &modelSpec{recipe: recipeParakeetTdt, archive: "main", source: source{kind: sourceSherpaRelease}, previewArchive: "preview"}
}

func whisperSpec() *modelSpec {
	return &modelSpec{recipe: recipeWhisperCpp, archive: "main", source: source{kind: sourceHuggingFace, repo: "ggerganov/whisper.cpp", file: "ggml-tiny.bin"}}
}

// legacyFolder is a model folder as the Windows engine before 0.12 left it.
func legacyFolder(t *testing.T, m *modelFolders, model string, files []string, marker bool) string {
	t.Helper()
	folder, err := m.folder(model)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(folder, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, f := range files {
		if err := os.WriteFile(filepath.Join(folder, f), []byte("contents of "+f), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if marker {
		if err := os.WriteFile(filepath.Join(folder, legacyMarker), nil, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return folder
}

func mustLegacy(t *testing.T, m *modelFolders, model string, spec *modelSpec, want legacyState) {
	t.Helper()
	got, err := m.adoptLegacy(model, spec)
	if err != nil || got != want {
		t.Fatalf("%s: %v %v, want %v", model, got, err, want)
	}
}

func TestAnOldWhisperDownloadMovesIntoPlace(t *testing.T) {
	m := &modelFolders{root: t.TempDir()}
	folder := legacyFolder(t, m, "whisper-tiny", []string{"ggml-tiny.bin"}, true)
	if m.isDownloaded("whisper-tiny", whisperSpec()) {
		t.Fatal("downloaded before the move")
	}
	mustLegacy(t, m, "whisper-tiny", whisperSpec(), legacyAdopted)
	if !m.isDownloaded("whisper-tiny", whisperSpec()) {
		t.Fatal("not downloaded after the move")
	}
	if b, err := os.ReadFile(filepath.Join(folder, "main", "ggml-tiny.bin")); err != nil || string(b) != "contents of ggml-tiny.bin" {
		t.Fatalf("%q %v", b, err)
	}
	if _, err := os.Stat(filepath.Join(folder, legacyMarker)); !os.IsNotExist(err) {
		t.Fatal("the marker stayed")
	}
	mustLegacy(t, m, "whisper-tiny", whisperSpec(), legacyNone)
}

func TestOldFilesThatCannotMoveAreAFormatChange(t *testing.T) {
	m := &modelFolders{root: t.TempDir()}
	legacyFolder(t, m, "parakeet-tdt-v3", []string{"encoder-model.int8.onnx", "vocab.txt"}, true)
	mustLegacy(t, m, "parakeet-tdt-v3", releaseSpec(), legacyFormatChanged)
	legacyFolder(t, m, "whisper-tiny", []string{"ggml-tiny.bin.partial"}, false)
	mustLegacy(t, m, "whisper-tiny", whisperSpec(), legacyNone)
	legacyFolder(t, m, "whisper-base", nil, true)
	mustLegacy(t, m, "whisper-base", whisperSpec(), legacyFormatChanged)
	mustLegacy(t, m, "never-downloaded", whisperSpec(), legacyNone)
}

func TestADownloadRemovesOldFilesAndKeepsCompleteArchives(t *testing.T) {
	m := &modelFolders{root: t.TempDir()}
	folder := legacyFolder(t, m, "m", []string{"encoder-model.int8.onnx", "x.partial"}, true)
	for _, d := range []string{"main", "main-old"} {
		if err := os.MkdirAll(filepath.Join(folder, d), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := m.prepareDownload("m", releaseSpec()); err != nil {
		t.Fatal(err)
	}
	entries, _ := os.ReadDir(folder)
	var left []string
	for _, e := range entries {
		left = append(left, e.Name())
	}
	sort.Strings(left)
	if !reflect.DeepEqual(left, []string{"main"}) {
		t.Fatalf("%q", left)
	}
	mustLegacy(t, m, "m", releaseSpec(), legacyNone)
}

func mkdir(t *testing.T, p string, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(p, 0o755); err != nil {
		t.Fatal(err)
	}
}

func TestDownloadedNeedsTheMarkerAndEveryArchive(t *testing.T) {
	m := &modelFolders{root: t.TempDir()}
	spec := releaseSpec()
	if m.isDownloaded("m", spec) {
		t.Fatal("empty store")
	}
	if err := m.prepareDownload("m", spec); err != nil {
		t.Fatal(err)
	}
	p, err := m.archiveDir("m", "main")
	mkdir(t, p, err)
	if m.isDownloaded("m", spec) {
		t.Fatal("no marker yet")
	}
	p, err = m.archiveDir("m", "preview")
	mkdir(t, p, err)
	if err := m.markDownloaded("m", spec); err != nil {
		t.Fatal(err)
	}
	if !m.isDownloaded("m", spec) {
		t.Fatal("marked but not downloaded")
	}
	other := *spec
	other.archive = "main-v2"
	if m.isDownloaded("m", &other) {
		t.Fatal("another archive in the catalog means a new download")
	}
	if err := m.delete("m"); err != nil || m.isDownloaded("m", spec) {
		t.Fatalf("delete: %v", err)
	}
	if err := m.delete("m"); err != nil {
		t.Fatalf("deleting twice: %v", err)
	}
}

func TestPrepareRemovesPartialExtractionsAndTheMarker(t *testing.T) {
	m := &modelFolders{root: t.TempDir()}
	staging, err := m.stagingDir("m", "main")
	mkdir(t, filepath.Join(staging, "x"), err)
	p, err := m.archiveDir("m", "main")
	mkdir(t, p, err)
	p2, err := m.archiveDir("m", "preview")
	mkdir(t, p2, err)
	if err := m.markDownloaded("m", releaseSpec()); err != nil {
		t.Fatal(err)
	}
	if err := m.prepareDownload("m", releaseSpec()); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(staging); !os.IsNotExist(err) {
		t.Fatal("staging stayed")
	}
	if m.isDownloaded("m", releaseSpec()) {
		t.Fatal("still marked")
	}
	if !isDir(p) {
		t.Fatal("complete archives stay")
	}
}

func TestRejectsUnsafeModelIDs(t *testing.T) {
	root := filepath.Join(t.TempDir(), "models")
	m := &modelFolders{root: root}
	for _, bad := range []string{"", "..", ".x", "a/b", "openai:whisper-1", `a\b`} {
		if _, err := m.folder(bad); err == nil {
			t.Errorf("%q accepted", bad)
		}
	}
	if p, err := m.folder("kroko-en"); err != nil || p != filepath.Join(root, "kroko-en") {
		t.Fatalf("%q %v", p, err)
	}
}
