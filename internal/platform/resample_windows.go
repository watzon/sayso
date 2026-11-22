package platform

import "math"

// streamResampler converts a stream of samples from one rate to another with
// a Hann-windowed sinc (resample.rs of sayso-platform-windows): a table of
// phases when the ratio needs at most 1024 of them (48 kHz and 44.1 kHz to
// 16 kHz both do), else a direct sum per output sample.

const (
	zeroCrossings = 10.0
	maxPhases     = 1024
)

type rsPhase struct{ first, at, length int }

type streamResampler struct {
	passthrough bool
	direct      bool
	buf         []float32
	// Polyphase.
	p, q, index, phase int
	phases             []rsPhase
	weights            []float32
	// Direct.
	step, scale, halfWidth, pos float64
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

func newStreamResampler(inRate, outRate uint32) *streamResampler {
	if inRate == outRate {
		return &streamResampler{passthrough: true}
	}
	d := gcd(uint64(inRate), uint64(outRate))
	p, q := uint64(inRate)/d, uint64(outRate)/d
	if q <= maxPhases {
		return newPolyphase(int(p), int(q))
	}
	step := float64(inRate) / float64(outRate)
	scale := math.Max(step, 1)
	hw := zeroCrossings * scale
	lead := int(math.Ceil(hw))
	return &streamResampler{direct: true, step: step, scale: scale, halfWidth: hw, pos: float64(lead), buf: make([]float32, lead)}
}

func newPolyphase(p, q int) *streamResampler {
	half := int(zeroCrossings) * max(p, q)
	scale := math.Max(float64(p)/float64(q), 1)
	halfWidth := float64(half) / float64(q)
	r := &streamResampler{p: p, q: q}
	for phase := 0; phase < q; phase++ {
		first := -((half - phase) / q)
		last := (phase + half) / q
		at := len(r.weights)
		exact := make([]float64, 0, last-first+1)
		norm := 0.0
		for tap := first; tap <= last; tap++ {
			x := float64(tap) - float64(phase)/float64(q)
			w := sinc(x/scale) * hann(x/halfWidth)
			exact = append(exact, w)
			norm += w
		}
		for _, w := range exact {
			if math.Abs(norm) > 1e-9 {
				w /= norm
			} else {
				w = 0
			}
			r.weights = append(r.weights, float32(w))
		}
		r.phases = append(r.phases, rsPhase{first: first, at: at, length: len(exact)})
	}
	lead := (half + q - 1) / q
	r.index = lead
	r.buf = make([]float32, lead)
	return r
}

// process appends the output for input to out.
func (r *streamResampler) process(input []float32, out []float32) []float32 {
	if r.passthrough {
		return append(out, input...)
	}
	r.buf = append(r.buf, input...)
	var keep int
	if r.direct {
		out, keep = r.runDirect(out)
	} else {
		out, keep = r.runPolyphase(out)
	}
	r.buf = append(r.buf[:0], r.buf[keep:]...)
	return out
}

func (r *streamResampler) runPolyphase(out []float32) ([]float32, int) {
	for {
		ph := r.phases[r.phase]
		first := r.index + ph.first
		if first < 0 || first+ph.length > len(r.buf) {
			break
		}
		var acc float32
		w := r.weights[ph.at : ph.at+ph.length]
		for i, s := range r.buf[first : first+ph.length] {
			acc += s * w[i]
		}
		out = append(out, acc)
		r.phase += r.p
		r.index += r.phase / r.q
		r.phase %= r.q
	}
	keep := min(max(r.index+r.phases[r.phase].first, 0), len(r.buf))
	r.index -= keep
	return out, keep
}

func (r *streamResampler) runDirect(out []float32) ([]float32, int) {
	for {
		first := int(math.Ceil(r.pos - r.halfWidth))
		last := int(math.Floor(r.pos + r.halfWidth))
		if last >= len(r.buf) {
			break
		}
		acc, norm := 0.0, 0.0
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

func frameRMS(s []float32) float32 {
	if len(s) == 0 {
		return 0
	}
	var sum float32
	for _, x := range s {
		sum += x * x
	}
	return float32(math.Sqrt(float64(sum / float32(len(s)))))
}

// downmix averages interleaved channels into mono.
func downmix(interleaved []float32, channels int, out []float32) []float32 {
	out = out[:0]
	if channels <= 1 {
		return append(out, interleaved...)
	}
	for i := 0; i+channels <= len(interleaved); i += channels {
		var sum float32
		for _, s := range interleaved[i : i+channels] {
			sum += s
		}
		out = append(out, sum/float32(channels))
	}
	return out
}
