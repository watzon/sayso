// Package audio reads audio files as 16 kHz mono 16-bit samples, writes
// WAV files, resamples, and finds speech by its level. WAV, FLAC, and MP3
// are pure Go; AAC and M4A decode with ExtAudioFile of macOS, and give
// ErrUnsupported elsewhere.
package audio

import (
	"bufio"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"

	"github.com/hajimehoshi/go-mp3"
	"github.com/mewkiz/flac"
)

// SampleRate is the rate of all audio after capture and decoding.
const SampleRate = 16000

const perMs = SampleRate / 1000

var audioFormats = "The formats are WAV, FLAC, and MP3" + systemFormats + "."

// ErrUnsupported is wrapped by the error of a format that this system or
// build cannot decode.
var ErrUnsupported = errors.New("this system cannot decode this format")

// Audio is 16 kHz mono 16-bit samples.
type Audio struct{ samples []int16 }

// AudioFromSamples wraps samples.
func AudioFromSamples(s []int16) *Audio { return &Audio{samples: s} }

// Samples are the samples.
func (a *Audio) Samples() []int16 { return a.samples }

// LenMs is the length in whole milliseconds.
func (a *Audio) LenMs() uint64 { return uint64(len(a.samples)) / perMs }

// Floats are the samples from start to end as floats.
func (a *Audio) Floats(startMs, endMs uint64) []float32 {
	index := func(ms uint64) int {
		if ms > uint64(math.MaxInt)/perMs {
			return len(a.samples)
		}
		return min(int(ms*perMs), len(a.samples))
	}
	s, e := index(startMs), index(endMs)
	e = max(e, s)
	out := make([]float32, e-s)
	for i, v := range a.samples[s:e] {
		out[i] = float32(v) / 32768
	}
	return out
}

// AllFloats are all samples as floats.
func (a *Audio) AllFloats() []float32 { return a.Floats(0, math.MaxUint64) }

// ReadFile reads WAV, FLAC, or MP3 as 16 kHz mono.
func ReadFile(path string) (*Audio, error) {
	name := filepath.Base(path)
	f, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("cannot open %s: %v", path, err)
	}
	defer f.Close()
	r := bufio.NewReaderSize(f, 64*1024)
	head, err := r.Peek(12)
	if err != nil && len(head) < 4 {
		if errors.Is(err, io.EOF) {
			return nil, fmt.Errorf("%s is not an audio file that can be read. %s", name, audioFormats)
		}
		return nil, fmt.Errorf("cannot read %s: %v", name, err)
	}
	var m mono
	switch {
	case len(head) >= 12 && string(head[:4]) == "RIFF" && string(head[8:12]) == "WAVE":
		err = readWAV(r, name, &m)
	case string(head[:4]) == "fLaC":
		err = readFLAC(r, name, &m)
	case isMP3(head):
		err = readMP3(r, name, &m)
	case len(head) >= 8 && string(head[4:8]) == "ftyp", len(head) >= 2 && head[0] == 0xFF && head[1]&0xF6 == 0xF0:
		samples, err := readSystem(path)
		if err != nil {
			return nil, fmt.Errorf("cannot decode the audio in %s: %w. %s", name, err, audioFormats)
		}
		m.push(samples, 1, SampleRate)
	default:
		return nil, fmt.Errorf("%s is not an audio file that can be read. %s", name, audioFormats)
	}
	if err != nil {
		return nil, err
	}
	return &Audio{samples: m.finish()}, nil
}

func isMP3(head []byte) bool {
	if string(head[:3]) == "ID3" {
		return true
	}
	return len(head) >= 2 && head[0] == 0xFF && head[1]&0xE0 == 0xE0 && head[1]&0x06 != 0
}

// mono mixes channels and resamples to 16 kHz.
type mono struct {
	samples   []int16
	rate      uint32
	resampler *StreamResampler
	heard     uint64
	start     int
	buf       []float32
	mixed     []float32
}

func (m *mono) push(interleaved []float32, channels int, rate uint32) {
	channels = max(channels, 1)
	m.mixed = m.mixed[:0]
	for i := 0; i+channels <= len(interleaved); i += channels {
		var sum float32
		for _, s := range interleaved[i : i+channels] {
			sum += s
		}
		m.mixed = append(m.mixed, sum/float32(channels))
	}
	if m.resampler == nil || m.rate != rate {
		m.flush()
		m.rate, m.resampler, m.heard, m.start = rate, NewStreamResampler(rate, SampleRate), 0, len(m.samples)
	}
	m.heard += uint64(len(m.mixed))
	m.buf = m.resampler.Process(m.mixed, m.buf[:0])
	for _, s := range m.buf {
		m.samples = append(m.samples, toI16(s))
	}
}

func (m *mono) flush() {
	if m.resampler == nil {
		return
	}
	m.buf = m.resampler.Process(make([]float32, m.rate/10), m.buf[:0])
	for _, s := range m.buf {
		m.samples = append(m.samples, toI16(s))
	}
	expected := m.heard * SampleRate / uint64(m.rate)
	if keep := m.start + int(expected); keep < len(m.samples) {
		m.samples = m.samples[:keep]
	}
	m.resampler = nil
}

func (m *mono) finish() []int16 {
	m.flush()
	if m.samples == nil {
		return []int16{}
	}
	return m.samples
}

func toI16(s float32) int16 {
	return int16(math.Round(float64(max(-1, min(1, s))) * 32767))
}

