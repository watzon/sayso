package sayso

import (
	"errors"
	"sync"
	"time"

	"github.com/watzon/sayso/sdk/go/capture/mic"
	"github.com/watzon/sayso/sdk/go/capture/system"
	"github.com/watzon/sayso/sdk/go/speech"
)

// CaptureSources are the captures of Local mode for the sources mic and
// system. The zero value captures the microphone and, where the system can,
// its sound. Set Off for a Local mode that takes only the source client.
type CaptureSources struct {
	Off bool
	// MicDevice is the microphone, "" for the default one (mic.Devices).
	MicDevice string
	// Microphone and SystemAudio replace the captures of this package, for
	// example with fakes in tests.
	Microphone  speech.Microphone
	SystemAudio speech.SystemAudio
}

type captureSources struct {
	mic    speech.Microphone
	system speech.SystemAudio
}

func (c CaptureSources) resolve() captureSources {
	if c.Off {
		return captureSources{}
	}
	out := captureSources{mic: c.Microphone, system: c.SystemAudio}
	if out.mic == nil {
		out.mic = &liveMic{device: c.MicDevice}
	}
	if out.system == nil && system.Supported() {
		out.system = liveSystem{}
	}
	return out
}

// captureEpoch is the start of the capture clock that the sources of a
// session share, so the echo rule can compare their times.
var captureEpoch = time.Now()

func captured(c mic.Chunk) speech.Captured {
	return speech.Captured{Samples: c.Samples, Time: c.Time.Sub(captureEpoch), Timed: !c.Time.IsZero()}
}

func sourceEnd(err error) speech.SourceEnd {
	if errors.Is(err, mic.ErrDenied) {
		return speech.SourceEnd{Reason: speech.EndPermission, Message: err.Error()}
	}
	return speech.SourceEnd{Reason: speech.EndFailed, Message: err.Error()}
}

func openError(err error) error {
	switch {
	case errors.Is(err, mic.ErrDenied):
		return &speech.OpenError{Kind: speech.OpenPermission, Message: err.Error()}
	case errors.Is(err, system.ErrUnsupported):
		return &speech.OpenError{Kind: speech.OpenUnsupported, Message: err.Error()}
	}
	return &speech.OpenError{Kind: speech.OpenFailed, Message: err.Error()}
}

type liveMic struct{ device string }

func (m *liveMic) Open(feed speech.Feed) (func(), error) {
	if mic.RequestPermission() == mic.Denied {
		return nil, openError(mic.ErrDenied)
	}
	s, err := mic.Open(mic.Options{
		Device:  m.device,
		OnChunk: func(c mic.Chunk) { feed.Audio(captured(c)) },
		OnGap:   feed.Gap,
		OnEnd:   func(err error) { feed.Ended(sourceEnd(err)) },
	})
	if err != nil {
		return nil, openError(err)
	}
	var once sync.Once
	return func() { once.Do(s.Stop) }, nil
}

type liveSystem struct{}

func (liveSystem) Open(appPID *uint32, feed speech.Feed) (func(), []string, error) {
	c, err := system.Open(system.Options{
		ExcludePID: appPID,
		OnChunk:    func(c system.Chunk) { feed.Audio(captured(c)) },
		OnGap:      feed.Gap,
		OnEnd:      func(err error) { feed.Ended(sourceEnd(err)) },
	})
	if err != nil {
		return nil, nil, openError(err)
	}
	var once sync.Once
	return func() { once.Do(c.Stop) }, c.Excludes(), nil
}
