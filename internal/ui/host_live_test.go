package ui

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/sdk/go/capture/mic"
	"github.com/watzon/sayso/sdk/go/capture/system"
)

// oneCapture is a capture that runs one capture at a time, like the
// captures of internal/platform.
type oneCapture struct {
	mu      sync.Mutex
	running bool
	devices []string
	stops   int
	err     error
	chunks  []platform.AudioChunk
}

func (c *oneCapture) ListDevices() ([]platform.AudioDevice, error) { return nil, nil }

func (c *oneCapture) Start(_ context.Context, device string, onChunk func(platform.AudioChunk)) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.err != nil {
		return c.err
	}
	if c.running {
		return errors.New("audio: capture already running")
	}
	c.running = true
	c.devices = append(c.devices, device)
	for _, ch := range c.chunks {
		onChunk(ch)
	}
	return nil
}

func (c *oneCapture) Stop() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.running = false
	c.stops++
	return nil
}

type feedLog struct {
	mu    sync.Mutex
	audio []bridge.Captured
}

func (f *feedLog) Audio(c bridge.Captured) { f.mu.Lock(); f.audio = append(f.audio, c); f.mu.Unlock() }
func (f *feedLog) Gap(uint64)              {}
func (f *feedLog) Ended(bridge.SourceEnd)  {}

func hostWith(services *platform.Services, device *string) *HostServices {
	cfg := config.Default()
	cfg.Audio.InputDevice = device
	return &HostServices{App: New(Deps{Config: cfg, Platform: services})}
}

func TestLiveSessionsGetTheMicrophone(t *testing.T) {
	if mic := hostWith(&platform.Services{Audio: &oneCapture{}}, nil).Microphone(); mic != nil {
		t.Fatal("without a live capture there is no microphone")
	}
	if mic := hostWith(nil, nil).Microphone(); mic != nil {
		t.Fatal("without a platform there is no microphone")
	}
	if mic := hostWith(&platform.Services{LiveMic: &oneCapture{}}, nil).Microphone(); mic == nil {
		t.Fatal("the live capture is the microphone of live sessions")
	}
}

func TestALiveSessionAndADictationCaptureTogether(t *testing.T) {
	dictation, live := &oneCapture{}, &oneCapture{}
	h := hostWith(&platform.Services{Audio: dictation, LiveMic: live}, nil)
	if err := dictation.Start(context.Background(), "", func(platform.AudioChunk) {}); err != nil {
		t.Fatal(err)
	}
	stop, err := h.Microphone().Open(&feedLog{})
	if err != nil {
		t.Fatalf("a live session must open while a dictation captures: %v", err)
	}
	stop()
	if !dictation.running || dictation.stops != 0 {
		t.Fatal("stopping the live session stopped the dictation")
	}
}

func TestTheLiveMicrophoneFeedsTimedAudio(t *testing.T) {
	at := captureEpoch.Add(1500 * time.Millisecond)
	live := &oneCapture{chunks: []platform.AudioChunk{{Samples: []float32{0.1, 0.2}, Time: at}, {Samples: []float32{0.3}}}}
	feed := &feedLog{}
	stop, err := hostWith(&platform.Services{LiveMic: live}, nil).Microphone().Open(feed)
	if err != nil {
		t.Fatal(err)
	}
	defer stop()
	if len(feed.audio) != 2 {
		t.Fatal(feed.audio)
	}
	if a := feed.audio[0]; !a.Timed || a.Time != 1500*time.Millisecond || len(a.Samples) != 2 {
		t.Fatalf("%+v", a)
	}
	if feed.audio[1].Timed {
		t.Fatal("a chunk without a time is not timed")
	}
}

func TestTheLiveMicrophoneStopsOnce(t *testing.T) {
	live := &oneCapture{}
	stop, err := hostWith(&platform.Services{LiveMic: live}, nil).Microphone().Open(&feedLog{})
	if err != nil {
		t.Fatal(err)
	}
	stop()
	stop()
	if live.stops != 1 {
		t.Fatalf("stopped %d times", live.stops)
	}
}

