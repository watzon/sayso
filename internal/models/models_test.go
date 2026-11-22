package models

import (
	"archive/tar"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestEmbeddedDescriptorsAreRenderedFormAndDigestsMatch(t *testing.T) {
	d := AllDescriptors()
	if Render(d) != string(descriptorsJSON) {
		t.Fatal("descriptors.json is not in the rendered form")
	}
	for name, a := range d.Artifacts {
		got, err := Digest(a.Files)
		if err != nil || got != a.Digest {
			t.Fatalf("%s: digest %s (%v), want %s", name, got, err, a.Digest)
		}
	}
	for _, x := range d.Descriptors {
		if _, err := d.Parts(x); err != nil {
			t.Fatal(err)
		}
	}
}

func TestEveryStoredCatalogModelHasADescriptor(t *testing.T) {
	for _, s := range []System{MacOS, Linux, Windows} {
		for _, m := range LocalModelsFor(s) {
			if FormatOf(m.Engine) == "" {
				continue
			}
			if _, ok := AllDescriptors().Find(string(m.ID), FormatOf(m.Engine)); !ok {
				t.Errorf("system %d: no descriptor for %s in %s", s, m.ID, FormatOf(m.Engine))
			}
		}
	}
}

func TestCheckPathAndDigestRefuseBadInput(t *testing.T) {
	for _, bad := range []string{"", "/a", "a//b", "a/./b", "../a", `a\b`, "a:b", "a\x01"} {
		if CheckPath(bad) == nil {
			t.Errorf("%q accepted", bad)
		}
	}
	h := strings.Repeat("a", 64)
	if _, err := Digest([]File{{"A", 1, h}, {"a", 1, h}}); err == nil {
		t.Fatal("case duplicates accepted")
	}
	if _, err := Digest([]File{{"a", 1, "XYZ"}}); err == nil {
		t.Fatal("bad hash accepted")
	}
}

func TestCatalogDefaultsAndRoles(t *testing.T) {
	if DefaultModelFor(MacOS) != "parakeet-unified-en" || DefaultModelFor(Windows) != "parakeet-tdt-v3" || DefaultModelFor(Linux) != "parakeet-tdt-v2" {
		t.Fatal("defaults")
	}
	for _, s := range []System{MacOS, Linux, Windows} {
		found := false
		for _, m := range CatalogFor(s) {
			if m.ID == DefaultModelFor(s) {
				found = m.Recommended
			}
		}
		if !found {
			t.Errorf("default of %d is not recommended", s)
		}
	}
	if !IsVadModel(BasicVAD) || !IsVadModel("silero-vad") || IsVadModel("whisper-base") {
		t.Fatal("IsVadModel")
	}
	if !IsDiarizationModel("speaker-diarization") {
		t.Fatal("IsDiarizationModel")
	}
	none := func(ModelID) bool { return false }
	all := func(ModelID) bool { return true }
	if v := VadFor(nil, Auto, none); !v.Basic {
		t.Fatal("auto without models must be basic")
	}
	if v := VadFor(nil, Auto, all); v.Model != "silero-vad" {
		t.Fatal(v)
	}
	asked := ModelID("basic")
	if v := VadFor(&asked, ParseRoleChoice("silero-vad"), all); !v.Basic {
		t.Fatal("asked wins")
	}
	if _, ok := DiarizationFor(nil, Auto, none); ok {
		t.Fatal("no diarization on disk")
	}
	active, _ := Find(DefaultModel())
	if id, ok := PreviewFor(active, Auto, "en", none); active.LivePreview && (!ok || id != active.ID) {
		t.Fatal("a streaming active model previews itself")
	}
	if ParseRoleChoice("auto") != Auto || ParseRoleChoice("x").String() != "x" {
		t.Fatal("role choice")
	}
}

func TestMacCatalogUsesMeasuredSpeed(t *testing.T) {
	for _, m := range CatalogFor(MacOS) {
		if m.ID == "whisper-large-v3" && (m.Speed != 1 || m.Rtfx == nil || *m.Rtfx != 4) {
			t.Fatal(m.Speed, m.Rtfx)
		}
		if m.ID == "parakeet-eou-120m" && m.Punctuation {
			t.Fatal("EOU has no punctuation")
		}
	}
	for _, m := range CatalogFor(Linux) {
		if m.Rtfx != nil || m.MinMacOS != 0 {
			t.Fatal("onnx models have no rtfx")
		}
	}
}

// fakeArtifact builds an artifact over the given files.
func fakeArtifact(t *testing.T, files map[string]string) (Artifact, map[string][]byte) {
	t.Helper()
	var list []File
	contents := map[string][]byte{}
	for p, c := range files {
		h := sha256.Sum256([]byte(c))
		list = append(list, File{Path: p, Size: uint64(len(c)), SHA256: hex.EncodeToString(h[:])})
		contents[p] = []byte(c)
	}
	dg, err := Digest(list)
	if err != nil {
		t.Fatal(err)
	}
	return Artifact{Source: Source{Kind: SourceHuggingFace, Repo: "r", Revision: "x"}, Digest: dg, Files: list}, contents
}

func writeFiles(dir string, contents map[string][]byte) error {
	for p, c := range contents {
		full := filepath.Join(dir, filepath.FromSlash(p))
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			return err
		}
		if err := os.WriteFile(full, c, 0o644); err != nil {
			return err
		}
	}
	return nil
}