func readWAV(r io.Reader, name string, m *mono) error {
	damaged := func(why string) error { return fmt.Errorf("%s is damaged: %s", name, why) }
	var riff [12]byte
	if _, err := io.ReadFull(r, riff[:]); err != nil {
		return damaged(err.Error())
	}
	var format, channels uint16
	var rate uint32
	var bits uint16
	haveFormat := false
	for {
		var hdr [8]byte
		if _, err := io.ReadFull(r, hdr[:]); err != nil {
			if haveFormat {
				return nil
			}
			return fmt.Errorf("%s has no audio.", name)
		}
		id := string(hdr[:4])
		size := binary.LittleEndian.Uint32(hdr[4:])
		switch id {
		case "fmt ":
			b := make([]byte, size+size%2)
			if _, err := io.ReadFull(r, b); err != nil || size < 16 {
				return damaged("bad fmt chunk")
			}
			format = binary.LittleEndian.Uint16(b[0:])
			channels = binary.LittleEndian.Uint16(b[2:])
			rate = binary.LittleEndian.Uint32(b[4:])
			bits = binary.LittleEndian.Uint16(b[14:])
			if format == 0xFFFE && size >= 26 {
				format = binary.LittleEndian.Uint16(b[24:])
			}
			haveFormat = true
		case "data":
			if !haveFormat || channels == 0 || rate == 0 {
				return damaged("no fmt chunk before the data")
			}
			if !(format == 1 && (bits == 8 || bits == 16 || bits == 24 || bits == 32)) && !(format == 3 && (bits == 32 || bits == 64)) {
				return fmt.Errorf("cannot decode the audio in %s (WAV format %d with %d bits). %s", name, format, bits, audioFormats)
			}
			return readWAVData(r, size, int(channels), rate, format, int(bits), m)
		default:
			if _, err := io.CopyN(io.Discard, r, int64(size)+int64(size%2)); err != nil {
				return fmt.Errorf("%s has no audio.", name)
			}
		}
	}
}

func readWAVData(r io.Reader, size uint32, channels int, rate uint32, format uint16, bits int, m *mono) error {
	bps := bits / 8
	frame := bps * channels
	chunk := make([]byte, frame*4096)
	floats := make([]float32, 0, channels*4096)
	remaining := int64(size)
	if size == 0 || size == 0xFFFFFFFF {
		remaining = math.MaxInt64
	}
	for remaining > 0 {
		want := int64(len(chunk))
		if remaining < want {
			want = remaining / int64(frame) * int64(frame)
			if want == 0 {
				break
			}
		}
		n, err := io.ReadFull(r, chunk[:want])
		n = n / frame * frame
		floats = floats[:0]
		for i := 0; i < n; i += bps {
			b := chunk[i : i+bps]
			var v float32
			switch {
			case format == 3 && bps == 4:
				v = math.Float32frombits(binary.LittleEndian.Uint32(b))
			case format == 3 && bps == 8:
				v = float32(math.Float64frombits(binary.LittleEndian.Uint64(b)))
			case bps == 1:
				v = (float32(b[0]) - 128) / 128
			case bps == 2:
				v = float32(int16(binary.LittleEndian.Uint16(b))) / 32768
			case bps == 3:
				v = float32(int32(uint32(b[0])<<8|uint32(b[1])<<16|uint32(b[2])<<24)>>8) / 8388608
			case bps == 4:
				v = float32(float64(int32(binary.LittleEndian.Uint32(b))) / 2147483648)
			}
			floats = append(floats, v)
		}
		if len(floats) > 0 {
			m.push(floats, channels, rate)
		}
		remaining -= int64(n)
		if err != nil {
			break
		}
	}
	return nil
}

func readFLAC(r io.Reader, name string, m *mono) error {
	stream, err := flac.New(r)
	if err != nil {
		return fmt.Errorf("%s is not an audio file that can be read. %s", name, audioFormats)
	}
	defer stream.Close()
	info := stream.Info
	channels := int(info.NChannels)
	if channels == 0 || info.SampleRate == 0 {
		return fmt.Errorf("%s has no audio.", name)
	}
	scale := float32(int64(1) << (info.BitsPerSample - 1))
	var floats []float32
	for {
		fr, err := stream.ParseNext()
		if err != nil {
			if errors.Is(err, io.EOF) {
				return nil
			}
			if len(m.samples) == 0 {
				return fmt.Errorf("cannot decode the audio in %s: %v", name, err)
			}
			return nil
		}
		n := int(fr.BlockSize)
		floats = floats[:0]
		for i := 0; i < n; i++ {
			for c := 0; c < channels && c < len(fr.Subframes); c++ {
				floats = append(floats, float32(fr.Subframes[c].Samples[i])/scale)
			}
		}
		m.push(floats, channels, info.SampleRate)
	}
}

func readMP3(r io.Reader, name string, m *mono) error {
	d, err := mp3.NewDecoder(r)
	if err != nil {
		return fmt.Errorf("%s is not an audio file that can be read. %s", name, audioFormats)
	}
	rate := uint32(d.SampleRate())
	buf := make([]byte, 4*4096)
	floats := make([]float32, 0, 2*4096)
	for {
		n, err := io.ReadFull(d, buf)
		n = n / 4 * 4
		floats = floats[:0]
		for i := 0; i < n; i += 2 {
			floats = append(floats, float32(int16(binary.LittleEndian.Uint16(buf[i:])))/32768)
		}
		if len(floats) > 0 {
			m.push(floats, 2, rate)
		}
		if err != nil {
			if errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) {
				return nil
			}
			if len(m.samples) == 0 {
				return fmt.Errorf("cannot decode the audio in %s: %v", name, err)
			}
			return nil
		}
	}
}
