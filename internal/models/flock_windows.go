//go:build windows

package models

import (
	"errors"
	"os"

	"golang.org/x/sys/windows"
)

// lockFile locks like Rust's std File::lock on Windows: LockFileEx on the
// whole range.
func lockFile(f *os.File, exclusive, wait bool) (bool, error) {
	var flags uint32
	if exclusive {
		flags |= windows.LOCKFILE_EXCLUSIVE_LOCK
	}
	if !wait {
		flags |= windows.LOCKFILE_FAIL_IMMEDIATELY
	}
	ol := new(windows.Overlapped)
	err := windows.LockFileEx(windows.Handle(f.Fd()), flags, 0, ^uint32(0), ^uint32(0), ol)
	if err == nil {
		return true, nil
	}
	if !wait && (errors.Is(err, windows.ERROR_LOCK_VIOLATION) || errors.Is(err, windows.ERROR_IO_PENDING)) {
		return false, nil
	}
	return false, err
}
