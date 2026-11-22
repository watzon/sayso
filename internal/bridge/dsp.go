package bridge

import "math"

// The energy VAD (sayso-speech/src/basic_vad.rs) and the stream resampler
// (sayso-speech/src/resample.rs).

const (
	vadMinSpeechMs = 250
	vadMinPauseMs  = 500
	vadMaxPartMs   = 30_000
	vadFrameMs     = 10
	vadFrame       = SampleRate * vadFrameMs / 1000
	vadFloorMs     = 5_000
	vadMarginDB    = 10.0
	vadQuietestDB  = -55.0
	vadSilenceDB   = -100.0
)

// BasicVad finds speech by its level above the noise floor of the last 5 s.
type BasicVad struct {
	pending   []float32
	frames    uint64
	floor     [][2]float64 // (frame index, level), a monotonic queue
	open      bool
	openStart uint64
	continued bool
	voiced    uint64
	speechEnd uint64
}

// NewBasicVad is a VAD at the start of audio.
func NewBasicVad() *BasicVad { return &BasicVad{} }

// DetectSpeech runs a BasicVad over all samples.
func DetectSpeech(samples []float32) []SpeechPart {
	v := NewBasicVad()
	return append(v.Accept(samples), v.Finish()...)
}

// Accept takes 16 kHz samples and gives the parts that ended.
func (v *BasicVad) Accept(samples []float32) []SpeechPart {
	var parts []SpeechPart
	v.pending = append(v.pending, samples...)
	n := len(v.pending) / vadFrame
	for i := 0; i < n; i++ {
		v.frame(levelDB(v.pending[i*vadFrame:(i+1)*vadFrame]), &parts)
	}
	v.pending = append([]float32(nil), v.pending[n*vadFrame:]...)
	return parts
}

// Finish gives the part that is still open.
func (v *BasicVad) Finish() []SpeechPart {
	var parts []SpeechPart
	v.close(&parts)
	return parts
}

// Speaking is true while a part is open.
func (v *BasicVad) Speaking() bool { return v.open }

func (v *BasicVad) frame(level float64, parts *[]SpeechPart) {
	index := v.frames
	v.frames++
	for len(v.floor) > 0 && v.floor[len(v.floor)-1][1] >= level {
		v.floor = v.floor[:len(v.floor)-1]
	}
	v.floor = append(v.floor, [2]float64{float64(index), level})
	for len(v.floor) > 0 && uint64(v.floor[0][0])+vadFloorMs/vadFrameMs <= index {
		v.floor = v.floor[1:]
	}
	floor := level
	if len(v.floor) > 0 {
		floor = v.floor[0][1]
	}
	speech := level > math.Max(floor+vadMarginDB, vadQuietestDB)
	if speech {
		if !v.open {
			v.open, v.openStart, v.continued = true, index, false
			v.voiced = 0
		}
		v.voiced++
		v.speechEnd = index + 1
		v.cutLongPart(parts)
	} else if v.open && v.frames-v.speechEnd >= vadMinPauseMs/vadFrameMs {
		v.close(parts)
	}
}

func (v *BasicVad) cutLongPart(parts *[]SpeechPart) {
	const maxFrames = vadMaxPartMs / vadFrameMs
	for v.open {
		start := v.openStart
		if v.speechEnd < start+maxFrames+vadMinSpeechMs/vadFrameMs {
			break
		}
		*parts = append(*parts, framePart(start, start+maxFrames))
		v.openStart, v.continued = start+maxFrames, true
		v.voiced = 0
	}
}

func (v *BasicVad) close(parts *[]SpeechPart) {
	if !v.open {
		return
	}
	v.open = false
	start, end := v.openStart, v.speechEnd
	if !v.continued && v.voiced < vadMinSpeechMs/vadFrameMs {
		return
	}
	const maxFrames = vadMaxPartMs / vadFrameMs
	if end-start > maxFrames {
		cut := min(start+maxFrames, end-vadMinSpeechMs/vadFrameMs)
		*parts = append(*parts, framePart(start, cut), framePart(cut, end))
	} else {
		*parts = append(*parts, framePart(start, end))
	}
}

func framePart(start, end uint64) SpeechPart {
	return SpeechPart{StartMs: start * vadFrameMs, EndMs: end * vadFrameMs}
}

func levelDB(frame []float32) float64 {
	var power float32
	for _, s := range frame {
		power += s * s
	}
	power /= float32(len(frame))
	if power > 0 {
		return math.Max(10*math.Log10(float64(power)), vadSilenceDB)
	}
	return vadSilenceDB
}

const (
	zeroCrossings = 10.0
	maxPhases     = 1024
)

// StreamResampler resamples a stream with a windowed sinc: polyphase when
// the ratio is small, direct otherwise.
type StreamResampler struct {
	kind int // 0 passthrough, 1 polyphase, 2 direct
	buf  []float32
	// polyphase
	p, q    int
	phases  []rsPhase
	weights []float32
	index   int
	phase   int
	// direct
	step, scale, halfWidth, pos float64
}