func testData(t *testing.T) (*Descriptors, Descriptor, map[string][]byte) {
	a, contents := fakeArtifact(t, map[string]string{"model.bin": "weights", "sub/vocab.json": "{}"})
	d := Descriptor{Model: "m", Format: "f-1", Revision: 2, Parts: []Part{{Role: "main", Artifact: "a"}}}
	return &Descriptors{Artifacts: map[string]Artifact{"a": a}, Descriptors: []Descriptor{d}}, d, contents
}

func TestStoreDownloadLoadDeleteCleanup(t *testing.T) {
	root := t.TempDir()
	s, err := openStoreWithHook(root, "")
	if err != nil {
		t.Fatal(err)
	}
	data, d, contents := testData(t)
	if ok, _ := s.OnDisk(d); ok {
		t.Fatal("on disk before download")
	}
	calls := 0
	fetch := func(a Artifact, work, files string) error { calls++; return writeFiles(files, contents) }
	if err := s.Download(data, d, fetch, nil); err != nil {
		t.Fatal(err)
	}
	if ok, _ := s.OnDisk(d); !ok {
		t.Fatal("not on disk after download")
	}
	if err := s.Download(data, d, fetch, nil); err != nil || calls != 1 {
		t.Fatal("second download fetched again", calls, err)
	}
	lease, err := s.Load(d)
	if err != nil || lease == nil {
		t.Fatal(lease, err)
	}
	folder, _ := lease.Folder("main")
	if b, _ := os.ReadFile(filepath.Join(folder, "sub", "vocab.json")); string(b) != "{}" {
		t.Fatal("artifact content")
	}
	// A loaded artifact survives delete until the lease goes.
	if err := s.Delete("m", "f-1"); err != nil {
		t.Fatal(err)
	}
	if !isDir(folder) {
		t.Fatal("cleanup removed an artifact in use")
	}
	lease.Close()
	if err := s.Cleanup(); err != nil {
		t.Fatal(err)
	}
	if isDir(folder) {
		t.Fatal("cleanup kept an unnamed artifact")
	}
}

func TestStoreRefusesWrongFilesAndKeepsNoStage(t *testing.T) {
	root := t.TempDir()
	s, _ := openStoreWithHook(root, "")
	data, d, contents := testData(t)
	contents["model.bin"] = []byte("tampered")
	err := s.Download(data, d, func(a Artifact, work, files string) error { return writeFiles(files, contents) }, nil)
	if !errors.Is(err, ErrVerify) {
		t.Fatal("want verify error, got", err)
	}
	entries, _ := os.ReadDir(filepath.Join(root, "staging"))
	if len(entries) != 0 {
		t.Fatal("stage left behind")
	}
	ferr := s.Download(data, d, func(Artifact, string, string) error { return errors.New("net down") }, nil)
	var fe *FetchError
	if !errors.As(ferr, &fe) {
		t.Fatal("want fetch error", ferr)
	}
}

