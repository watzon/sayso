package ui

import (
	"fmt"
	"slices"
	"time"
)

// frameProbe measures the gaps between the overlay's frames from the start
// of each recording: the first two seconds, and the four after them. It is
// a development option (LogOverlayFrames); release builds never set it.
type frameProbe struct {
	log func(string)
	// recording is whether the last frame was of a recording; start is the
	// wall time of the recording's first frame.
	recording bool
	start     time.Time
	lastFrame time.Time // the frame time of the last frame, to skip rebuilds
	lastWall  time.Time
	early     []time.Duration // gaps in the first probeEarly
	steady    []time.Duration // gaps from probeEarly to probeEnd
	done      bool
}

const (
	probeEarly = 2 * time.Second
	probeEnd   = 6 * time.Second
)

// LogOverlayFrames reports the overlay's frame gaps of each recording to log.
func (a *App) LogOverlayFrames(log func(string)) { a.ov.probe = &frameProbe{log: log} }

// frame notes a build of the overlay: frame is its frame time (a second
// build in one frame has the same), wall the time it was built.
func (f *frameProbe) frame(frame, wall time.Time, recording bool) {
	if !recording {
		f.recording = false
		return
	}
	if !f.recording {
		*f = frameProbe{log: f.log, recording: true, start: wall, lastFrame: frame, lastWall: wall}
		return
	}
	if frame.Equal(f.lastFrame) || f.done {
		return
	}
	gap := wall.Sub(f.lastWall)
	f.lastFrame, f.lastWall = frame, wall
	switch at := wall.Sub(f.start); {
	case at <= probeEarly:
		f.early = append(f.early, gap)
	case at <= probeEnd:
		f.steady = append(f.steady, gap)
	default:
		f.done = true
		f.log(fmt.Sprintf("overlay frames: start (0-2 s) %s; steady (2-6 s) %s", gapStats(f.early, probeEarly), gapStats(f.steady, probeEnd-probeEarly)))
	}
}

// gapStats is the frame rate, the longest gap and the 95th percentile gap.
func gapStats(gaps []time.Duration, span time.Duration) string {
	if len(gaps) == 0 {
		return "no frames"
	}
	s := slices.Clone(gaps)
	slices.Sort(s)
	p95 := s[min(len(s)-1, len(s)*95/100)]
	ms := func(d time.Duration) float64 { return float64(d.Microseconds()) / 1000 }
	return fmt.Sprintf("%d frames, %.1f fps, max gap %.1f ms, p95 %.1f ms", len(s), float64(len(s))/span.Seconds(), ms(s[len(s)-1]), ms(p95))
}
