package device

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"
)

// fakeBackend gives audio when the test sends it. Its starts can fail.
type fakeBackend struct {
	mu      sync.Mutex
	on      func(Chunk)
	starts  []string
	stops   int
	failFor map[string]error
}

func (f *fakeBackend) ListDevices() ([]Device, error) { return nil, nil }
func (f *fakeBackend) Start(_ context.Context, id string, on func(Chunk)) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.starts = append(f.starts, id)
	if err := f.failFor[id]; err != nil {
		return err
	}
	f.on = on
	return nil
}
func (f *fakeBackend) Stop() error { f.mu.Lock(); f.stops++; f.on = nil; f.mu.Unlock(); return nil }
func (f *fakeBackend) send(c Chunk) {
	f.mu.Lock()
	on := f.on
	f.mu.Unlock()
	if on != nil {
		on(c)
	}
}
func (f *fakeBackend) started() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.starts...)
}

func frame(at time.Time) Chunk { return Chunk{Samples: make([]float32, 320), Time: at} }

func eventually(t *testing.T, what string, ok func() bool) {
	t.Helper()
	for end := time.Now().Add(3 * time.Second); time.Now().Before(end); time.Sleep(5 * time.Millisecond) {
		if ok() {
			return
		}
	}
	t.Fatal(what)
}

func TestAHoleInTheTimesOfTheAudioIsAGap(t *testing.T) {
	b := &fakeBackend{}
	var mu sync.Mutex
	var gaps []uint64
	chunks := 0
	s, err := Open(b, Options{Device: "coreaudio:built-in", OnChunk: func(Chunk) { mu.Lock(); chunks++; mu.Unlock() }, OnGap: func(ms uint64) { mu.Lock(); gaps = append(gaps, ms); mu.Unlock() }}, time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Stop()
	if got := b.started(); len(got) != 1 || got[0] != "built-in" {
		t.Fatal("the cpal id is the device", got)
	}
	t0 := time.Now()
	b.send(frame(t0))
	b.send(frame(t0.Add(20 * time.Millisecond)))
	b.send(frame(t0.Add(540 * time.Millisecond)))
	mu.Lock()
	defer mu.Unlock()
	if chunks != 3 || len(gaps) != 1 || gaps[0] != 500 {
		t.Fatal(chunks, gaps)
	}
}

func TestACaptureWithNoAudioStartsAgainOnTheDefaultDeviceThenEnds(t *testing.T) {
	b := &fakeBackend{failFor: map[string]error{}}
	ended := make(chan error, 1)
	s, err := Open(b, Options{Device: "usb", OnEnd: func(err error) { ended <- err }}, 40*time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Stop()
	b.mu.Lock()
	b.failFor["usb"] = errors.New("the device is gone")
	b.mu.Unlock()
	eventually(t, "a start on the default device", func() bool {
		got := b.started()
		return len(got) >= 3 && got[1] == "usb" && got[2] == ""
	})
	select {
	case err := <-ended:
		if err == nil {
			t.Fatal("no reason")
		}
	case <-time.After(3 * time.Second):
		t.Fatal("a capture that never gives audio must end")
	}
}

func TestARefusedRestartEndsWithTheDeniedError(t *testing.T) {
	b := &fakeBackend{failFor: map[string]error{}}
	ended := make(chan error, 1)
	s, err := Open(b, Options{OnEnd: func(err error) { ended <- err }}, 40*time.Millisecond)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Stop()
	b.mu.Lock()
	b.failFor[""] = ErrDenied
	b.mu.Unlock()
	select {
	case err := <-ended:
		if !errors.Is(err, ErrDenied) {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("no end")
	}
}

func TestNoCallbackRunsAfterStop(t *testing.T) {
	b := &fakeBackend{}
	var mu sync.Mutex
	stopped := false
	late := false
	s, err := Open(b, Options{OnChunk: func(Chunk) {
		mu.Lock()
		late = late || stopped
		mu.Unlock()
	}}, time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	on := b.on
	s.Stop()
	mu.Lock()
	stopped = true
	mu.Unlock()
	on(frame(time.Now()))
	if late {
		t.Fatal("a chunk after Stop")
	}
}

func TestNormalizeDeviceID(t *testing.T) {
	for in, want := range map[string]string{"coreaudio:ABC": "ABC", "alsa:hw:0,0": "hw:0,0", "plain": "plain", "WASAPI:{x}": "{x}", "other:x": "other:x"} {
		if got := NormalizeDeviceID(in); got != want {
			t.Errorf("%s: %s", in, got)
		}
	}
}
