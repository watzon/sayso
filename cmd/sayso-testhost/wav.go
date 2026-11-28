package main

import (
	"sync"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/sdk/go/audio"
)

// epoch is the capture clock of all sources, so the echo rule compares the
// microphone with the system audio.
var epoch = time.Now()

const frameSamples = audio.SampleRate / 50

// wavSource plays a WAV file as a capture, at speed times real time, and
// then silence until it stops.
type wavSource struct {
	path  string
	speed float64
}

func (w *wavSource) Open(feed bridge.Feed) (func(), error) {
	a, err := audio.ReadFile(w.path)
	if err != nil {
		return nil, &bridge.OpenError{Kind: bridge.OpenFailed, Message: err.Error()}
	}
	samples := a.AllFloats()
	start := time.Since(epoch)
	stop := make(chan struct{})
	done := make(chan struct{})
	go func() {
		defer close(done)
		tick := time.NewTicker(time.Duration(float64(20*time.Millisecond) / w.speed))
		defer tick.Stop()
		silence := make([]float32, frameSamples)
		for at := 0; ; at += frameSamples {
			select {
			case <-stop:
				return
			case <-tick.C:
			}
			frame := silence
			if at < len(samples) {
				frame = samples[at:min(at+frameSamples, len(samples))]
			}
			feed.Audio(bridge.Captured{Samples: frame, Time: start + time.Duration(at)*time.Second/audio.SampleRate, Timed: true})
		}
	}()
	var once sync.Once
	return func() {
		once.Do(func() {
			close(stop)
			<-done
		})
	}, nil
}

type systemAudio struct{ *wavSource }

func (s systemAudio) Open(_ *uint32, feed bridge.Feed) (func(), []string, error) {
	stop, err := s.wavSource.Open(feed)
	return stop, []string{}, err
}
