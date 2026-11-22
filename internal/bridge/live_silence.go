package bridge

import (
	"sync"
	"time"
)

// silenceWatch is the silence limit of a live session (SessionSpec.SilenceLimit,
// sayso-speech/src/live/silence.rs). Each source says how long nobody has
// spoken in it. When every source has been silent for the limit, the watch
// runs its action one time. The clock is the audio that a source heard, so a
// source that gets no audio does not count as silent.
type silenceWatch struct {
	limitMs uint64
	// action runs when the limit passes. False: not now, ask again at the
	// next audio.
	action func() bool
	mu     sync.Mutex
	// silentMs is the silence of each source so far; nil after the action ran.
	silentMs []uint64
}

func newSilenceWatch(limit time.Duration, sources int, action func() bool) *silenceWatch {
	return &silenceWatch{limitMs: uint64(limit.Milliseconds()), action: action, silentMs: make([]uint64, sources)}
}

// heard says that source index has heard silentMs of silence since its last
// speech.
func (w *silenceWatch) heard(index int, silentMs uint64) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.silentMs == nil {
		return
	}
	w.silentMs[index] = silentMs
	for _, ms := range w.silentMs {
		if ms < w.limitMs {
			return
		}
	}
	if w.action() {
		w.silentMs = nil
	}
}
