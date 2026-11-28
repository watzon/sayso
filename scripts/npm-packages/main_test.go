package main

import (
	"archive/tar"
	"archive/zip"
	"compress/gzip"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

const root = "../.."

func version(t *testing.T) string {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(root, "internal/update/version.go"))
	if err != nil {
		t.Fatal(err)
	}
	return regexp.MustCompile(`(?m)^var Version = "([^"]+)"`).FindStringSubmatch(string(data))[1]
}

type entry struct {
	name, body, link string
}

func writeTar(t *testing.T, file string, entries []entry) {
	t.Helper()
	f, err := os.Create(file)
	if err != nil {
		t.Fatal(err)
	}
	gz := gzip.NewWriter(f)
	tw := tar.NewWriter(gz)
	for _, e := range entries {
		switch {
		case strings.HasSuffix(e.name, "/"):
			tw.WriteHeader(&tar.Header{Name: e.name, Typeflag: tar.TypeDir, Mode: 0o755})
		case e.link != "":
			tw.WriteHeader(&tar.Header{Name: e.name, Typeflag: tar.TypeSymlink, Linkname: e.link, Mode: 0o755})
		default:
			tw.WriteHeader(&tar.Header{Name: e.name, Typeflag: tar.TypeReg, Mode: 0o755, Size: int64(len(e.body))})
			tw.Write([]byte(e.body))
		}
	}
	tw.Close()
	gz.Close()
	f.Close()
	sum(t, file)
}

func writeZip(t *testing.T, file string, entries []entry) {
	t.Helper()
	f, err := os.Create(file)
	if err != nil {
		t.Fatal(err)
	}
	zw := zip.NewWriter(f)
	for _, e := range entries {
		w, _ := zw.Create(e.name)
		w.Write([]byte(e.body))
	}
	zw.Close()
	f.Close()
	sum(t, file)
}

func sum(t *testing.T, file string) {
	data, _ := os.ReadFile(file)
	h := sha256.Sum256(data)
	os.WriteFile(file+".sha256", []byte(hex.EncodeToString(h[:])+"  "+filepath.Base(file)+"\n"), 0o644)
}

// release writes a release folder with every file of every package.
func release(t *testing.T) string {
	t.Helper()
	v := version(t)
	dir := t.TempDir()
	for _, k := range kinds {
		for _, p := range k.Platforms {
			top := fmt.Sprintf("sayso-%s-%s", k.Name, v)
			file := filepath.Join(dir, fmt.Sprintf("%s-%s.%s", top, p.Release, p.ext()))
			entries := []entry{
				{name: top + "/"},
				{name: top + "/README.md", body: "release readme"},
				{name: top + "/LICENSE", body: "gpl"},
				{name: top + "/" + k.Program(p), body: "program"},
			}
			if p.ext() == "zip" {
				writeZip(t, file, entries)
				continue
			}
			entries = append(entries,
				entry{name: top + "/lib/libx.1.0.dylib", body: "library"},
				entry{name: top + "/lib/libx.1.dylib", link: "libx.1.0.dylib"},
				entry{name: top + "/lib/libx.dylib", link: "libx.1.dylib"},
			)
			writeTar(t, file, entries)
		}
	}
	return dir
}

