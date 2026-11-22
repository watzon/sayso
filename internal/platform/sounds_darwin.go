package platform

// Sound cues with NSSound, which decodes the WAV bytes and plays them on its
// own thread. Each sound is kept until it has finished.

import (
	"errors"
	"time"

	"github.com/ebitengine/purego/objc"
)

type macSounds struct{}

func (macSounds) Play(cue SoundCue, wavData []byte, volume float32) error {
	if len(wavData) == 0 {
		return errors.New("sounds: no audio data for " + string(cue))
	}
	if err := loadDarwin(); err != nil {
		return err
	}
	if volume <= 0 {
		return nil
	}
	if volume > 1 {
		volume = 1
	}
	var sound objc.ID
	var length float64
	var played bool
	withPool(func() {
		sound = class("NSSound").Send(sel("alloc")).Send(sel("initWithData:"), nsData(wavData))
		if sound == 0 {
			return
		}
		sound.Send(sel("setVolume:"), volume)
		length = objc.Send[float64](sound, sel("duration"))
		played = objc.Send[bool](sound, sel("play"))
	})
	if sound == 0 {
		return errors.New("sounds: cannot decode the audio for " + string(cue))
	}
	if !played {
		sound.Send(sel("release"))
		return errors.New("sounds: cannot play " + string(cue))
	}
	// Release after the end; releasing early would stop the sound.
	time.AfterFunc(time.Duration(length*float64(time.Second))+500*time.Millisecond, func() {
		withPool(func() { sound.Send(sel("release")) })
	})
	return nil
}
