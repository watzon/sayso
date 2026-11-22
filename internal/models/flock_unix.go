//go:build !windows

package models

import (
	"errors"
	"os"

	"golang.org/x/sys/unix"
)

// flock locks like Rust's std File::lock on Unix, so Go, Rust, and Swift
// engines see each other's locks.
func lockFile(f *os.File, exclusive, wait bool) (bool, error) {
	how := unix.LOCK_SH
	if exclusive {
		how = unix.LOCK_EX
	}
	if !wait {
		how |= unix.LOCK_NB
	}
	for {
		err := unix.Flock(int(f.Fd()), how)
		if err == nil {
			return true, nil
		}
		if errors.Is(err, unix.EINTR) {
			continue
		}
		if !wait && errors.Is(err, unix.EWOULDBLOCK) {
			return false, nil
		}
		return false, err
	}
}
