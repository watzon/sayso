package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const manifest = `{
  "about": "test",
  "sdks": ["go", "rust"],
  "scenarios": [
    {"id": "H01", "title": "hello"},
    {"id": "H07", "title": "old host", "na": {"rust/windows": "unix socket"}},
    {"id": "AU01", "title": "helper", "na": {"rust": "no helper"}}
  ]
}`

func read(t *testing.T, text string) *Manifest {
	t.Helper()
	m, err := ReadManifest(strings.NewReader(text))
	if err != nil {
		t.Fatal(err)
	}
	return m
}

func ids(list ...string) map[string]bool {
	out := map[string]bool{}
	for _, id := range list {
		out[id] = true
	}
	return out
}

func texts(ps []Problem) []string {
	var out []string
	for _, p := range ps {
		out = append(out, p.String())
	}
	return out
}

func TestEveryScenarioThatAppliesPasses(t *testing.T) {
	m := read(t, manifest)
	reports := map[string]map[string]bool{"go": ids("H01", "H07", "AU01"), "rust": ids("H01", "H07")}
	if p := Check(m, "darwin", reports); len(p) != 0 {
		t.Fatal(texts(p))
	}
	reports["rust"] = ids("H01")
	if p := Check(m, "windows", reports); len(p) != 0 {
		t.Fatal(texts(p))
	}
}

func TestAMissingScenarioFails(t *testing.T) {
	m := read(t, manifest)
	p := Check(m, "linux", map[string]map[string]bool{"go": ids("H01", "AU01"), "rust": ids("H01")})
	got := strings.Join(texts(p), "\n")
	if len(p) != 2 || !strings.Contains(got, "go: H07: missing") || !strings.Contains(got, "rust: H07: missing") {
		t.Fatal(got)
	}
}

func TestAMissingReportFails(t *testing.T) {
	p := Check(read(t, manifest), "linux", map[string]map[string]bool{"go": ids("H01", "H07", "AU01")})
	if len(p) != 1 || p[0].String() != "rust: -: no report" {
		t.Fatal(texts(p))
	}
}

func TestAnIDThatIsNotInTheManifestFails(t *testing.T) {
	p := Check(read(t, manifest), "linux", map[string]map[string]bool{"go": ids("H01", "H07", "AU01", "H99"), "rust": ids("H01", "H07")})
	if len(p) != 1 || p[0].String() != "go: H99: not in scenarios.json" {
		t.Fatal(texts(p))
	}
}

func TestABadManifestIsRefused(t *testing.T) {
	for _, bad := range []string{
		`{"sdks": ["go"], "scenarios": [{"id": "A"}, {"id": "A"}]}`,
		`{"sdks": ["go"], "scenarios": [{"id": "A", "na": {"swift": "x"}}]}`,
		`{"sdks": ["go"], "scenarios": [{"id": "A", "na": {"go": ""}}]}`,
		`{"sdks": ["go"], "scenarios": [{"id": "A", "nope": 1}]}`,
		`{"sdks": [], "scenarios": []}`,
	} {
		if _, err := ReadManifest(strings.NewReader(bad)); err == nil {
			t.Errorf("accepted %s", bad)
		}
	}
}

func TestTheManifestOfTheRepositoryReads(t *testing.T) {
	f, err := os.Open(filepath.Join("..", "..", "sdk", "conformance", "scenarios.json"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	m, err := ReadManifest(f)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Join(m.SDKs, ",") != "go,typescript,rust" || len(m.Scenarios) < 40 {
		t.Fatal(m.SDKs, len(m.Scenarios))
	}
}

func TestRunReadsTheFiles(t *testing.T) {
	dir := t.TempDir()
	write := func(name, text string) string {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
		return p
	}
	mpath := write("m.json", manifest)
	goFile, rustFile := write("go.txt", "H01\nH07\nAU01\nH01\n"), write("rust.txt", "H01\n")
	files := map[string]*string{"go": &goFile, "rust": &rustFile}
	if code := run(mpath, "windows", files); code != 0 {
		t.Fatal(code)
	}
	if code := run(mpath, "linux", files); code != 1 {
		t.Fatal("rust has no H07 on linux", code)
	}
}
