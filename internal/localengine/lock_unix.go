//go:build unix

package localengine

import (
	"errors"
	"os"

	"golang.org/x/sys/unix"
)

// The locks of the model store are flock locks, the locks that Rust's
// File::lock takes on Unix, so a Go engine and a Rust engine share a store.

func lockShared(f *os.File) error    { return flock(f, unix.LOCK_SH) }
func lockExclusive(f *os.File) error { return flock(f, unix.LOCK_EX) }

// tryLockShared and tryLockExclusive report errWouldBlock when another
// process holds the lock.
func tryLockShared(f *os.File) error    { return flock(f, unix.LOCK_SH|unix.LOCK_NB) }
func tryLockExclusive(f *os.File) error { return flock(f, unix.LOCK_EX|unix.LOCK_NB) }

func flock(f *os.File, how int) error {
	for {
		err := unix.Flock(int(f.Fd()), how)
		if errors.Is(err, unix.EINTR) {
			continue
		}
		if errors.Is(err, unix.EWOULDBLOCK) {
			return errWouldBlock
		}
		return err
	}
}

// syncFolder makes a rename in folder last through a power cut.
func syncFolder(folder string) {
	if f, err := os.Open(folder); err == nil {
		_ = f.Sync()
		f.Close()
	}
}
