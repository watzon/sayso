// Package testbin builds the programs that the tests of the SDK run: the
// fake engine, the fake cleanup runtime, and the test host of the app. An
// environment variable can name a program that is built already.
package testbin

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sync"
	"testing"
)

// Programs and the variables that can name them.
const (
	FakeEngine  = "sayso-fake-engine"
	FakeCleanup = "sayso-fake-cleanup"
	TestHost    = "sayso-testhost"
)

var envOf = map[string]string{FakeEngine: "SAYSO_FAKE_ENGINE", FakeCleanup: "SAYSO_FAKE_CLEANUP", TestHost: "SAYSO_TESTHOST"}

var (
	mu    sync.Mutex
	dir   string
	built = map[string]string{}
	fails = map[string]string{}
)

// ModuleDir is sdk/go of this checkout.
func ModuleDir() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "..")
}

// RepoDir is the root of the checkout, or "" when sdk/go is not in one.
func RepoDir() string {
	root := filepath.Join(ModuleDir(), "..", "..")
	if _, err := os.Stat(filepath.Join(root, "cmd", "sayso-testhost")); err != nil {
		return ""
	}
	return root
}

// Require says whether a program that cannot be built fails the test
// (SAYSO_TEST_REQUIRE_FAKES=1, as in CI) or skips it.
func Require() bool { return os.Getenv("SAYSO_TEST_REQUIRE_FAKES") == "1" }

// Path builds the program once per test process and gives its path.
func Path(t testing.TB, name string) string {
	t.Helper()
	if p := os.Getenv(envOf[name]); p != "" {
		return p
	}
	mu.Lock()
	defer mu.Unlock()
	if p, ok := built[name]; ok {
		return p
	}
	if why, ok := fails[name]; ok {
		missing(t, name, why)
	}
	if dir == "" {
		d, err := os.MkdirTemp("", "sayso-sdk-bin")
		if err != nil {
			t.Fatal(err)
		}
		dir = d
	}
	out := filepath.Join(dir, name)
	if runtime.GOOS == "windows" {
		out += ".exe"
	}
	var cmd *exec.Cmd
	if name == TestHost {
		repo := RepoDir()
		if repo == "" {
			fails[name] = "sdk/go is not in a checkout of the app"
			missing(t, name, fails[name])
		}
		cmd = exec.Command("go", "build", "-o", out, "./cmd/sayso-testhost")
		cmd.Dir = repo
	} else {
		cmd = exec.Command("go", "build", "-o", out, "./cmd/"+name)
		cmd.Dir = ModuleDir()
	}
	cmd.Env = append(os.Environ(), "CGO_ENABLED=0")
	if b, err := cmd.CombinedOutput(); err != nil {
		fails[name] = string(b) + err.Error()
		missing(t, name, fails[name])
	}
	built[name] = out
	return out
}

func missing(t testing.TB, name, why string) {
	t.Helper()
	if Require() {
		t.Fatalf("cannot build %s: %s", name, why)
	}
	t.Skipf("cannot build %s: %s", name, why)
}

// Cleanup removes the programs. TestMain calls it.
func Cleanup() {
	mu.Lock()
	defer mu.Unlock()
	if dir != "" {
		os.RemoveAll(dir)
	}
}
