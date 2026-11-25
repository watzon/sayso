package winres

import (
	"bytes"
	"image/png"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// The committed .syso files are the output of generate for the committed
// icon, so a new AppIcon.ico cannot ship with the old resources.
func TestCommittedResourcesMatchTheIcon(t *testing.T) {
	root := filepath.Join("..", "..")
	files, err := Generate(filepath.Join(root, "assets", "app", "AppIcon.ico"))
	if err != nil {
		t.Fatal(err)
	}
	for name, want := range files {
		got, err := os.ReadFile(filepath.Join(root, "cmd", "sayso", name))
		if err != nil {
			t.Fatalf("%v: run go run ./scripts/winres generate", err)
		}
		if !bytes.Equal(got, want) {
			t.Errorf("cmd/sayso/%s is not the output for assets/app/AppIcon.ico: run go run ./scripts/winres generate", name)
		}
	}
}

// A Windows build of cmd/sayso links the resources, and check finds every
// size of the icon group that MyGo loads.
func TestTheWindowsExeHasTheIcon(t *testing.T) {
	if testing.Short() {
		t.Skip("builds cmd/sayso")
	}
	exe := filepath.Join(t.TempDir(), "sayso.exe")
	cmd := exec.Command("go", "build", "-tags", "mygo_noinspector", "-ldflags=-s -w -H=windowsgui", "-o", exe, "./cmd/sayso")
	cmd.Dir = filepath.Join("..", "..")
	cmd.Env = append(os.Environ(), "GOOS=windows", "GOARCH=amd64", "CGO_ENABLED=0")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	sizes, err := Check(exe)
	if err != nil {
		t.Fatal(err)
	}
	if len(sizes) != len(IconSizes) {
		t.Errorf("sizes %v, want %v", sizes, IconSizes)
	}
}

func TestCheckRejectsAnExeWithoutTheIcon(t *testing.T) {
	if testing.Short() {
		t.Skip("builds a program")
	}
	dir := t.TempDir()
	src := filepath.Join(dir, "main.go")
	if err := os.WriteFile(src, []byte("package winres\nfunc main() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module x\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	exe := filepath.Join(dir, "x.exe")
	cmd := exec.Command("go", "build", "-o", exe, ".")
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GOOS=windows", "GOARCH=amd64", "CGO_ENABLED=0", "GOFLAGS=")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("build: %v\n%s", err, out)
	}
	if _, err := Check(exe); err == nil {
		t.Error("check passed an exe with no icon")
	}
}

func TestGenerateFillsTheMissingSizes(t *testing.T) {
	images, err := iconImages(mustRead(t, filepath.Join("..", "..", "assets", "app", "AppIcon.ico")))
	if err != nil {
		t.Fatal(err)
	}
	if len(images) != len(IconSizes) {
		t.Fatalf("%d images", len(images))
	}
	for i, img := range images {
		cfg, err := png.DecodeConfig(bytes.NewReader(img))
		if err != nil || cfg.Width != IconSizes[i] || cfg.Height != IconSizes[i] {
			t.Errorf("image %d: %dx%d (%v), want %d px", i, cfg.Width, cfg.Height, err, IconSizes[i])
		}
	}
}

func mustRead(t *testing.T, path string) []byte {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return b
}
