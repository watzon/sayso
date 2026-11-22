package platform

// Sound cues on Linux: the WAV is scaled to the volume in process and played
// with pw-play, paplay, or aplay from a temporary file.

import (
	"encoding/binary"
	"errors"
	"os"
	"os/exec"
)

type linuxSounds struct {
	lookPath func(string) (string, error)
}

func (s *linuxSounds) player() (string, error) {
	for _, p := range []string{"pw-play", "paplay", "aplay"} {
		if _, err := s.lookPath(p); err == nil {
			return p, nil
		}
	}
	return "", errors.New("sounds: no player: install pipewire (pw-play), pulseaudio-utils (paplay), or alsa-utils (aplay)")
}

func (s *linuxSounds) Play(cue SoundCue, wavData []byte, volume float32) error {
	if len(wavData) == 0 {
		return errors.New("sounds: no audio data for " + string(cue))
	}
	if volume <= 0 {
		return nil
	}
	scaled, err := scaleWAV(wavData, volume)
	if err != nil {
		return err
	}
	player, err := s.player()
	if err != nil {
		return err
	}
	f, err := os.CreateTemp("", "sayso-cue-*.wav")
	if err != nil {
		return err
	}
	if _, err := f.Write(scaled); err != nil {
		f.Close()
		os.Remove(f.Name())
		return err
	}
	f.Close()
	args := []string{f.Name()}
	if player == "aplay" {
		args = []string{"-q", f.Name()}
	}
	cmd := exec.Command(player, args...)
	if err := cmd.Start(); err != nil {
		os.Remove(f.Name())
		return err
	}
	go func() {
		_ = cmd.Wait()
		os.Remove(f.Name())
	}()
	return nil
}

// scaleWAV returns a copy of a 16-bit PCM WAV with its samples scaled by
// volume (0 to 1). Other formats are played as they are.
func scaleWAV(wav []byte, volume float32) ([]byte, error) {
	if len(wav) < 12 || string(wav[0:4]) != "RIFF" || string(wav[8:12]) != "WAVE" {
		return nil, errors.New("sounds: not a WAV file")
	}
	if volume >= 1 {
		return wav, nil
	}
	out := append([]byte(nil), wav...)
	bits := 0
	for off := 12; off+8 <= len(out); {
		id := string(out[off : off+4])
		size := int(binary.LittleEndian.Uint32(out[off+4:]))
		body := off + 8
		if body+size > len(out) {
			size = len(out) - body
		}
		switch id {
		case "fmt ":
			if size >= 16 && binary.LittleEndian.Uint16(out[body:]) == 1 {
				bits = int(binary.LittleEndian.Uint16(out[body+14:]))
			}
		case "data":
			if bits != 16 {
				return out, nil
			}
			for i := body; i+1 < body+size; i += 2 {
				v := float32(int16(binary.LittleEndian.Uint16(out[i:]))) * volume
				binary.LittleEndian.PutUint16(out[i:], uint16(int16(v)))
			}
			return out, nil
		}
		off = body + size + size%2
	}
	return out, nil
}
