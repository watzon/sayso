package platform

import (
	"encoding/binary"
	"errors"
	"sync"
	"unsafe"
)

// windowsSounds plays a WAV cue with PlaySoundW from memory, without waiting.
// The volume scales the samples of a 16-bit PCM file; another format plays at
// its own level. A new cue stops the one before it.

const (
	sndAsync     = 0x0001
	sndNoDefault = 0x0002
	sndMemory    = 0x0004
)

type windowsSounds struct {
	mu sync.Mutex
	// playing keeps the bytes alive while PlaySoundW plays them.
	playing []byte
}

func (s *windowsSounds) Play(_ SoundCue, wavData []byte, volume float32) error {
	if len(wavData) == 0 {
		return errors.New("the sound is empty")
	}
	data := scaleWAV(wavData, min(max(volume, 0), 1))
	s.mu.Lock()
	defer s.mu.Unlock()
	if r, _, _ := procPlaySoundW.Call(uintptr(unsafe.Pointer(&data[0])), 0, sndMemory|sndAsync|sndNoDefault); r == 0 {
		return errors.New("Windows could not play the sound")
	}
	s.playing = data
	return nil
}

// scaleWAV is a copy of a WAV file with its 16-bit PCM samples scaled by
// volume. A file it does not understand comes back unchanged.
func scaleWAV(b []byte, volume float32) []byte {
	out := append([]byte(nil), b...)
	if volume >= 1 || len(b) < 12 || string(b[0:4]) != "RIFF" || string(b[8:12]) != "WAVE" {
		return out
	}
	le := binary.LittleEndian
	pcm16 := false
	for pos := 12; pos+8 <= len(out); {
		id, size := string(out[pos:pos+4]), int(le.Uint32(out[pos+4:]))
		body := pos + 8
		if size < 0 || body+size > len(out) {
			size = len(out) - body
		}
		switch id {
		case "fmt ":
			if size >= 16 {
				pcm16 = le.Uint16(out[body:]) == 1 && le.Uint16(out[body+14:]) == 16
			}
		case "data":
			if !pcm16 {
				return out
			}
			for i := body; i+2 <= body+size; i += 2 {
				v := float32(int16(le.Uint16(out[i:]))) * volume
				le.PutUint16(out[i:], uint16(int16(v)))
			}
			return out
		}
		pos = body + size + size&1
	}
	return out
}
