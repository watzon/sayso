package embedded

import (
	"errors"
	"sync"
	"time"

	"github.com/watzon/sayso/sdk/go/capture/mic"
	"github.com/watzon/sayso/sdk/go/speech"
)

// CaptureMicrophone captures one input device in the process of the app.
type CaptureMicrophone struct {
	mu      sync.Mutex
	device  string
	onLevel func(float32)
	early   *earlyCapture
}

var _ speech.Microphone = (*CaptureMicrophone)(nil)

// earlyCapture is a capture that runs before its session: it keeps the
// audio until the session takes it.
type earlyCapture struct {
	mu     sync.Mutex
	stream *mic.Stream
	kept   []speech.Captured
	feed   speech.Feed
}

func (e *earlyCapture) audio(c speech.Captured) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.feed != nil {
		e.feed.Audio(c)
		return
	}
	e.kept = append(e.kept, c)
}

func (e *earlyCapture) take(feed speech.Feed) {
	e.mu.Lock()
	defer e.mu.Unlock()
	for _, c := range e.kept {
		feed.Audio(c)
	}
	e.kept, e.feed = nil, feed
}

// SetDevice uses this device from the next session on; "" is the default.
func (m *CaptureMicrophone) SetDevice(device string) {
	m.mu.Lock()
	m.device = device
	m.mu.Unlock()
}

// SetLevel calls onLevel with the level of each frame of the next sessions,
// from 0 to 1. nil stops it.
func (m *CaptureMicrophone) SetLevel(onLevel func(float32)) {
	m.mu.Lock()
	m.onLevel = onLevel
	m.mu.Unlock()
}

var epoch = time.Now()

func (m *CaptureMicrophone) start(audio func(speech.Captured), feed func() speech.Feed) (*mic.Stream, error) {
	m.mu.Lock()
	device, onLevel := m.device, m.onLevel
	m.mu.Unlock()
	s, err := mic.Open(mic.Options{
		Device: device,
		OnChunk: func(c mic.Chunk) {
			if onLevel != nil {
				onLevel(c.RMS)
			}
			audio(speech.Captured{Samples: c.Samples, Time: c.Time.Sub(epoch), Timed: !c.Time.IsZero()})
		},
		OnGap: func(ms uint64) {
			if f := feed(); f != nil {
				f.Gap(ms)
			}
		},
		OnEnd: func(err error) {
			if f := feed(); f != nil {
				reason := speech.EndFailed
				if errors.Is(err, mic.ErrDenied) {
					reason = speech.EndPermission
				}
				f.Ended(speech.SourceEnd{Reason: reason, Message: err.Error()})
			}
		},
	})
	if err != nil {
		kind := speech.OpenFailed
		if errors.Is(err, mic.ErrDenied) {
			kind = speech.OpenPermission
		}
		return nil, &speech.OpenError{Kind: kind, Message: err.Error()}
	}
	return s, nil
}

// OpenEarly starts the capture now, before the models of a session load. The
// next session gets this audio first, so the first words are not lost. A
// session that does not come needs CloseEarly.
func (m *CaptureMicrophone) OpenEarly() error {
	e := &earlyCapture{}
	s, err := m.start(e.audio, func() speech.Feed { e.mu.Lock(); defer e.mu.Unlock(); return e.feed })
	if err != nil {
		return err
	}
	e.stream = s
	m.mu.Lock()
	old := m.early
	m.early = e
	m.mu.Unlock()
	if old != nil {
		old.stream.Stop()
	}
	return nil
}

// CloseEarly stops a capture of OpenEarly that no session took.
func (m *CaptureMicrophone) CloseEarly() {
	m.mu.Lock()
	e := m.early
	m.early = nil
	m.mu.Unlock()
	if e != nil {
		e.stream.Stop()
	}
}

// Open starts the capture for a session, or gives it the early one.
func (m *CaptureMicrophone) Open(feed speech.Feed) (func(), error) {
	m.mu.Lock()
	e := m.early
	m.early = nil
	m.mu.Unlock()
	if e != nil {
		e.take(feed)
		var once sync.Once
		return func() { once.Do(e.stream.Stop) }, nil
	}
	s, err := m.start(feed.Audio, func() speech.Feed { return feed })
	if err != nil {
		return nil, err
	}
	var once sync.Once
	return func() { once.Do(s.Stop) }, nil
}
