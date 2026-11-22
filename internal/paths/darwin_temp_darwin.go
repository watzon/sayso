//go:build darwin

package paths

import (
	"os"
	"sync"

	"github.com/ebitengine/purego"
)

// _CS_DARWIN_USER_TEMP_DIR from <unistd.h>.
const csDarwinUserTempDir = 65537

var (
	confstrOnce sync.Once
	confstr     func(name int32, buf *byte, size uintptr) uintptr
)

// darwinUserTempDir calls confstr(_CS_DARWIN_USER_TEMP_DIR) through libSystem,
// without cgo. Each process of the user gets the same folder, also when
// Finder starts it. It falls back to TMPDIR, which launchd sets to the same.
func darwinUserTempDir() string {
	confstrOnce.Do(func() {
		lib, err := purego.Dlopen("/usr/lib/libSystem.B.dylib", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			return
		}
		purego.RegisterLibFunc(&confstr, lib, "confstr")
	})
	if confstr == nil {
		return os.Getenv("TMPDIR")
	}
	size := confstr(csDarwinUserTempDir, nil, 0)
	if size == 0 {
		return os.Getenv("TMPDIR")
	}
	buf := make([]byte, size)
	written := confstr(csDarwinUserTempDir, &buf[0], size)
	if written == 0 || written > size {
		return os.Getenv("TMPDIR")
	}
	return string(buf[:written-1])
}
