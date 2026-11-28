package audio

import (
	"encoding/binary"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func tone(rate, channels int, seconds float64, amp float64) []byte {
	n := int(float64(rate) * seconds)
	b := []byte("RIFF\x00\x00\x00\x00WAVEfmt ")
	b = binary.LittleEndian.AppendUint32(b, 16)
	b = binary.LittleEndian.AppendUint16(b, 1)
	b = binary.LittleEndian.AppendUint16(b, uint16(channels))
	b = binary.LittleEndian.AppendUint32(b, uint32(rate))
	b = binary.LittleEndian.AppendUint32(b, uint32(rate*channels*2))
	b = binary.LittleEndian.AppendUint16(b, uint16(channels*2))
	b = binary.LittleEndian.AppendUint16(b, 16)
	b = append(b, "data"...)
	b = binary.LittleEndian.AppendUint32(b, uint32(n*channels*2))
	for i := 0; i < n; i++ {
		v := int16(amp * 32767 * math.Sin(2*math.Pi*440*float64(i)/float64(rate)))
		for c := 0; c < channels; c++ {
			b = binary.LittleEndian.AppendUint16(b, uint16(v))
		}
	}
	return b
}

func TestAStereo48kFileReadsAs16kMono(t *testing.T) {
	path := filepath.Join(t.TempDir(), "a.wav")
	os.WriteFile(path, tone(48000, 2, 1.5, 0.5), 0o600)
	a, err := ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if a.LenMs() != 1500 {
		t.Fatalf("%d ms", a.LenMs())
	}
}

func TestEncodeWAVReadsBack(t *testing.T) {
	path := filepath.Join(t.TempDir(), "b.wav")
	in := []int16{0, 1000, -1000, 32767, -32768}
	if err := WriteWAV(path, in); err != nil {
		t.Fatal(err)
	}
	a, err := ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	got := a.Samples()
	if len(got) != len(in) || got[1] != 1000 || got[4] != -32767 {
		t.Fatal(got)
	}
}

func TestAFileThatIsNotAudioSaysTheFormats(t *testing.T) {
	path := filepath.Join(t.TempDir(), "c.txt")
	os.WriteFile(path, []byte("hello, this is text"), 0o600)
	if _, err := ReadFile(path); err == nil || !strings.Contains(err.Error(), "WAV, FLAC, and MP3") {
		t.Fatal(err)
	}
}

func TestSpeechIsFoundBetweenSilences(t *testing.T) {
	samples := make([]float32, SampleRate*3)
	for i := SampleRate; i < 2*SampleRate; i++ {
		samples[i] = float32(0.3 * math.Sin(2*math.Pi*300*float64(i)/SampleRate))
	}
	parts := DetectSpeech(samples)
	if len(parts) != 1 || parts[0].StartMs < 900 || parts[0].StartMs > 1100 || parts[0].EndMs < 1900 || parts[0].EndMs > 2100 {
		t.Fatal(parts)
	}
	if parts := DetectSpeech(make([]float32, SampleRate)); len(parts) != 0 {
		t.Fatal(parts)
	}
}
