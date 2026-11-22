//go:build windows

package localengine

import (
	"errors"
	"math"
	"os"

	"golang.org/x/sys/windows"
)

// The locks of the model store are LockFileEx locks over the whole file, the
// locks that Rust's File::lock takes on Windows, so a Go engine and a Rust
// engine share a store.

func lockShared(f *os.File) error    { return lockFile(f, 0) }
func lockExclusive(f *os.File) error { return lockFile(f, windows.LOCKFILE_EXCLUSIVE_LOCK) }

// tryLockShared and tryLockExclusive report errWouldBlock when another
// process holds the lock.
func tryLockShared(f *os.File) error {
	return lockFile(f, windows.LOCKFILE_FAIL_IMMEDIATELY)
}

func tryLockExclusive(f *os.File) error {
	return lockFile(f, windows.LOCKFILE_EXCLUSIVE_LOCK|windows.LOCKFILE_FAIL_IMMEDIATELY)
}

func lockFile(f *os.File, flags uint32) error {
	var overlapped windows.Overlapped
	err := windows.LockFileEx(windows.Handle(f.Fd()), flags, 0, math.MaxUint32, math.MaxUint32, &overlapped)
	if errors.Is(err, windows.ERROR_LOCK_VIOLATION) || errors.Is(err, windows.ERROR_IO_PENDING) {
		return errWouldBlock
	}
	return err
}

// syncFolder is a no-op: Windows cannot flush a folder.
func syncFolder(string) {}