type rsPhase struct{ first, at, len int }

// NewStreamResampler resamples from in to out Hz.
func NewStreamResampler(in, out uint32) *StreamResampler {
	if in == 0 || out == 0 {
		panic("sample rates must be positive")
	}
	if in == out {
		return &StreamResampler{}
	}
	d := gcd(uint64(in), uint64(out))
	p, q := uint64(in)/d, uint64(out)/d
	if q <= maxPhases {
		return newPolyphase(int(p), int(q))
	}
	return newDirect(in, out)
}

func newPolyphase(p, q int) *StreamResampler {
	half := int(zeroCrossings) * max(p, q)
	scale := math.Max(float64(p)/float64(q), 1)
	halfWidth := float64(half) / float64(q)
	r := &StreamResampler{kind: 1, p: p, q: q}
	for phase := 0; phase < q; phase++ {
		first := -((half - phase) / q)
		last := (phase + half) / q
		at := len(r.weights)
		exact := make([]float64, 0, last-first+1)
		for tap := first; tap <= last; tap++ {
			x := float64(tap) - float64(phase)/float64(q)
			exact = append(exact, sinc(x/scale)*hann(x/halfWidth))
		}
		var norm float64
		for _, w := range exact {
			norm += w
		}
		for _, w := range exact {
			if math.Abs(norm) > 1e-9 {
				r.weights = append(r.weights, float32(w/norm))
			} else {
				r.weights = append(r.weights, 0)
			}
		}
		r.phases = append(r.phases, rsPhase{first: first, at: at, len: len(exact)})
	}
	lead := (half + q - 1) / q
	r.index = lead
	r.buf = make([]float32, lead)
	return r
}

func newDirect(in, out uint32) *StreamResampler {
	step := float64(in) / float64(out)
	scale := math.Max(step, 1)
	halfWidth := zeroCrossings * scale
	lead := int(math.Ceil(halfWidth))
	return &StreamResampler{kind: 2, step: step, scale: scale, halfWidth: halfWidth, pos: float64(lead), buf: make([]float32, lead)}
}

// Process appends the resampled samples of input to out.
func (r *StreamResampler) Process(input []float32, out []float32) []float32 {
	if r.kind == 0 {
		return append(out, input...)
	}
	r.buf = append(r.buf, input...)
	var keep int
	if r.kind == 1 {
		out, keep = r.runPolyphase(out)
	} else {
		out, keep = r.runDirect(out)
	}
	r.buf = append(r.buf[:0], r.buf[keep:]...)
	return out
}

func (r *StreamResampler) runPolyphase(out []float32) ([]float32, int) {
	for {
		ph := r.phases[r.phase]
		first := r.index + ph.first
		if first < 0 || first+ph.len > len(r.buf) {
			break
		}
		out = append(out, dot(r.buf[first:first+ph.len], r.weights[ph.at:ph.at+ph.len]))
		r.phase += r.p
		r.index += r.phase / r.q
		r.phase %= r.q
	}
	keep := min(max(r.index+r.phases[r.phase].first, 0), len(r.buf))
	r.index -= keep
	return out, keep
}

func (r *StreamResampler) runDirect(out []float32) ([]float32, int) {
	for {
		first := int(math.Ceil(r.pos - r.halfWidth))
		last := int(math.Floor(r.pos + r.halfWidth))
		if last >= len(r.buf) {
			break
		}
		var acc, norm float64
		for i := first; i <= last; i++ {
			x := float64(i) - r.pos
			w := sinc(x/r.scale) * hann(x/r.halfWidth)
			acc += float64(r.buf[i]) * w
			norm += w
		}
		if math.Abs(norm) > 1e-9 {
			out = append(out, float32(acc/norm))
		} else {
			out = append(out, 0)
		}
		r.pos += r.step
	}
	keep := min(int(math.Max(math.Floor(r.pos-r.halfWidth), 0)), len(r.buf))
	r.pos -= float64(keep)
	return out, keep
}

func dot(samples, weights []float32) float32 {
	var lanes [8]float32
	n := len(samples) / 8 * 8
	var tail float32
	for i := n; i < len(samples); i++ {
		tail += samples[i] * weights[i]
	}
	for i := 0; i < n; i += 8 {
		for l := 0; l < 8; l++ {
			lanes[l] += samples[i+l] * weights[i+l]
		}
	}
	var sum float32
	for _, l := range lanes {
		sum += l
	}
	return sum + tail
}

func gcd(a, b uint64) uint64 {
	for b != 0 {
		a, b = b, a%b
	}
	return a
}

func sinc(x float64) float64 {
	if math.Abs(x) < 1e-12 {
		return 1
	}
	return math.Sin(math.Pi*x) / (math.Pi * x)
}

func hann(t float64) float64 { return 0.5 * (1 + math.Cos(math.Pi*t)) }
