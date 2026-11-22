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
	// Every platform that Sayso ships, with both tag sets of the bundles.
	for _, p := range []string{"darwin/arm64", "darwin/amd64", "linux/amd64", "linux/arm64", "windows/amd64"} {
		goos, goarch, _ := strings.Cut(p, "/")
		for _, tags := range []string{"", "mygo_noinspector"} {
			cmd := exec.Command(gobin, "list", "-deps", "-tags="+tags, "github.com/watzon/sayso/cmd/sayso")
			cmd.Env = append(os.Environ(), "GOOS="+goos, "GOARCH="+goarch, "CGO_ENABLED=0")
			out, err := cmd.Output()
			if err != nil {
				t.Fatalf("go list for %s %q: %v", p, tags, err)
			}
			for _, dep := range strings.Fields(string(out)) {
				for _, f := range forbidden {
					if dep == f || strings.HasPrefix(dep, f+"/") {
						t.Errorf("%s %q: cmd/sayso depends on %s", p, tags, dep)
					}
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
