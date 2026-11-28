package device

import (
	"context"
	"errors"
	"sync"
	"time"
)

// Options of Open. The callbacks run on the goroutines of the capture, one
// at a time, and must not block.
type Options struct {
	// Device is the ID of a device of Devices, or "" for the default one. A
	// cpal ID ("coreaudio:<UID>") works too.
	Device string
	// OnChunk gets the audio.
	OnChunk func(Chunk)
	// OnGap says that this much audio was lost, for example while the device
	// changed.
	OnGap func(ms uint64)
	// OnEnd says that the capture ended before Stop and does not come back.
	// err wraps ErrDenied when the permission went.
	OnEnd func(err error)
}

const (
	// StallAfter without audio, the capture starts again: the device went,
	// or the system stopped it.
	StallAfter = 2 * time.Second
	// gapAfter is the smallest hole in the times of the audio that is a gap.
	gapAfter = 100 * time.Millisecond
	// maxRestarts in a row that give no audio end the capture.
	maxRestarts = 3
)

// Stream is a capture that runs.
type Stream struct {
	b    Backend
	opts Options

	// cb is held while a callback runs, so they run one at a time and none
	// runs after Stop.
	cb       sync.Mutex
	mu       sync.Mutex
	last     time.Time // the end of the last audio on the clock of the chunks
	lastSeen time.Time // when the last audio came
	restarts int
	stopped  bool
	ended    bool

	stop     chan struct{}
	done     chan struct{}
	stopOnce sync.Once
}

// Open starts b. Its audio goes to opts.OnChunk until Stop. stall is how
// long it waits for audio before it starts b again (StallAfter). An error
// wraps ErrDenied when the system refused the permission.
func Open(b Backend, opts Options, stall time.Duration) (*Stream, error) {
	s := &Stream{b: b, opts: opts, stop: make(chan struct{}), done: make(chan struct{})}
	s.lastSeen = time.Now()
	if err := b.Start(context.Background(), NormalizeDeviceID(opts.Device), s.chunk); err != nil {
		return nil, err
	}
	go s.watch(stall)
	return s, nil
}

func (s *Stream) chunk(c Chunk) {
	s.cb.Lock()
	defer s.cb.Unlock()
	s.mu.Lock()
	if s.stopped || s.ended {
		s.mu.Unlock()
		return
	}
	var gap time.Duration
	if !s.last.IsZero() && !c.Time.IsZero() {
		gap = c.Time.Sub(s.last)
	}
	dur := time.Duration(len(c.Samples)) * time.Second / 16000
	if !c.Time.IsZero() {
		s.last = c.Time.Add(dur)
	}
	s.lastSeen = time.Now()
	s.restarts = 0
	s.mu.Unlock()
	if gap >= gapAfter && s.opts.OnGap != nil {
		s.opts.OnGap(uint64(gap.Milliseconds()))
	}
	if s.opts.OnChunk != nil {
		s.opts.OnChunk(c)
	}
}

// watch starts the capture again when no audio comes for stall, and ends it
// after maxRestarts that gave none.
func (s *Stream) watch(stall time.Duration) {
	defer close(s.done)
	tick := time.NewTicker(stall / 4)
	defer tick.Stop()
	for {
		select {
		case <-s.stop:
			return
		case <-tick.C:
		}
		s.mu.Lock()
		quiet := time.Since(s.lastSeen) >= stall
		if quiet {
			s.restarts++
			s.lastSeen = time.Now()
		}
		restarts := s.restarts
		s.mu.Unlock()
		if !quiet {
			continue
		}
		var err error
		if restarts > maxRestarts {
			err = errors.New("the microphone gives no audio")
		} else {
			s.b.Stop()
			// The device of the user can be gone: the default one goes on.
			if err = s.b.Start(context.Background(), NormalizeDeviceID(s.opts.Device), s.chunk); err != nil && s.opts.Device != "" && !errors.Is(err, ErrDenied) {
				err = s.b.Start(context.Background(), "", s.chunk)
			}
		}
		if err != nil {
			s.end(err)
			return
		}
	}
}

func (s *Stream) end(err error) {
	s.mu.Lock()
	if s.stopped || s.ended {
		s.mu.Unlock()
		return
	}
	s.ended = true
	s.mu.Unlock()
	// The backend waits for its callback, which can wait for cb.
	s.b.Stop()
	s.cb.Lock()
	defer s.cb.Unlock()
	if s.opts.OnEnd != nil {
		s.opts.OnEnd(err)
	}
}

// Stop ends the capture. No callback runs after it returns.
func (s *Stream) Stop() {
	s.stopOnce.Do(func() {
		s.mu.Lock()
		s.stopped = true
		s.mu.Unlock()
		close(s.stop)
		<-s.done
		s.b.Stop()
		s.cb.Lock()
		s.cb.Unlock()
	})
}
