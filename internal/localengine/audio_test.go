package localengine

import (
	"encoding/base64"
	"encoding/binary"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func testWAV(channels uint16, rate uint32, samples []int16) []byte {
	var data []byte
	for _, s := range samples {
		data = binary.LittleEndian.AppendUint16(data, uint16(s))
	}
	le32 := func(b []byte, v uint32) []byte { return binary.LittleEndian.AppendUint32(b, v) }
	le16 := func(b []byte, v uint16) []byte { return binary.LittleEndian.AppendUint16(b, v) }
	out := []byte("RIFF")
	out = le32(out, 36+uint32(len(data))+10)
	out = append(out, "WAVE"...)
	// An unknown chunk with an odd size, which needs a pad byte.
	out = append(out, "LIST"...)
	out = le32(out, 1)
	out = append(out, 0, 0)
	out = append(out, "fmt "...)
	out = le32(out, 16)
	out = le16(out, 1)
	out = le16(out, channels)
	out = le32(out, rate)
	out = le32(out, rate*2*uint32(channels))
	out = le16(out, 2*channels)
	out = le16(out, 16)
	out = append(out, "data"...)
	out = le32(out, uint32(len(data)))
	return append(out, data...)
}

func TestReadsMonoAndMixesStereo(t *testing.T) {
	got, err := parseWAV(testWAV(1, 16_000, []int16{16384, -16384}))
	if err != nil || !reflect.DeepEqual(got, []float32{0.5, -0.5}) {
		t.Fatalf("mono: %v %v", got, err)
	}
	got, err = parseWAV(testWAV(2, 16_000, []int16{16384, 0, -32768, -32768}))
	if err != nil || !reflect.DeepEqual(got, []float32{0.25, -1.0}) {
		t.Fatalf("stereo: %v %v", got, err)
	}
	if _, err := parseWAV(testWAV(1, 44_100, []int16{0})); err == nil || !strings.Contains(err.Error(), "16 kHz") {
		t.Fatalf("44.1 kHz: %v", err)
	}
	if _, err := parseWAV([]byte("nope")); err == nil {
		t.Fatal("garbage parsed")
	}
}

func TestReadsAWAVFileFromDisk(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "clip.wav")
	samples := make([]int16, SampleRate+1)
	for i := range samples {
		samples[i] = 8192
	}
	if err := os.WriteFile(path, testWAV(1, 16_000, samples), 0o644); err != nil {
		t.Fatal(err)
	}
	got, err := readWAV(path)
	if err != nil || len(got) <= SampleRate {
		t.Fatalf("%d samples, %v", len(got), err)
	}
	if _, err := readWAV(filepath.Join(dir, "missing.wav")); err == nil {
		t.Fatal("missing file read")
	}
}

func TestDecodesBase64PCM(t *testing.T) {
	got, err := decodePCM(base64.StdEncoding.EncodeToString([]byte{0x00, 0x40, 0x00, 0xC0}))
	if err != nil || !reflect.DeepEqual(got, []float32{0.5, -0.5}) {
		t.Fatalf("%v %v", got, err)
	}
	if _, err := decodePCM("***"); err == nil {
		t.Fatal("bad base64 decoded")
	}
}

func filled(n int, v float32) []float32 {
	s := make([]float32, n)
	for i := range s {
		s[i] = v
	}
	return s
}

func TestSplitCutsLongAudioAtAQuietPoint(t *testing.T) {
	if n := len(splitAudio(filled(SampleRate*3, 0.5), 10)); n != 1 {
		t.Fatalf("short audio in %d pieces", n)
	}
	// 25 s of noise with one quiet second at 8 s.
	samples := filled(SampleRate*25, 0.5)
	clear(samples[SampleRate*8 : SampleRate*9])
	pieces := splitAudio(samples, 10)
	if len(pieces) < 3 {
		t.Fatalf("%d pieces", len(pieces))
	}
	if first := len(pieces[0]); first < SampleRate*8 || first > SampleRate*9 {
		t.Fatalf("cut at %d", first)
	}
	total := 0
	for _, p := range pieces {
		total += len(p)
		if len(p) > SampleRate*10 {
			t.Fatalf("piece of %d samples", len(p))
		}
	}
	if total != len(samples) {
		t.Fatalf("pieces hold %d of %d samples", total, len(samples))
	}
}

func TestTheQuietestPointIsInTheSilence(t *testing.T) {
	samples := filled(msToSamples(1_000), 0.5)
	clear(samples[msToSamples(600):msToSamples(800)])
	if p := quietestPoint(samples, 0, len(samples)); p < msToSamples(600) || p >= msToSamples(800) {
		t.Fatalf("point %d", p)
	}
	if p := quietestPoint(samples, 10, 20); p != 15 {
		t.Fatalf("a short range gives its middle, got %d", p)
	}
	if rms(nil) != 0 {
		t.Fatal("rms of nothing")
	}
	if r := rms([]float32{0.5, -0.5}); r < 0.5-1e-6 || r > 0.5+1e-6 {
		t.Fatalf("rms %v", r)
	}
}

func TestJoinUsesSpacesOnlyWhereTheScriptDoes(t *testing.T) {
	if got := joinTexts([]string{" hello ", "", "world"}); got != "hello world" {
		t.Fatalf("%q", got)
	}
	if got := joinTexts([]string{"你好", "世界"}); got != "你好世界" {
		t.Fatalf("%q", got)
	}
}
