package localengine

import (
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"math"
	"os"
	"strings"
)

// SampleRate is the rate of all audio the engine takes.
const SampleRate = 16_000

// msToSamples is a duration in samples.
func msToSamples(ms int) int { return ms * SampleRate / 1000 }

// readWAV reads the samples of a 16 kHz 16-bit PCM WAV file as floats. More
// than one channel is mixed down to mono.
func readWAV(path string) ([]float32, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", path, err)
	}
	samples, err := parseWAV(data)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", path, err)
	}
	return samples, nil
}

func parseWAV(b []byte) ([]float32, error) {
	if len(b) < 12 || string(b[0:4]) != "RIFF" || string(b[8:12]) != "WAVE" {
		return nil, errors.New("not a WAV file")
	}
	var haveFormat bool
	var tag, channels, bits uint16
	var rate uint32
	at := 12
	for at+8 <= len(b) {
		id := string(b[at : at+4])
		size := int(binary.LittleEndian.Uint32(b[at+4 : at+8]))
		end := at + 8 + size
		if end > len(b) || end < at {
			end = len(b)
		}
		body := b[at+8 : end]
		switch {
		case id == "fmt " && len(body) >= 16:
			tag = binary.LittleEndian.Uint16(body[0:])
			channels = binary.LittleEndian.Uint16(body[2:])
			rate = binary.LittleEndian.Uint32(body[4:])
			bits = binary.LittleEndian.Uint16(body[14:])
			haveFormat = true
		case id == "data":
			if !haveFormat {
				return nil, errors.New("the data chunk comes before the format chunk")
			}
			// 1 is PCM. 0xFFFE is the extensible format, which Sayso does not write
			// but other tools do for plain PCM too.
			if !(tag == 1 || tag == 0xFFFE) || bits != 16 {
				return nil, fmt.Errorf("only 16-bit PCM is supported (format %d, %d bits)", tag, bits)
			}
			if rate != SampleRate {
				return nil, fmt.Errorf("audio must be 16 kHz, not %d Hz", rate)
			}
			ch := int(channels)
			if ch < 1 {
				ch = 1
			}
			frames := len(body) / 2 / ch
			out := make([]float32, frames)
			for i := range frames {
				var sum float32
				for c := range ch {
					off := (i*ch + c) * 2
					sum += float32(int16(binary.LittleEndian.Uint16(body[off:])))
				}
				out[i] = sum / float32(ch) / 32768
			}
			return out, nil
		}
		at += 8 + size + (size & 1)
	}
	return nil, errors.New("no data chunk")
}

// decodePCM reads the pcm field of stream_audio: base64 of little-endian
// 16-bit samples.
func decodePCM(pcm string) ([]float32, error) {
	data, err := base64.StdEncoding.DecodeString(pcm)
	if err != nil {
		return nil, errors.New("pcm is not base64")
	}
	out := make([]float32, len(data)/2)
	for i := range out {
		out[i] = float32(int16(binary.LittleEndian.Uint16(data[i*2:]))) / 32768
	}
	return out, nil
}

// splitAudio cuts audio into pieces of at most maxSeconds. Each cut is at the
// quietest point in the last five seconds of a piece, so it rarely falls
// inside a word. maxSeconds <= 0 means no limit.
func splitAudio(samples []float32, maxSeconds int) [][]float32 {
	if maxSeconds <= 0 || maxSeconds > math.MaxInt/SampleRate {
		return [][]float32{samples}
	}
	limit := maxSeconds * SampleRate
	window := SampleRate / 5 // 200 ms
	search := min(5*SampleRate, limit/2)
	var pieces [][]float32
	start := 0
	for len(samples)-start > limit {
		end := start + limit
		best := end
		bestEnergy := float32(math.MaxFloat32)
		for position := end - search; position+window <= end; position += window / 2 {
			var energy float32
			for _, s := range samples[position : position+window] {
				energy += s * s
			}
			if energy < bestEnergy {
				bestEnergy = energy
				best = position + window/2
			}
		}
		pieces = append(pieces, samples[start:best])
		start = best
	}
	return append(pieces, samples[start:])
}

// rms is the root mean square of a block of samples.
func rms(samples []float32) float32 {
	if len(samples) == 0 {
		return 0
	}
	var sum float32
	for _, s := range samples {
		sum += s * s
	}
	return float32(math.Sqrt(float64(sum / float32(len(samples)))))
}

// quietestPoint is the center of the 100 ms window with the lowest energy in
// [from, to), scanned in 20 ms steps. A cut there rarely splits a word.
func quietestPoint(samples []float32, from, to int) int {
	window := msToSamples(100)
	hop := msToSamples(20)
	end := min(to, len(samples))
	start := min(from, end)
	if end-start <= window {
		return (start + end) / 2
	}
	bestEnergy := float32(math.MaxFloat32)
	best := (start + end) / 2
	for at := start; at+window <= end; at += hop {
		if energy := rms(samples[at : at+window]); energy < bestEnergy {
			bestEnergy = energy
			best = at + window/2
		}
	}
	return best
}

// joinTexts joins the texts of pieces. A space goes between two pieces only
// when the left one ends in a script that uses spaces (not Chinese or Japanese).
func joinTexts(parts []string) string {
	var b strings.Builder
	var last rune
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part == "" {
			continue
		}
		if b.Len() > 0 && last < 0x2E80 {
			b.WriteByte(' ')
		}
		b.WriteString(part)
		r := []rune(part)
		last = r[len(r)-1]
	}
	return b.String()
}
