package update

import (
	"os"
	"os/exec"
	"regexp"
	"runtime"
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/localengine"
)

// Version is the one source of the release version, also in a build that
// does not set it with -ldflags: go run, go install, and the tests. The
// updater compares it with the newest release, so a stale Version offers the
// same release again and again. The engines answer hello with their own copy,
// and the engine file check of a release wants the release version there.
func TestTheEnginesHaveTheVersionOfTheApp(t *testing.T) {
	if MustVersion(Version).String() != Version {
		t.Fatalf("Version %q is not a plain x.y.z version", Version)
	}
	if localengine.Version != Version {
		t.Fatalf("internal/localengine has %s, internal/update has %s: set the same version in both", localengine.Version, Version)
	}
	b, err := os.ReadFile("../../native/macos/SaysoEngine/Sources/SaysoEngine/Protocol.swift")
	if err != nil {
		t.Fatal(err)
	}
	m := regexp.MustCompile(`(?m)^let engineVersion = "([^"]+)"`).FindSubmatch(b)
	if m == nil {
		t.Fatal("no engineVersion in Protocol.swift")
	}
	if string(m[1]) != Version {
		t.Fatalf("the Swift engine has %s, internal/update has %s: set the same version in both", m[1], Version)
	}
}

// The scripts and the Release workflow read the version with scripts/version.sh.
func TestVersionScriptPrintsVersion(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("bash script")
	}
	out, err := exec.Command("bash", "../../scripts/version.sh").Output()
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.TrimSpace(string(out)); got != Version {
		t.Fatalf("scripts/version.sh prints %q, Version is %q", got, Version)
	}
}
