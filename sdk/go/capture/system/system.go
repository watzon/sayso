// Package system captures the sound that the computer plays, as 16 kHz
// mono audio: WASAPI loopback of the default output on Windows, and the
// monitor of the default output on Linux (parec, or pw-record on PipeWire).
// On macOS it gives ErrUnsupported for now (sdk/protocol/audio-helper.md).
package system

import (
	"errors"

	"github.com/watzon/sayso/sdk/go/capture/internal/device"
)

type (
	Chunk  = device.Chunk
	Stream = device.Stream
	State  = device.State
)

const (
	Granted = device.Granted
	Denied  = device.Denied
	Unknown = device.Unknown
)

// ErrUnsupported is the error of Open where this system cannot capture its
// sound.
var ErrUnsupported = errors.New("this system cannot capture the sound that it plays")

// Options of Open. The callbacks are those of mic.Options.
type Options struct {
	// ExcludePID is a process whose sound stays out, where the system can
	// do that (Excludes says it).
	ExcludePID *uint32
	OnChunk    func(Chunk)
	OnGap      func(ms uint64)
	OnEnd      func(err error)
}

// Capture is a capture of the system audio that runs.
type Capture struct {
	stream   *Stream
	excludes []string
}

// Excludes say what the capture leaves out: "sayso" (this process), "app"
// (ExcludePID). Empty where the system mixes everything.
func (c *Capture) Excludes() []string { return append([]string{}, c.excludes...) }

// Stop ends the capture. No callback runs after it returns.
func (c *Capture) Stop() { c.stream.Stop() }

// Supported is true where Open can work.
func Supported() bool { return loopback() != nil }

// Open starts the capture.
func Open(opts Options) (*Capture, error) {
	b := loopback()
	if b == nil {
		return nil, ErrUnsupported
	}
	s, err := device.Open(b, device.Options{OnChunk: opts.OnChunk, OnGap: opts.OnGap, OnEnd: opts.OnEnd}, device.StallAfter)
	if err != nil {
		return nil, err
	}
	return &Capture{stream: s, excludes: []string{}}, nil
}

// Access is the state of the permission to capture the system audio.
func Access() State {
	if !Supported() {
		return Unknown
	}
	return Granted
}

// Request asks for the permission where the system has one, and gives the
// state after it.
func Request() State { return Access() }
