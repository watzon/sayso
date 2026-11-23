package update

import (
	"os"
	"regexp"
	"testing"
)

// Version is the version of the release (Cargo.toml) also in a build that
// does not set it with -ldflags: go run, go install, and the tests. The
// updater compares it with the newest release, so a stale Version offers the
// same release again and again.
func TestVersionMatchesCargoToml(t *testing.T) {
	b, err := os.ReadFile("../../Cargo.toml")
	if err != nil {
		t.Fatal(err)
	}
	m := regexp.MustCompile(`(?m)^version = "([^"]+)"`).FindSubmatch(b)
	if m == nil {
		t.Fatal("no version in Cargo.toml")
	}
	if Version != string(m[1]) {
		t.Fatalf("internal/update/version.go has %s, Cargo.toml has %s: set the same version in both", Version, m[1])
	}
}
