package sayso_test

import (
	"os"
	"path/filepath"
	"slices"
	"sync"
	"testing"
	"time"

	sayso "github.com/watzon/sayso/sdk/go"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/speech"
)

// wavSource is a capture that plays samples in real time, 20 ms at a time,
// on the clock that the sources of a session share.
type wavSource struct {
	samples []int16
	epoch   time.Time
	pid     *uint32
	mu      sync.Mutex
	opened  int
}

func (w *wavSource) play(feed speech.Feed) func() {
	w.mu.Lock()
	w.opened++
	w.mu.Unlock()
	stop := make(chan struct{})
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < len(w.samples); i += 320 {
			select {
			case <-stop:
				return
			case <-time.After(2 * time.Millisecond):
			}
			end := min(i+320, len(w.samples))
			f := make([]float32, end-i)
			for j, v := range w.samples[i:end] {
				f[j] = float32(v) / 32768
			}
			feed.Audio(speech.Captured{Samples: f, Time: time.Duration(i) * time.Second / 16000, Timed: true})
		}
	}()
	var once sync.Once
	return func() { once.Do(func() { close(stop); <-done }) }
}

func (w *wavSource) Open(feed speech.Feed) (func(), error) { return w.play(feed), nil }

type wavSystem struct{ wavSource }

func (w *wavSystem) Open(pid *uint32, feed speech.Feed) (func(), []string, error) {
	w.mu.Lock()
	w.pid = pid
	w.mu.Unlock()
	return w.play(feed), []string{"sayso"}, nil
}

func TestLocalModeCapturesTheMicrophoneAndRecordsIt(t *testing.T) {
	m := &wavSource{samples: testbin.Speech(900)}
	s := connect(t, sayso.Options{Mode: sayso.Local, Capture: sayso.CaptureSources{Microphone: m}})
	if f := s.Features(); !f.MicCapture {
		t.Fatalf("%+v", f)
	}
	if err := s.DownloadModel(ctx(t), "fake-stream", "tests", nil); err != nil {
		t.Fatal(err)
	}
	rec := filepath.Join(t.TempDir(), "rec")
	sess, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{sayso.SourceMic}, Model: "fake-stream", RecordTo: rec})
	if err != nil {
		t.Fatal(err)
	}
	var seen []string
	seg := next[*protocol.SegmentEvent](t, s, &seen)
	if seg.Session == nil || *seg.Session != sess.ID || seg.Source == nil || *seg.Source != sayso.SourceMic || seg.Text != "alpha bravo charlie" {
		t.Fatalf("%+v", seg)
	}
	text, err := s.StopSession(ctx(t), sess.ID)
	if err != nil || text != "alpha bravo charlie" {
		t.Fatal(text, err)
	}
	if fi, err := os.Stat(filepath.Join(rec, "mic.wav")); err != nil || fi.Size() < 44+2*16000 {
		t.Fatal(fi, err)
	}
}

func TestLocalModeDropsTheEchoOfTheSystemFromTheMicrophone(t *testing.T) {
	speech := testbin.Speech(900)
	m := &wavSource{samples: speech}
	sys := &wavSystem{wavSource{samples: speech}}
	s := connect(t, sayso.Options{Mode: sayso.Local, Capture: sayso.CaptureSources{Microphone: m, SystemAudio: sys}})
	if err := s.DownloadModel(ctx(t), "fake-final", "tests", nil); err != nil {
		t.Fatal(err)
	}
	sess, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{sayso.SourceMic, sayso.SourceSystem}})
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(sess.Excludes, []string{"sayso"}) {
		t.Fatal(sess.Excludes)
	}
	time.Sleep(500 * time.Millisecond)
	text, err := s.StopSession(ctx(t), sess.ID)
	if err != nil {
		t.Fatal(err)
	}
	if text != "alpha bravo charlie" {
		t.Fatalf("the microphone heard the speakers; the text is %q", text)
	}
}

func TestLocalModeWithCaptureOffRefusesTheMicrophone(t *testing.T) {
	s := connect(t, sayso.Options{Mode: sayso.Local, Capture: sayso.CaptureSources{Off: true}})
	if f := s.Features(); f.MicCapture || f.SystemCapture {
		t.Fatalf("%+v", f)
	}
	if _, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{sayso.SourceSystem}}); !isUnsupported(err) {
		t.Fatal(err)
	}
}

func isUnsupported(err error) bool {
	e, ok := err.(*sayso.Error)
	return ok && e.Code == protocol.CodeUnsupported
}
