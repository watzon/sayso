//go:build unix

package update

import (
	"os"
	"path/filepath"
	"sync"

	"golang.org/x/sys/unix"
)

// InstallLock: every running Sayso of one install holds a shared flock; a
// swap needs the exclusive lock, so it waits until no other copy runs.
type InstallLock struct {
	file *os.File
	busy sync.Mutex
}

// SharedLock takes the shared lock. waited is true when a swap held it.
func SharedLock(path string) (*InstallLock, bool, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, false, err
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return nil, false, err
	}
	l := &InstallLock{file: f}
	if unix.Flock(int(f.Fd()), unix.LOCK_SH|unix.LOCK_NB) == nil {
		return l, false, nil
	}
	if err := unix.Flock(int(f.Fd()), unix.LOCK_SH); err != nil {
		f.Close()
		return nil, false, err
	}
	return l, true, nil
}

// TryExclusive takes the exclusive lock; release returns to shared.
func (l *InstallLock) TryExclusive() (release func(), ok bool) {
	if !l.busy.TryLock() {
		return nil, false
	}
	if unix.Flock(int(l.file.Fd()), unix.LOCK_EX|unix.LOCK_NB) != nil {
		unix.Flock(int(l.file.Fd()), unix.LOCK_SH)
		l.busy.Unlock()
		return nil, false
	}
	return func() {
		unix.Flock(int(l.file.Fd()), unix.LOCK_SH)
		l.busy.Unlock()
	}, true
}

// Close drops the lock.
func (l *InstallLock) Close() { l.file.Close() }
