//go:build darwin

package update

import (
	"os"
	"path/filepath"
	"testing"
)

func bundle(t *testing.T, dir, marker string) string {
	app := filepath.Join(dir, "Sayso.app")
	os.MkdirAll(filepath.Join(app, "Contents/MacOS"), 0o755)
	os.WriteFile(filepath.Join(app, "Contents/MacOS/Sayso"), []byte(marker), 0o644)
	return app
}

func TestMacInstallExchange(t *testing.T) {
	root, _ := filepath.EvalSymlinks(t.TempDir())
	app := bundle(t, root, "old")
	m, manual := macForExecutable(filepath.Join(app, "Contents/MacOS/Sayso"))
	if manual != ManualNone || m.InstallPath() != app {
		t.Fatal(manual)
	}
	if _, err := os.Stat(m.stagingDir()); err == nil {
		t.Fatal("staging dir left after detect")
	}
	m.makeStagingDir()
	staged := m.stagedApp()
	os.MkdirAll(filepath.Join(staged, "Contents/MacOS"), 0o755)
	os.WriteFile(filepath.Join(staged, "Contents/MacOS/Sayso"), []byte("new"), 0o644)
	if err := m.Exchange(); err != nil {
		t.Fatal(err)
	}
	if b, _ := os.ReadFile(filepath.Join(app, "Contents/MacOS/Sayso")); string(b) != "new" {
		t.Fatal(string(b))
	}
	m.Exchange()
	if b, _ := os.ReadFile(filepath.Join(app, "Contents/MacOS/Sayso")); string(b) != "old" {
		t.Fatal("back")
	}
	m.RemoveStaging()
	if _, err := os.Stat(m.stagingDir()); err == nil {
		t.Fatal("staging left")
	}
	if _, manual := macForExecutable(filepath.Join(root, "target/debug/sayso")); manual != ManualWrongFolder {
		t.Fatal("not a bundle")
	}
	if _, manual := macForExecutable("/private/var/folders/x/AppTranslocation/ABC/d/Sayso.app/Contents/MacOS/Sayso"); manual != ManualWrongFolder {
		t.Fatal("translocated")
	}
}