func TestTheLiveMicrophoneReadsTheDeviceAtEachOpen(t *testing.T) {
	live := &oneCapture{}
	device := "usb-mic"
	h := hostWith(&platform.Services{LiveMic: live}, &device)
	mic := h.Microphone()
	stop, _ := mic.Open(&feedLog{})
	stop()
	cfg := config.Default()
	other := "headset"
	cfg.Audio.InputDevice = &other
	h.App.snap.mu.Lock()
	h.App.snap.cfg = &cfg
	h.App.snap.mu.Unlock()
	stop, _ = mic.Open(&feedLog{})
	stop()
	if fmt.Sprint(live.devices) != "[usb-mic headset]" {
		t.Fatal(live.devices)
	}
}

func TestAMicrophoneErrorIsAnOpenError(t *testing.T) {
	cases := []struct {
		err  error
		kind bridge.OpenErrorKind
	}{
		{fmt.Errorf("audio: %w", platform.ErrMicrophoneDenied), bridge.OpenPermission},
		{errors.New("there is no microphone"), bridge.OpenFailed},
	}
	for _, c := range cases {
		_, err := hostWith(&platform.Services{LiveMic: &oneCapture{err: c.err}}, nil).Microphone().Open(&feedLog{})
		var open *bridge.OpenError
		if !errors.As(err, &open) || open.Kind != c.kind || open.Message != c.err.Error() {
			t.Fatalf("%v: got %#v", c.err, err)
		}
	}
}

type fakeSystemCapture struct {
	excludes []string
	stops    int
}

func (c *fakeSystemCapture) Excludes() []string { return c.excludes }
func (c *fakeSystemCapture) Stop()              { c.stops++ }

func TestLiveSystemPassesThePidTheClockAndTheExcludes(t *testing.T) {
	c := &fakeSystemCapture{excludes: []string{"sayso"}}
	var got system.Options
	s := &liveSystem{open: func(o system.Options) (systemCapture, error) { got = o; return c, nil }}
	pid := uint32(42)
	f := &feedLog{}
	stop, excludes, err := s.Open(&pid, f)
	if err != nil || len(excludes) != 1 || excludes[0] != "sayso" || got.ExcludePID == nil || *got.ExcludePID != 42 {
		t.Fatal(err, excludes, got.ExcludePID)
	}
	at := captureEpoch.Add(1500 * time.Millisecond)
	got.OnChunk(system.Chunk{Samples: make([]float32, 320), Time: at})
	if len(f.audio) != 1 || !f.audio[0].Timed || f.audio[0].Time != 1500*time.Millisecond {
		t.Fatalf("%+v", f.audio)
	}
	stop()
	stop()
	if c.stops != 1 {
		t.Fatal("stops", c.stops)
	}
}

func TestLiveSystemMapsTheOpenErrors(t *testing.T) {
	for _, c := range []struct {
		err  error
		kind bridge.OpenErrorKind
	}{
		{system.ErrUnsupported, bridge.OpenUnsupported},
		{fmt.Errorf("loopback: %w", mic.ErrDenied), bridge.OpenPermission},
		{errors.New("no output"), bridge.OpenFailed},
	} {
		s := &liveSystem{open: func(system.Options) (systemCapture, error) { return nil, c.err }}
		_, _, err := s.Open(nil, &feedLog{})
		var oe *bridge.OpenError
		if !errors.As(err, &oe) || oe.Kind != c.kind {
			t.Errorf("%v: %v", c.err, err)
		}
	}
}

func TestTheHostOffersSystemAudioOnlyWhereItCanCaptureIt(t *testing.T) {
	h := &HostServices{}
	if (h.SystemAudio() != nil) != system.Supported() {
		t.Fatal("SystemAudio and system.Supported disagree")
	}
}
