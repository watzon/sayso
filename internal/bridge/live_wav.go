package bridge

import (
	"bufio"
	"encoding/binary"
	"fmt"
	"math"
	"os"
)

// The recording of one source of a live session (sayso-speech/src/live/wav.rs):
// a 16 kHz mono 16-bit WAV file that grows while the session runs. The sizes
// in the header are right after finish.

const wavHeaderBytes = 44

// liveToI16 is a sample from -1 to 1 as 16-bit PCM, with the scale of
// DecodeF32, so the audio of a client source goes back to the same samples
// that the app sent.
func liveToI16(s float32) int16 {
	return int16(math.Max(-32768, math.Min(32767, math.Round(float64(s)*32768))))
}

type wavWriter struct {
	path    string
	file    *os.File
	buf     *bufio.Writer
	samples uint64
}

// createWav makes the file, or empties a file that exists. A new file is for
// this user only, because a recording holds what people said.
func createWav(path string) (*wavWriter, error) {
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return nil, err
	}
	w := &wavWriter{path: path, file: f, buf: bufio.NewWriterSize(f, 64<<10)}
	if _, err := w.buf.Write(wavHeader(0)); err != nil {
		f.Close()
		return nil, err
	}
	return w, nil
}

func (w *wavWriter) write(samples []float32) error {
	b := make([]byte, 2*len(samples))
	for i, s := range samples {
		binary.LittleEndian.PutUint16(b[2*i:], uint16(liveToI16(s)))
	}
	if _, err := w.buf.Write(b); err != nil {
		return err
	}
	w.samples += uint64(len(samples))
	return nil
}

// finish writes the sizes in the header and closes the file. It gives the
// number of samples.
func (w *wavWriter) finish() (uint64, error) {
	err := w.buf.Flush()
	if err == nil {
		_, err = w.file.WriteAt(wavHeader(w.samples), 0)
	}
	if err == nil {
		err = w.file.Sync()
	}
	if cerr := w.file.Close(); err == nil {
		err = cerr
	}
	return w.samples, err
}

// abandon closes the file after an error.
func (w *wavWriter) abandon() { w.file.Close() }

func wavHeader(samples uint64) []byte {
	// A WAV file holds at most 4 GiB; that is 37 hours of this audio.
	data := uint32(math.MaxUint32 - 36)
	if samples*2 <= uint64(data) {
		data = uint32(samples * 2)
	}
	b := make([]byte, 0, wavHeaderBytes)
	b = append(b, "RIFF"...)
	b = binary.LittleEndian.AppendUint32(b, 36+data)
	b = append(b, "WAVEfmt "...)
	b = binary.LittleEndian.AppendUint32(b, 16)
	b = binary.LittleEndian.AppendUint16(b, 1) // PCM
	b = binary.LittleEndian.AppendUint16(b, 1) // mono
	b = binary.LittleEndian.AppendUint32(b, SampleRate)
	b = binary.LittleEndian.AppendUint32(b, SampleRate*2) // bytes per second
	b = binary.LittleEndian.AppendUint16(b, 2)            // bytes per sample
	b = binary.LittleEndian.AppendUint16(b, 16)           // bits per sample
	b = append(b, "data"...)
	b = binary.LittleEndian.AppendUint32(b, data)
	return b
}

// readLiveWav gives the samples of a recording from -1 to 1. It reads only
// the format that wavWriter writes.
func readLiveWav(path string) ([]float32, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	bad := func(what string) error {
		return fmt.Errorf("%s is not a recording of a live session: %s", path, what)
	}
	if len(b) < wavHeaderBytes || string(b[0:4]) != "RIFF" || string(b[8:16]) != "WAVEfmt " {
		return nil, bad("no RIFF header")
	}
	if string(b[36:40]) != "data" {
		return nil, bad("no data chunk after the format")
	}
	size := int(binary.LittleEndian.Uint32(b[40:44]))
	data := b[wavHeaderBytes:min(wavHeaderBytes+size, len(b))]
	out := make([]float32, len(data)/2)
	for i := range out {
		out[i] = float32(int16(binary.LittleEndian.Uint16(data[2*i:]))) / 32768
	}
	return out, nil
}