func TestMakeWritesEveryPackage(t *testing.T) {
	out := filepath.Join(t.TempDir(), "npm")
	made, err := Make(root, release(t), out, false)
	if err != nil {
		t.Fatal(err)
	}
	if len(made) != 11 {
		t.Fatalf("made %d packages, want 11: %v", len(made), made)
	}
	v := version(t)
	for _, dir := range made {
		var pkg struct{ Name, Version, License string }
		data, err := os.ReadFile(filepath.Join(dir, "package.json"))
		if err != nil {
			t.Fatal(err)
		}
		json.Unmarshal(data, &pkg)
		if pkg.Name != "@justsayso/"+filepath.Base(dir) || pkg.Version != v || pkg.License != "GPL-3.0-only" {
			t.Errorf("%s: package.json %+v", dir, pkg)
		}
		if _, err := os.Stat(filepath.Join(dir, "LICENSE")); err != nil {
			t.Errorf("%s has no LICENSE", dir)
		}
	}
	mac := filepath.Join(out, "cleanup-darwin-arm64")
	if b, _ := os.ReadFile(filepath.Join(mac, "bin/sayso-cleanup")); string(b) != "program" {
		t.Errorf("bin/sayso-cleanup = %q", b)
	}
	if b, _ := os.ReadFile(filepath.Join(mac, "CLEANUP.md")); string(b) != "release readme" {
		t.Errorf("CLEANUP.md = %q", b)
	}
	if b, _ := os.ReadFile(filepath.Join(mac, "LICENSE")); string(b) == "gpl" {
		t.Error("the LICENSE of the package must be the LICENSE of the repository")
	}
	for _, link := range []string{"libx.1.dylib", "libx.dylib"} {
		fi, err := os.Lstat(filepath.Join(mac, "lib", link))
		if err != nil || !fi.Mode().IsRegular() {
			t.Fatalf("%s must be a file, not a link: %v %v", link, fi, err)
		}
		if b, _ := os.ReadFile(filepath.Join(mac, "lib", link)); string(b) != "library" {
			t.Errorf("%s = %q", link, b)
		}
	}
	if _, err := os.Stat(filepath.Join(out, "cleanup-darwin-x64")); err == nil {
		t.Error("there is no cleanup package for Intel Macs")
	}
	if b, _ := os.ReadFile(filepath.Join(out, "engine-win32-x64-msvc/sayso-engine.exe")); string(b) != "program" {
		t.Errorf("sayso-engine.exe = %q", b)
	}
}

func TestMakeRefusesABadFile(t *testing.T) {
	v := version(t)
	cases := map[string]func(t *testing.T, dir string){
		"checksum": func(t *testing.T, dir string) {
			os.WriteFile(filepath.Join(dir, fmt.Sprintf("sayso-engine-%s-linux-x86_64.tar.gz.sha256", v)), []byte(strings.Repeat("0", 64)+"\n"), 0o644)
		},
		"no checksum": func(t *testing.T, dir string) {
			os.Remove(filepath.Join(dir, fmt.Sprintf("sayso-engine-%s-linux-x86_64.tar.gz.sha256", v)))
		},
		"missing": func(t *testing.T, dir string) {
			os.Remove(filepath.Join(dir, fmt.Sprintf("sayso-cleanup-%s-windows-x64.zip", v)))
		},
		"leaves the folder": func(t *testing.T, dir string) {
			top := "sayso-engine-" + v
			writeTar(t, filepath.Join(dir, top+"-linux-arm64.tar.gz"), []entry{{name: top + "/sayso-engine", body: "p"}, {name: top + "/../evil", body: "x"}})
		},
		"link leaves the folder": func(t *testing.T, dir string) {
			top := "sayso-engine-" + v
			writeTar(t, filepath.Join(dir, top+"-linux-arm64.tar.gz"), []entry{{name: top + "/sayso-engine", body: "p"}, {name: top + "/x", link: "../../etc/passwd"}})
		},
		"no program": func(t *testing.T, dir string) {
			top := "sayso-cleanup-" + v
			writeTar(t, filepath.Join(dir, top+"-macos-arm64.tar.gz"), []entry{{name: top + "/sayso-cleanup", body: "p"}})
		},
	}
	for name, spoil := range cases {
		t.Run(name, func(t *testing.T) {
			dir := release(t)
			spoil(t, dir)
			if _, err := Make(root, dir, filepath.Join(t.TempDir(), "npm"), false); err == nil {
				t.Fatal("Make accepted the release folder")
			} else {
				t.Log(err)
			}
		})
	}
}

func TestMakeSkipsAMissingFileWhenAllowed(t *testing.T) {
	dir := release(t)
	os.Remove(filepath.Join(dir, fmt.Sprintf("sayso-cleanup-%s-windows-x64.zip", version(t))))
	out := filepath.Join(t.TempDir(), "npm")
	made, err := Make(root, dir, out, true)
	if err != nil {
		t.Fatal(err)
	}
	if len(made) != 10 {
		t.Fatalf("made %d packages, want 10", len(made))
	}
}
