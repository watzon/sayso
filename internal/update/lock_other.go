//go:build !unix

package update

import "sync"

// InstallLock has no file lock where there is no self-update (Windows):
// only one thread of this process swaps at a time.
type InstallLock struct{ busy sync.Mutex }

// SharedLock gives a lock that never waits.
func SharedLock(string) (*InstallLock, bool, error) { return &InstallLock{}, false, nil }

// TryExclusive takes the in-process lock.
func (l *InstallLock) TryExclusive() (release func(), ok bool) {
	if !l.busy.TryLock() {
		return nil, false
	}
	return l.busy.Unlock, true
}

// Close does nothing.
func (l *InstallLock) Close() {}
