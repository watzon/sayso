package localengine

import (
	"errors"
	"strconv"
	"sync/atomic"
	"testing"
	"time"
)

// countingStream counts the samples it heard; decode gives the count as text.
type countingStream struct{ heard *atomic.Int64 }

func (c *countingStream) accept(s []float32) { c.heard.Add(int64(len(s))) }
func (c *countingStream) decode() segment {
	return segment{text: strconv.FormatInt(c.heard.Load(), 10)}
}
func (c *countingStream) finish() segment {
	return segment{text: "done " + strconv.FormatInt(c.heard.Load(), 10), endpoint: true}
}
func (c *countingStream) close() {}

type countingPreview struct{ heard *atomic.Int64 }

func (p *countingPreview) start() (previewStream, error) { return &countingStream{p.heard}, nil }

// gatedPreview is a lazy preview whose load waits for the gate. It counts its loads.
func gatedPreview(loads, heard *atomic.Int64) (*lazyPreview, chan struct{}) {
	gate := make(chan struct{})
	return newLazyPreview(func() (livePreview, error) {
		loads.Add(1)
		<-gate
		return &countingPreview{heard}, nil
	}), gate
}

func waitUntil(t *testing.T, what string, done func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !done() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out: %s", what)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func TestNothingLoadsUntilTheFirstStream(t *testing.T) {
	var loads, heard atomic.Int64
	p, _ := gatedPreview(&loads, &heard)
	time.Sleep(20 * time.Millisecond)
	if loads.Load() != 0 || p.isLoaded() {
		t.Fatal("loaded before a stream")
	}
}

func TestStartReturnsWhileTheModelLoadsAndTheStreamCatchesUp(t *testing.T) {
	var loads, heard atomic.Int64
	p, gate := gatedPreview(&loads, &heard)
	s, err := p.start()
	if err != nil {
		t.Fatal(err)
	}
	s.accept(make([]float32, 100))
	if got := s.decode(); got != (segment{}) || heard.Load() != 0 {
		t.Fatalf("the audio waits for the model: %+v %d", got, heard.Load())
	}
	close(gate)
	waitUntil(t, "the model loads", p.isLoaded)
	s.accept(make([]float32, 50))
	if heard.Load() != 150 {
		t.Fatalf("the held audio first, then the new audio: %d", heard.Load())
	}
	if s.decode().text != "150" || s.finish().text != "done 150" {
		t.Fatal("wrong text")
	}
}

func TestStreamsShareOneLoad(t *testing.T) {
	var loads, heard atomic.Int64
	p, gate := gatedPreview(&loads, &heard)
	first, _ := p.start()
	second, _ := p.start()
	close(gate)
	waitUntil(t, "the model loads", p.isLoaded)
	first.accept(make([]float32, 10))
	second.accept(make([]float32, 10))
	third, _ := p.start()
	third.accept(make([]float32, 10))
	if loads.Load() != 1 || heard.Load() != 30 {
		t.Fatalf("loads %d heard %d", loads.Load(), heard.Load())
	}
}

func TestAStreamThatEndsBeforeTheLoadGivesNoText(t *testing.T) {
	var loads, heard atomic.Int64
	p, _ := gatedPreview(&loads, &heard)
	s, _ := p.start()
	s.accept(make([]float32, 100))
	if got := s.finish(); got != (segment{endpoint: true}) {
		t.Fatalf("%+v", got)
	}
}

func TestAStreamHoldsAtMostAMinuteOfAudio(t *testing.T) {
	var loads, heard atomic.Int64
	p, gate := gatedPreview(&loads, &heard)
	s, _ := p.start()
	s.accept(make([]float32, maxHeldSamples))
	s.accept(make([]float32, 1))
	close(gate)
	waitUntil(t, "the model loads", p.isLoaded)
	s.accept(nil)
	if heard.Load() != maxHeldSamples {
		t.Fatalf("%d", heard.Load())
	}
}

func slotOf(p *lazyPreview) slotState {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.state
}

func TestAFailedLoadShowsAtTheNextStartAndThenTriesAgain(t *testing.T) {
	var calls atomic.Int64
	p := newLazyPreview(func() (livePreview, error) {
		if calls.Add(1) == 1 {
			return nil, errors.New("broken model")
		}
		return &countingPreview{new(atomic.Int64)}, nil
	})
	s, _ := p.start()
	waitUntil(t, "the first load fails", func() bool { return slotOf(p) == slotFailed })
	s.accept(make([]float32, 10))
	if s.decode() != (segment{}) {
		t.Fatal("the stream of the failed load stays empty")
	}
	if _, err := p.start(); err == nil || err.Error() != "broken model" {
		t.Fatalf("the failure shows once: %v", err)
	}
	if _, err := p.start(); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, "the second load", p.isLoaded)
	if calls.Load() != 2 {
		t.Fatalf("%d", calls.Load())
	}
}

func TestAPanicInTheLoadIsAFailure(t *testing.T) {
	p := newLazyPreview(func() (livePreview, error) { panic("boom") })
	if _, err := p.start(); err != nil {
		t.Fatal(err)
	}
	waitUntil(t, "the load ends", func() bool { return slotOf(p) == slotFailed })
	if _, err := p.start(); err == nil {
		t.Fatal("no error after a panic")
	}
}

