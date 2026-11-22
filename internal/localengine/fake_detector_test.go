package localengine

// energyDetector is a detector for tests that follows the detector of
// sherpa-onnx: a window with a root mean square above 0.01 is speech. Speech
// starts after minSpeechMs of it, and ends after minSilenceMs of silence.
type energyDetector struct {
	heard      uint64
	loud       int
	quiet      int
	triggered  bool
	start      uint64
	open       bool
	done       [][2]uint64
	closeCount int
}

const fakeWindow = 512

func (d *energyDetector) window() int { return fakeWindow }

func (d *energyDetector) accept(window []float32) {
	d.heard += uint64(len(window))
	minSpeech := msToSamples(minSpeechMs)
	minSilence := msToSamples(minSilenceMs)
	if rms(window) > 0.01 {
		d.quiet = 0
		d.loud += len(window)
		if !d.triggered && d.loud >= minSpeech {
			d.triggered = true
			back := uint64(2*fakeWindow + minSpeech)
			d.start, d.open = 0, true
			if d.heard > back {
				d.start = d.heard - back
			}
		}
		return
	}
	if !d.triggered {
		d.loud = 0
		return
	}
	d.quiet += len(window)
	if d.quiet >= minSilence {
		end := d.heard - uint64(minSilence)
		d.done = append(d.done, [2]uint64{d.start, end})
		d.open, d.triggered, d.loud, d.quiet = false, false, 0, 0
	}
}

func (d *energyDetector) detected() bool { return d.open }

func (d *energyDetector) nextSegment() (uint64, uint64, bool) {
	if len(d.done) == 0 {
		return 0, 0, false
	}
	s := d.done[0]
	d.done = d.done[1:]
	return s[0], s[1], true
}

func (d *energyDetector) flush() {
	if d.open {
		d.done = append(d.done, [2]uint64{d.start, d.heard})
		d.open, d.triggered = false, false
	}
}

func (d *energyDetector) close() { d.closeCount++ }

type audioPiece struct {
	ms   int
	loud bool
}

// audioOf is audio for the tests: pieces of silence or of loud noise.
func audioOf(pieces ...audioPiece) []float32 {
	var out []float32
	for _, p := range pieces {
		v := float32(0)
		if p.loud {
			v = 0.5
		}
		out = append(out, filled(msToSamples(p.ms), v)...)
	}
	return out
}
