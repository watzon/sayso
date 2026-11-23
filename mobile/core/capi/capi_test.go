package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
)

// TestCSmoke builds the core as a host C archive, links testdata/smoke.c
// against it, and runs it: every ABI call from C, plus on_touch timing
// through cgo (scripts/test-capi.sh does the same by hand).
func TestCSmoke(t *testing.T) {
	if testing.Short() {
		t.Skip("builds a C archive")
	}
	cc, err := exec.LookPath("cc")
	if err != nil {
		t.Skip("no C compiler")
	}
	dir := t.TempDir()
	lib := filepath.Join(dir, "libsayso_core.a")
	build := exec.Command("go", "build", "-buildmode=c-archive", "-o", lib, ".")
	build.Env = append(os.Environ(), "CGO_ENABLED=1")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("go build: %v\n%s", err, out)
	}
	bin := filepath.Join(dir, "smoke")
	args := []string{"-O2", "-Wall", "-Wextra", "-Werror", "-I", ".", "testdata/smoke.c", lib, "-lpthread", "-o", bin}
	if runtime.GOOS == "darwin" {
		args = append(args, "-framework", "CoreFoundation", "-framework", "Security")
	}
	if out, err := exec.Command(cc, args...).CombinedOutput(); err != nil {
		t.Fatalf("cc: %v\n%s", err, out)
	}
	out, err := exec.Command(bin, "../../models/en-US.sslm", filepath.Join(dir, "journal")).CombinedOutput()
	t.Logf("\n%s", out)
	if err != nil {
		t.Fatal(err)
	}
}
