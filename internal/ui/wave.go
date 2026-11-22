package ui

import (
	"math"
	"time"
)

// liveWave holds the input levels of a recording and the playhead that
// scrolls the ink stroke through them. Levels come in steps (one each
// 20 ms audio frame, and in bursts when the system hands over several
// frames at once). The playhead moves on by each display frame's time and
// reads between two levels, so the stroke flows at the display's rate
// instead of stepping one level at a time.
type liveWave struct {
	ring  [waveKeep]float32
	count int // levels pushed since the reset
	// head is the playhead, in levels: level i is at i.
	head  float64
	first time.Time // the frame time of the first level
	clock motionClock
}

const (
	waveKeep = 64
	// waveRate is the levels per second the playhead assumes until it has
	// measured them: one per 20 ms audio frame.
	waveRate = 50
	// waveLag is how far the playhead aims behind the newest level, so it
	// reads between levels it has and a late level never shows as a jump.
	waveLag = 3
	// waveMaxLag is the farthest the playhead falls behind.
	waveMaxLag = 12
)

// reset forgets the levels, for a new recording.
func (w *liveWave) reset() { *w = liveWave{} }

// push adds the newest level, from 0 to 1.
func (w *liveWave) push(l float32) {
	w.ring[w.count%waveKeep] = max(0, min(l, 1))
	w.count++
}

// advance moves the playhead on to the frame time now.
func (w *liveWave) advance(now time.Time) {
	dt := float64(w.clock.tick(now))
	if w.count == 0 {
		w.head = 0
		return
	}
	if w.first.IsZero() {
		w.first = now
		w.head = float64(w.count - waveLag)
	}
	rate := float64(waveRate)
	if el := now.Sub(w.first).Seconds(); el > 0.25 {
		rate = max(20, min(float64(w.count)/el, 120))
	}
	w.head += dt * rate
	// A gentle pull toward waveLag behind the newest level keeps the
	// playhead with the audio when the measured rate is a little off.
	target := float64(w.count - waveLag)
	w.head += (target - w.head) * min(1, dt*3)
	w.head = max(float64(w.count-waveMaxLag), min(w.head, float64(w.count-1)))
}

// level is level i, or silence outside what the wave holds.
func (w *liveWave) level(i int) float32 {
	if i < 0 || i >= w.count || i < w.count-waveKeep {
		return 0
	}
	return w.ring[i%waveKeep]
}

// at is the level at a position between two levels.
func (w *liveWave) at(pos float64) float32 {
	i := int(math.Floor(pos))
	t := float32(pos - float64(i))
	return w.level(i)*(1-t) + w.level(i+1)*t
}

// points are n levels for the stroke, oldest first, ending at the
// playhead, each a 1-2-1 average of its neighbors (Rust LiveAudio::smoothed).
func (w *liveWave) points(n int) []float32 {
	out := make([]float32, n)
	for i := range out {
		pos := w.head - float64(n-1-i)
		out[i] = (w.at(pos-1) + 2*w.at(pos) + w.at(pos+1)) / 4
	}
	return out
}
