//go:build !windows

package paths

import (
	"os"
	"strings"
)

// SystemSocketEnv reads the real environment.
type SystemSocketEnv struct{}

func (SystemSocketEnv) Var(key string) string { return os.Getenv(key) }

func (SystemSocketEnv) DarwinUserTempDir() string {
	dir := darwinUserTempDir()
	return strings.TrimRight(dir, "/")
}

func (SystemSocketEnv) UID() (uint32, bool) { return uint32(os.Getuid()), true }

func (SystemSocketEnv) UserSID() string { return "" }
