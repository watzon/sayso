package store

import (
	"bytes"
	"crypto/rand"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"math"
	"os"
	"path/filepath"
	"strings"

	"github.com/mewkiz/flac"
	"github.com/mewkiz/flac/frame"
	"github.com/mewkiz/flac/meta"
)

const (
	audioExtension = "flac"
	// minFLACSamples: the encoder needs a minimum block, so very short audio is padded.
	minFLACSamples = 16
	flacBlockSize  = 4096
)

// AudioError is a FLAC encode or decode failure.
type AudioError struct{ Msg string }

func (e *AudioError) Error() string { return "audio error: " + e.Msg }

func newUUID() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

// SaveAudio writes 16 kHz mono samples as a 16-bit FLAC file and returns its
// name (<uuid>.flac), through a .part file and a rename.
func (s *Store) SaveAudio(samples []float32) (string, error) {
	if len(samples) == 0 {
		return "", &InvalidInputError{"no audio samples"}
	}
	b, err := EncodeFLAC(samples)
	if err != nil {
		return "", err
	}
	name := newUUID() + "." + audioExtension
	final := filepath.Join(s.audioDir, name)
	temp := final + ".part"
	if err := os.WriteFile(temp, b, 0o644); err != nil {
		return "", err
	}
	if err := os.Rename(temp, final); err != nil {
		return "", err
	}
	return name, nil
}

// LoadAudio reads a stored audio file as samples in [-1, 1].
func (s *Store) LoadAudio(name string) ([]float32, error) {
	b, err := os.ReadFile(s.AudioPath(name))
	if err != nil {
		return nil, err
	}
	return DecodeFLAC(b)
}

// AudioPath is the path of an audio file; only the base name of `name` counts.
func (s *Store) AudioPath(name string) string {
	return filepath.Join(s.audioDir, filepath.Base(filepath.Clean("/"+name)))
}

func (s *Store) removeAudioFile(name string) bool {
	err := os.Remove(s.AudioPath(name))
	switch {
	case err == nil:
		return true
	case errors.Is(err, fs.ErrNotExist):
		return false
	}
	slog.Warn("could not delete audio file", "name", name, "err", err)
	return false
}

func (s *Store) removeAllAudioFiles() {
	entries, err := os.ReadDir(s.audioDir)
	if err != nil {
		return
	}
	for _, e := range entries {
		ext := strings.TrimPrefix(filepath.Ext(e.Name()), ".")
		if ext == audioExtension || ext == "part" {
			if err := os.Remove(filepath.Join(s.audioDir, e.Name())); err != nil {
				slog.Warn("could not delete an audio file", "name", e.Name(), "err", err)
			}
		}
	}
}

func (s *Store) audioUse() (int, uint64) {
	entries, err := os.ReadDir(s.audioDir)
	if err != nil {
		return 0, 0
	}
	files, total := 0, uint64(0)
	for _, e := range entries {
		if filepath.Ext(e.Name()) == "."+audioExtension {
			files++
			if fi, err := e.Info(); err == nil {
				total += uint64(fi.Size())
			}
		}
	}
	return files, total
}

// memWriteSeeker lets the encoder rewrite STREAMINFO at Close.
type memWriteSeeker struct {
	buf []byte
	pos int
}

func (m *memWriteSeeker) Write(p []byte) (int, error) {
	if need := m.pos + len(p); need > len(m.buf) {
		m.buf = append(m.buf, make([]byte, need-len(m.buf))...)
	}
	copy(m.buf[m.pos:], p)
	m.pos += len(p)
	return len(p), nil
}

func (m *memWriteSeeker) Seek(offset int64, whence int) (int64, error) {
	var abs int64
	switch whence {
	case io.SeekStart:
		abs = offset
	case io.SeekCurrent:
		abs = int64(m.pos) + offset
	case io.SeekEnd:
		abs = int64(len(m.buf)) + offset
	}
	if abs < 0 {
		return 0, errors.New("negative position")
	}
	m.pos = int(abs)
	return abs, nil
}

// EncodeFLAC encodes 16 kHz mono samples as 16-bit FLAC, with fixed
// predictors and blocks of 4096 samples.
func EncodeFLAC(samples []float32) ([]byte, error) {
	pcm := make([]int32, len(samples), max(len(samples), minFLACSamples))
	for i, s := range samples {
		v := math.Round(float64(s) * 32768)
		pcm[i] = int32(min(max(v, -32768), 32767))
	}
	for len(pcm) < minFLACSamples {
		pcm = append(pcm, 0)
	}
	info := &meta.StreamInfo{
		BlockSizeMin: flacBlockSize, BlockSizeMax: flacBlockSize,
		SampleRate: AudioSampleRate, NChannels: 1, BitsPerSample: 16, NSamples: uint64(len(pcm)),
	}
	out := &memWriteSeeker{}
	enc, err := flac.NewEncoder(out, info)
	if err != nil {
		return nil, &AudioError{"FLAC encode failed: " + err.Error()}
	}
	enc.EnablePredictionAnalysis(true)
	for start := 0; start < len(pcm); start += flacBlockSize {
		end := min(start+flacBlockSize, len(pcm))
		block := pcm[start:end]
		f := &frame.Frame{
			Header: frame.Header{
				HasFixedBlockSize: true, BlockSize: uint16(len(block)), SampleRate: AudioSampleRate,
				Channels: frame.ChannelsMono, BitsPerSample: 16, Num: uint64(start / flacBlockSize),
			},
			Subframes: []*frame.Subframe{{
				SubHeader: frame.SubHeader{Pred: frame.PredVerbatim},
				Samples:   append([]int32(nil), block...),
				NSamples:  len(block),
			}},
		}
		if err := enc.WriteFrame(f); err != nil {
			return nil, &AudioError{"FLAC write failed: " + err.Error()}
		}
	}
	if err := enc.Close(); err != nil {
		return nil, &AudioError{"FLAC write failed: " + err.Error()}
	}
	return out.buf, nil
}

// DecodeFLAC decodes FLAC to mono samples in [-1, 1], averaging channels.
func DecodeFLAC(b []byte) ([]float32, error) {
	stream, err := flac.New(bytes.NewReader(b))
	if err != nil {
		return nil, &AudioError{"FLAC header: " + err.Error()}
	}
	defer stream.Close()
	scale := 1.0 / float32(int64(1)<<(stream.Info.BitsPerSample-1))
	var out []float32
	for {
		f, err := stream.ParseNext()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, &AudioError{"FLAC decode failed: " + err.Error()}
		}
		n := len(f.Subframes)
		if n == 0 {
			continue
		}
		for i := 0; i < int(f.BlockSize); i++ {
			var sum float32
			for _, sf := range f.Subframes {
				sum += float32(sf.Samples[i]) * scale
			}
			out = append(out, sum/float32(n))
		}
	}
	return out, nil
}
