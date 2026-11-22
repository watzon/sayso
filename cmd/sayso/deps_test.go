package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

// The app must never link yzma or its libffi binding: the binding loads libffi
// in a package init and panics when that fails, and in a signed app on macOS
// library validation makes it fail. Only cmd/sayso-cleanup, a separate
// process, may use them (docs/cleanup-models.md, "Runtime").
func TestAppDoesNotLinkLlama(t *testing.T) {
	gobin := filepath.Join(runtime.GOROOT(), "bin", "go")
	if _, err := os.Stat(gobin); err != nil {
		gobin = "go"
	}
	forbidden := []string{"github.com/hybridgroup/yzma", "github.com/jupiterrider/ffi", "github.com/watzon/sayso/internal/cleanupd/llama"}
	for _, goos := range []string{"darwin", "linux", "windows"} {
		cmd := exec.Command(gobin, "list", "-deps", "github.com/watzon/sayso/cmd/sayso")
		cmd.Env = append(os.Environ(), "GOOS="+goos, "GOARCH=amd64", "CGO_ENABLED=0")
		out, err := cmd.Output()
		if err != nil {
			t.Fatalf("go list for %s: %v", goos, err)
		}
		for _, dep := range strings.Fields(string(out)) {
			for _, f := range forbidden {
				if dep == f || strings.HasPrefix(dep, f+"/") {
					t.Errorf("%s: cmd/sayso depends on %s", goos, dep)
				}
			}
		}
	}
	// The check sees the runtime's dependencies, so it would notice them.
	cmd := exec.Command(gobin, "list", "-deps", "github.com/watzon/sayso/cmd/sayso-cleanup")
	cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
	out, err := cmd.Output()
	if err != nil || !strings.Contains(string(out), "github.com/jupiterrider/ffi") {
		t.Fatalf("go list of cmd/sayso-cleanup does not show the libffi binding: %v", err)
	}
}