func TestManifestNeverGoesBackAndBadManifestBlocksCleanup(t *testing.T) {
	root := t.TempDir()
	s, _ := openStoreWithHook(root, "")
	data, d, contents := testData(t)
	if err := s.Download(data, d, func(a Artifact, w, f string) error { return writeFiles(f, contents) }, nil); err != nil {
		t.Fatal(err)
	}
	lower := d
	lower.Revision = 1
	if err := s.writeManifest(lower, data); err != nil {
		t.Fatal(err)
	}
	m, _ := s.readManifest("m", "f-1")
	if m.Revision != 2 {
		t.Fatal("revision went back")
	}
	// An unparsable manifest makes garbage unknown: nothing is removed.
	os.MkdirAll(filepath.Join(root, "models", "x"), 0o755)
	os.WriteFile(filepath.Join(root, "models", "x", "f.json"), []byte("{"), 0o644)
	os.MkdirAll(filepath.Join(root, "artifacts", "orphan"), 0o755)
	if err := s.Cleanup(); err != nil {
		t.Fatal(err)
	}
	if !isDir(filepath.Join(root, "artifacts", "orphan")) {
		t.Fatal("orphan removed while a manifest is unreadable")
	}
}

func TestDownloaderFetchesHuggingFaceFilesAndArchives(t *testing.T) {
	var tarball bytes.Buffer
	{
		tw := tar.NewWriter(&tarball)
		for _, f := range []struct{ name, body string }{{"pkg/", ""}, {"pkg/a.txt", "alpha"}, {"pkg/d/b.txt", "beta"}} {
			h := &tar.Header{Name: f.name, Mode: 0o644, Size: int64(len(f.body))}
			if strings.HasSuffix(f.name, "/") {
				h.Typeflag, h.Mode = tar.TypeDir, 0o755
			}
			tw.WriteHeader(h)
			tw.Write([]byte(f.body))
		}
		tw.Close()
	}
	bz := bzip2Compress(t, tarball.Bytes())
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, ".tar.bz2"):
			w.Write(bz)
		case strings.HasSuffix(r.URL.Path, "/model.bin"):
			w.Write([]byte("weights"))
		case strings.HasSuffix(r.URL.Path, "/sub/vocab.json"):
			w.Write([]byte("{}"))
		default:
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()
	var got uint64
	dl := NewDownloader()
	dl.URLOf = func(u string) string {
		return srv.URL + "/" + u[strings.Index(u, "://")+3:]
	}
	dl.OnBytes = func(n uint64) { got += n }
	data, d, _ := testData(t)
	s, _ := openStoreWithHook(t.TempDir(), "")
	if err := s.Download(data, d, dl.Fetch, nil); err != nil {
		t.Fatal(err)
	}
	if got != uint64(len("weights")+2) {
		t.Fatal("progress bytes", got)
	}
	archive, _ := fakeArtifact(t, map[string]string{"a.txt": "alpha", "d/b.txt": "beta"})
	archive.Source = Source{Kind: SourceArchive, URL: "https://example.com/x.tar.bz2"}
	ad := Descriptor{Model: "arc", Format: FormatSherpaOnnx, Revision: 1, Parts: []Part{{"main", "arc"}}}
	adata := &Descriptors{Artifacts: map[string]Artifact{"arc": archive}, Descriptors: []Descriptor{ad}}
	if err := s.Download(adata, ad, dl.Fetch, nil); err != nil {
		t.Fatal(err)
	}
}

func bzip2Compress(t *testing.T, b []byte) []byte {
	t.Helper()
	cmd := exec.Command("bzip2", "-c")
	cmd.Stdin = bytes.NewReader(b)
	out, err := cmd.Output()
	if err != nil {
		t.Skip("bzip2 not available:", err)
	}
	return out
}

