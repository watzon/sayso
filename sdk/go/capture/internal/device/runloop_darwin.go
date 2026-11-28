package device

import "time"

// stopRunLoop stops the run loop until its thread says that it ended.
func stopRunLoop(runLoop uintptr, done <-chan struct{}) {
	for {
		cfRunLoopStop(runLoop)
		select {
		case <-done:
			return
		case <-time.After(20 * time.Millisecond):
		}
	}
}
