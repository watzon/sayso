package audio

import (
	"encoding/binary"
	"math"
	"os"
)

// WAVHeaderBytes is the size of the header that WAVHeader writes.
const WAVHeaderBytes = 44

// WAVHeader is the header of a 16 kHz mono 16-bit WAV file with this many
// samples. A WAV file holds at most 4 GiB, 37 hours of this audio.
func WAVHeader(samples uint64) []byte {
	data := uint32(math.MaxUint32 - 36)
	if samples*2 <= uint64(data) {
		data = uint32(samples * 2)
	}
	b := make([]byte, 0, WAVHeaderBytes)
	b = append(b, "RIFF"...)
	b = binary.LittleEndian.AppendUint32(b, 36+data)
	b = append(b, "WAVEfmt "...)
	b = binary.LittleEndian.AppendUint32(b, 16)
	b = binary.LittleEndian.AppendUint16(b, 1)
	b = binary.LittleEndian.AppendUint16(b, 1)
	b = binary.LittleEndian.AppendUint32(b, SampleRate)
	b = binary.LittleEndian.AppendUint32(b, SampleRate*2)
	b = binary.LittleEndian.AppendUint16(b, 2)
	b = binary.LittleEndian.AppendUint16(b, 16)
	b = append(b, "data"...)
	b = binary.LittleEndian.AppendUint32(b, data)
	return b
}

// EncodeWAV is a whole 16 kHz mono 16-bit WAV file.
func EncodeWAV(samples []int16) []byte {
	out := WAVHeader(uint64(len(samples)))
	for _, s := range samples {
		out = binary.LittleEndian.AppendUint16(out, uint16(s))
	}
	return out
}

// WriteWAV writes samples as a WAV file that only this user can read.
func WriteWAV(path string, samples []int16) error {
	return os.WriteFile(path, EncodeWAV(samples), 0o600)
}

// FloatsToI16 is samples from -1 to 1 as 16-bit PCM, with the scale of the
// protocol (32768), clamped.
func FloatsToI16(samples []float32) []int16 {
	out := make([]int16, len(samples))
	for i, s := range samples {
		out[i] = int16(math.Max(-32768, math.Min(32767, math.Round(float64(s)*32768))))
	}
	return out
}

// I16ToFloats is 16-bit PCM as floats from -1 to 1.
func I16ToFloats(samples []int16) []float32 {
	out := make([]float32, len(samples))
	for i, s := range samples {
		out[i] = float32(s) / 32768
	}
	return out
}