func TestExtractRefusesLinksAndEscapes(t *testing.T) {
	for _, h := range []*tar.Header{
		{Name: "../evil", Size: 0, Typeflag: tar.TypeReg},
		{Name: "link", Linkname: "/etc/passwd", Typeflag: tar.TypeSymlink},
	} {
		var buf bytes.Buffer
		tw := tar.NewWriter(&buf)
		tw.WriteHeader(h)
		tw.Close()
		bz := bzip2Compress(t, buf.Bytes())
		if err := ExtractTarBz2(bytes.NewReader(bz), t.TempDir()); err == nil {
			t.Fatalf("%s accepted", h.Name)
		}
	}
}

func TestProgressIsMonotonicCappedAndThrottled(t *testing.T) {
	p := NewProgress(1000)
	start := time.Now()
	if !p.Due(start) {
		t.Fatal("first is due")
	}
	p.Add(500)
	if p.Fraction() != 0.5 || p.Due(start.Add(100*time.Millisecond)) || !p.Due(start.Add(300*time.Millisecond)) {
		t.Fatal("throttle")
	}
	p.Add(700)
	if p.Fraction() != 0.99 || p.BytesTotal() != 1200 {
		t.Fatal(p.Fraction(), p.BytesTotal())
	}
}

func TestMigrationMovesMatchingModelsAndKeepsOthers(t *testing.T) {
	modelsDir := t.TempDir()
	data, d, contents := testData(t)
	badArtifact, _ := fakeArtifact(t, map[string]string{"model.bin": "real weights"})
	data.Artifacts["b"] = badArtifact
	bad := Descriptor{Model: "bad", Format: "f-1", Revision: 1, Parts: []Part{{Role: "main", Artifact: "b"}}}
	data.Descriptors = append(data.Descriptors, bad)
	writeFiles(filepath.Join(modelsDir, "m", "old"), contents)
	writeFiles(filepath.Join(modelsDir, "bad", "old"), map[string][]byte{"model.bin": []byte("nope")})
	old := []OldModel{
		{Model: "m", Format: "f-1", Parts: []OldPart{{"main", []string{"m/old"}}}, Remove: []string{"m"}},
		{Model: "bad", Format: "f-1", Parts: []OldPart{{"main", []string{"bad/old"}}}, Remove: []string{"bad"}},
	}
	store, _ := openStoreWithHook(filepath.Join(modelsDir, StoreFolder), "")
	report, err := RunMigrationWith(store, modelsDir, data, old, LinksCopyOnly)
	if err != nil {
		t.Fatal(err)
	}
	if len(report.Moved) != 1 || report.Moved[0] != "m" || len(report.Kept) != 1 || report.Kept[0][0] != "bad" {
		t.Fatalf("%+v", report)
	}
	if ok, _ := store.OnDisk(d); !ok {
		t.Fatal("moved model not on disk")
	}
	if isDir(filepath.Join(modelsDir, "m")) {
		t.Fatal("old folder of a moved model stays")
	}
	if !isDir(filepath.Join(modelsDir, "bad")) {
		t.Fatal("old folder of a kept model removed")
	}
	if !StoreInUse(modelsDir) {
		t.Fatal("finished journal must mean the store is in use")
	}
	// A second run is a no-op.
	again, err := RunMigrationWith(store, modelsDir, data, old, LinksCopyOnly)
	if err != nil || len(again.Moved) != 0 {
		t.Fatalf("%+v %v", again, err)
	}
}

func TestOldModelsAndTokenizers(t *testing.T) {
	if WhisperTokenizer("distil-whisper_distil-large-v3_594MB") != "openai/whisper-large-v3" {
		t.Fatal("tokenizer")
	}
	if WhisperTokenizer("openai_whisper-small.en") != "openai/whisper-small.en" {
		t.Fatal("english tokenizer")
	}
	for _, s := range []System{MacOS, Linux, Windows} {
		if len(OldModels(s)) == 0 {
			t.Fatal("no old models for", s)
		}
	}
	for _, m := range OldModels(Windows) {
		if m.Model == "whisper-base" && (len(m.Parts[0].Folders) != 2 || m.Parts[0].Folders[0] != "whisper-base/main") {
			t.Fatalf("%+v", m)
		}
	}
}
