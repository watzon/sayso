package testbin

import (
	"math"
	"path/filepath"
	"testing"

	"github.com/watzon/sayso/sdk/go/audio"
)

// Speech is 16 kHz mono audio: 500 ms of silence, then a tone for each
// length in ms, each with 1 s of silence after it. The fake engine hears a
// word for each 300 ms of tone.
func Speech(lengths ...int) []int16 {
	samples := make([]int16, audio.SampleRate/2)
	for _, ms := range lengths {
		for i := 0; i < ms*16; i++ {
			samples = append(samples, int16(9000*math.Sin(2*math.Pi*220*float64(i)/audio.SampleRate)))
		}
		samples = append(samples, make([]int16, audio.SampleRate)...)
	}
	return samples
}

// SpeechWAV writes Speech to a WAV file in a temporary folder of the test.
func SpeechWAV(t testing.TB, lengths ...int) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "speech.wav")
	if err := audio.WriteWAV(path, Speech(lengths...)); err != nil {
		t.Fatal(err)
	}
	return path
}
