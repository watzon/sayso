package ui

import (
	"context"
	"errors"
	"sync"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/sdk/go/capture/mic"
	"github.com/watzon/sayso/sdk/go/capture/system"
)

// captureEpoch is the start of the capture clock that the sources of a live
// session share, so the echo rule can compare their times.
var captureEpoch = time.Now()

// liveMic is the microphone of live sessions. It has a capture of its own
// (platform.Services.LiveMic), so a session and a dictation never stop each
// other; the system mixes the two captures of one device.
type liveMic struct {
	capture platform.AudioCapture
	// device is the microphone that the user chose, read at each open.
	device func() string
}

func (m *liveMic) Open(feed bridge.Feed) (func(), error) {
	err := m.capture.Start(context.Background(), m.device(), func(c platform.AudioChunk) {
		feed.Audio(bridge.Captured{Samples: c.Samples, Time: c.Time.Sub(captureEpoch), Timed: !c.Time.IsZero()})
	})
	if err != nil {
		kind := bridge.OpenFailed
		if errors.Is(err, platform.ErrMicrophoneDenied) {
			kind = bridge.OpenPermission
		}
		return nil, &bridge.OpenError{Kind: kind, Message: err.Error()}
	}
	var once sync.Once
	return func() { once.Do(func() { m.capture.Stop() }) }, nil
}

// openSystem starts a capture of the system audio; tests replace it.
type openSystem func(system.Options) (systemCapture, error)

type systemCapture interface {
	Excludes() []string
	Stop()
}

func openSystemAudio(o system.Options) (systemCapture, error) { return system.Open(o) }

// liveSystem is the system audio of live sessions (sdk/go/capture/system),
// on the capture clock of the microphone.
type liveSystem struct{ open openSystem }

func (s *liveSystem) Open(appPID *uint32, feed bridge.Feed) (func(), []string, error) {
	c, err := s.open(system.Options{
		ExcludePID: appPID,
		OnChunk: func(c system.Chunk) {
			feed.Audio(bridge.Captured{Samples: c.Samples, Time: c.Time.Sub(captureEpoch), Timed: !c.Time.IsZero()})
		},
		OnGap: feed.Gap,
		OnEnd: func(err error) {
			reason := bridge.EndFailed
			if errors.Is(err, mic.ErrDenied) {
				reason = bridge.EndPermission
			}
			feed.Ended(bridge.SourceEnd{Reason: reason, Message: err.Error()})
		},
	})
	if err != nil {
		kind := bridge.OpenFailed
		switch {
		case errors.Is(err, system.ErrUnsupported):
			kind = bridge.OpenUnsupported
		case errors.Is(err, mic.ErrDenied):
			kind = bridge.OpenPermission
		}
		return nil, nil, &bridge.OpenError{Kind: kind, Message: err.Error()}
	}
	var once sync.Once
	return func() { once.Do(c.Stop) }, c.Excludes(), nil
}
