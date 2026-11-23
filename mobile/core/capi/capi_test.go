package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sync"
	"testing"
)

var (
	archiveOnce sync.Once
	archiveDir  string
	archiveErr  error
	archiveOut  []byte
)

// buildC builds the core as a host C archive (once per test binary), links
// testdata/<name>.c against it, and returns the program's path
// (scripts/test-capi.sh does the same by hand).
func buildC(t *testing.T, name string) string {
	t.Helper()
	if testing.Short() {
		t.Skip("builds a C archive")
	}
	cc, err := exec.LookPath("cc")
	if err != nil {
		t.Skip("no C compiler")
	}
	archiveOnce.Do(func() {
		archiveDir, archiveErr = os.MkdirTemp("", "sayso-capi")
		if archiveErr != nil {
			return
		}
		build := exec.Command("go", "build", "-buildmode=c-archive", "-o", filepath.Join(archiveDir, "libsayso_core.a"), ".")
		build.Env = append(os.Environ(), "CGO_ENABLED=1")
		archiveOut, archiveErr = build.CombinedOutput()
	})
	if archiveErr != nil {
		t.Fatalf("go build: %v\n%s", archiveErr, archiveOut)
	}
	bin := filepath.Join(t.TempDir(), name)
	args := []string{"-O2", "-Wall", "-Wextra", "-Werror", "-I", ".", "testdata/" + name + ".c",
		filepath.Join(archiveDir, "libsayso_core.a"), "-lpthread", "-o", bin}
	if runtime.GOOS == "darwin" {
		args = append(args, "-framework", "CoreFoundation", "-framework", "Security")
	}
	if out, err := exec.Command(cc, args...).CombinedOutput(); err != nil {
		t.Fatalf("cc: %v\n%s", err, out)
	}
	return bin
}

func TestMain(m *testing.M) {
	code := m.Run()
	if archiveDir != "" {
		os.RemoveAll(archiveDir)
	}
	os.Exit(code)
}

// TestCSmoke runs testdata/smoke.c: every ABI call from C, plus on_touch
// timing through cgo.
func TestCSmoke(t *testing.T) {
	bin := buildC(t, "smoke")
	out, err := exec.Command(bin, "../../models/en-US.sslm", filepath.Join(t.TempDir(), "journal")).CombinedOutput()
	t.Logf("\n%s", out)
	if err != nil {
		t.Fatal(err)
	}
}

// TestCDestroyRace runs testdata/race.c: on_touch from four threads while
// engines are created and destroyed under them. It crashed (SIGSEGV in the
// decoder, reading the unmapped model) when a call could outlive destroy.
func TestCDestroyRace(t *testing.T) {
	bin := buildC(t, "race")
	for run := 0; run < 3; run++ {
		out, err := exec.Command(bin, "../../models/en-US.sslm", "1500").CombinedOutput()
		if err != nil {
			if len(out) > 4000 {
				out = out[:4000]
			}
			t.Fatalf("run %d: %v\n%s", run, err, out)
		}
		t.Logf("run %d: %s", run, out)
	}
}
