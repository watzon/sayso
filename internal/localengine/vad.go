package localengine

// Voice activity detection (docs/engine-protocol.md, "VAD").
//
// The model comes from a detector (sherpa-onnx, a fake in the tests). The
// tracker turns what the detector says into the events of the protocol and
// holds the rules of the protocol. The vad request and a VAD session both
// use it, so the same audio gives the same parts.
//
// sherpa-onnx 1.13.8 does two of the rules (shortest speech, shortest pause).
// Its max_speech_duration only splits at a short pause, which leaves a gap
// under 500 ms between two parts, so the detector gets a very long one and
// the tracker cuts speech at 30 s.

const (
	// minSpeechMs: speech under this is left out.
	minSpeechMs = 250
	// minSilenceMs: a pause under this does not end speech.
	minSilenceMs = 500
	// maxPartMs: the longest part. Longer speech is cut into parts of this length.
	maxPartMs = 30_000
)

// detector is a voice activity detector that hears the audio one window at a
// time. Positions are in samples from the first window.
type detector interface {
	// window is the number of samples in a window.
	window() int
	// accept hears one window.
	accept(window []float32)
	// detected is true while speech is open.
	detected() bool
	// nextSegment is the oldest speech that ended since the last call. The end
	// is where the pause began.
	nextSegment() (start, end uint64, ok bool)
	// flush ends the speech that is open, at the last sample heard.
	flush()
	// close frees the detector.
	close()
}

// vadEvent is an event of a VAD session. Times are in milliseconds. A start
// event has endMs == 0 and isEnd false.
type vadEvent struct {
	isEnd   bool
	startMs uint64
	endMs   uint64
}

func samplesToMs(samples uint64) uint64 { return samples * 1000 / SampleRate }

// tracker feeds audio to a detector and reports the speech parts.
type tracker struct {
	det     detector
	win     int
	pending []float32
	// given is the number of samples of the caller.
	given uint64
	// heard is the number of samples the detector heard: given, then silence at the end.
	heard   uint64
	open    bool
	openAt  uint64
	lastEnd uint64
}

func newTracker(det detector) *tracker {
	return &tracker{det: det, win: det.window()}
}

// accept hears more audio. Any chunk size gives the same events.
func (t *tracker) accept(samples []float32) []vadEvent {
	t.given += uint64(len(samples))
	t.pending = append(t.pending, samples...)
	var events []vadEvent
	t.drain(&events)
	return events
}

// finish closes the part that is open. Do not use the tracker after it.
func (t *tracker) finish() []vadEvent {
	// Give the detector the rest of the last window and a pause, so that speech
	// at the end of the audio ends as other speech does.
	rest := (t.win - len(t.pending)%t.win) % t.win
	silence := rest + msToSamples(minSilenceMs) + 3*t.win
	t.pending = append(t.pending, make([]float32, silence)...)
	var events []vadEvent
	t.drain(&events)
	t.det.flush()
	t.collect(&events)
	if t.open {
		t.open = false
		t.endPart(t.openAt, t.given, &events)
	}
	return events
}

func (t *tracker) drain(events *[]vadEvent) {
	whole := len(t.pending) / t.win * t.win
	for at := 0; at < whole; at += t.win {
		t.det.accept(t.pending[at : at+t.win])
		t.heard += uint64(t.win)
		t.collect(events)
		if t.det.detected() {
			t.openPart(events)
			t.cutLongPart(events)
		}
	}
	t.pending = append(t.pending[:0], t.pending[whole:]...)
}

func (t *tracker) collect(events *[]vadEvent) {
	for {
		start, end, ok := t.det.nextSegment()
		if !ok {
			return
		}
		if t.open {
			start = t.openAt
			t.open = false
		} else {
			start = min(max(start, t.lastEnd), t.given)
			*events = append(*events, vadEvent{startMs: samplesToMs(start)})
		}
		t.endPart(start, end, events)
	}
}

// endPart ends the part from start to end. Speech that ends less than a wait
// after 30 s is longer than a part can be: it becomes two parts, cut so that
// the second part is not shorter than the shortest speech.
func (t *tracker) endPart(start, end uint64, events *[]vadEvent) {
	end = max(min(end, t.given), start)
	maxLen := uint64(msToSamples(maxPartMs))
	if end-start > maxLen {
		cut := min(start+maxLen, end-uint64(msToSamples(minSpeechMs)))
		*events = append(*events,
			vadEvent{isEnd: true, startMs: samplesToMs(start), endMs: samplesToMs(cut)},
			vadEvent{startMs: samplesToMs(cut)},
			vadEvent{isEnd: true, startMs: samplesToMs(cut), endMs: samplesToMs(end)})
	} else {
		*events = append(*events, vadEvent{isEnd: true, startMs: samplesToMs(start), endMs: samplesToMs(end)})
	}
	t.lastEnd = end
}

// openPart reports the start of speech that just began. The detector decides
// after the shortest speech and two windows, so the speech began that long ago.
func (t *tracker) openPart(events *[]vadEvent) {
	if t.open {
		return
	}
	back := uint64(2*t.win + msToSamples(minSpeechMs))
	var start uint64
	if t.heard > back {
		start = t.heard - back
	}
	start = min(max(start, t.lastEnd), t.given)
	t.open, t.openAt = true, start
	*events = append(*events, vadEvent{startMs: samplesToMs(start)})
}

// cutLongPart cuts the open part when it is 30 s long. The cut waits until the
// speech has gone on a pause and a shortest speech past the cut point, so the
// next part is not shorter than the shortest speech.
func (t *tracker) cutLongPart(events *[]vadEvent) {
	maxLen := uint64(msToSamples(maxPartMs))
	wait := uint64(msToSamples(minSilenceMs) + msToSamples(minSpeechMs))
	for t.open {
		start := t.openAt
		if t.heard < start+maxLen+wait {
			return
		}
		cut := start + maxLen
		*events = append(*events,
			vadEvent{isEnd: true, startMs: samplesToMs(start), endMs: samplesToMs(cut)},
			vadEvent{startMs: samplesToMs(cut)})
		t.openAt = cut
		t.lastEnd = cut
	}
}

// speechParts are the parts with speech, as start and end ms pairs, from the
// events of a whole audio.
func speechParts(events []vadEvent) [][2]uint64 {
	parts := [][2]uint64{}
	for _, e := range events {
		if e.isEnd {
			parts = append(parts, [2]uint64{e.startMs, e.endMs})
		}
	}
	return parts
}
