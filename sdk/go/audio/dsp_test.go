package audio

import (
	"math"
	"testing"
)

func gated(seconds float64, on func(i int) bool) []float32 {
	n := int(seconds * SampleRate)
	out := make([]float32, n)
	for i := range out {
		if on(i) {
			out[i] = float32(0.3 * math.Sin(2*math.Pi*300*float64(i)/SampleRate))
		} else {
			out[i] = float32(0.0005 * math.Sin(float64(i)))
		}
	}
	return out
}

func TestBasicVadFindsSpeechAndCutsLongParts(t *testing.T) {
	parts := DetectSpeech(gated(6, func(i int) bool { return i >= 2*SampleRate && i < 3*SampleRate }))
	if len(parts) != 1 || parts[0].StartMs < 1900 || parts[0].StartMs > 2100 || parts[0].EndMs < 2900 || parts[0].EndMs > 3100 {
		t.Fatal(parts)
	}
	if got := DetectSpeech(gated(3, func(int) bool { return false })); len(got) != 0 {
		t.Fatal("silence", got)
	}
	if got := DetectSpeech(gated(1, func(i int) bool { return i < SampleRate/10 })); len(got) != 0 {
		t.Fatal("a click is no speech", got)
	}
	long := DetectSpeech(gated(65, func(i int) bool { return i%SampleRate >= 2*vadFrame }))
	for _, p := range long {
		if p.EndMs-p.StartMs > vadMaxPartMs {
			t.Fatal("too long", p)
		}
	}
	if len(long) < 3 {
		t.Fatal(long)
	}
}

func TestResamplerKeepsLengthAndTone(t *testing.T) {
	for _, rate := range []uint32{8000, 22050, 44100, 48000, 16000, 96000, 11025} {
		r := NewStreamResampler(rate, SampleRate)
		in := make([]float32, int(rate))
		for i := range in {
			in[i] = float32(math.Sin(2 * math.Pi * 440 * float64(i) / float64(rate)))
		}
		var out []float32
		for i := 0; i < len(in); i += 1000 {
			out = r.Process(in[i:min(i+1000, len(in))], out)
		}
		out = r.Process(make([]float32, rate/10), out)
		if len(out) < SampleRate-50 || len(out) > SampleRate+SampleRate/10+50 {
			t.Fatalf("%d: %d samples", rate, len(out))
		}
		var peak float32
		for _, s := range out[2000:14000] {
			peak = max(peak, float32(math.Abs(float64(s))))
		}
		if peak < 0.9 || peak > 1.1 {
			t.Fatalf("%d: peak %v", rate, peak)
		}
	}
}
