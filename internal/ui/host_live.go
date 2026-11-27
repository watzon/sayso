package ui

import (
	"context"
	"errors"
	"sync"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/platform"
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
