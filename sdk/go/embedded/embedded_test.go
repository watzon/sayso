package embedded_test

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/deepgram"
	"github.com/watzon/sayso/sdk/go/embedded"
	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/speech"
)

func TestMain(m *testing.M) {
	code := m.Run()
	testbin.Cleanup()
	os.Exit(code)
}

// fakeMic plays samples when a session opens it.
type fakeMic struct{ samples []int16 }

func (m fakeMic) Open(feed speech.Feed) (func(), error) {
	done := make(chan struct{})
	go func() {
		for i := 0; i < len(m.samples); i += 320 {
			select {
			case <-done:
				return
			default:
			}
			end := min(i+320, len(m.samples))
			f := make([]float32, end-i)
			for j, v := range m.samples[i:end] {
				f[j] = float32(v) / 32768
			}
			feed.Audio(speech.Captured{Samples: f})
		}
	}()
	return func() {
		select {
		case <-done:
		default:
			close(done)
		}
	}, nil
}

func program(t *testing.T) *embedded.ProgramEngine {
	t.Helper()
	dir := t.TempDir()
	p := embedded.NewProgramEngine(engine.Options{Program: testbin.Path(t, testbin.FakeEngine), ModelsDir: filepath.Join(dir, "models")})
	t.Cleanup(p.Stop)
	return p
}

func until[T embedded.Event](t *testing.T, s *embedded.Speech) T {
	t.Helper()
	timeout := time.After(20 * time.Second)
	for {
		select {
		case ev := <-s.Events():
			if v, ok := ev.(T); ok {
				return v
			}
		case <-timeout:
			var zero T
			t.Fatalf("no %T", zero)
		}
	}
}

func TestAMicSessionInTheProcessOfTheApp(t *testing.T) {
	p := program(t)
	s, err := embedded.Connect(embedded.Options{App: "Notes", Engine: p, Models: p, TempDir: t.TempDir(), Microphone: fakeMic{testbin.Speech(900)}})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if p.IsRunning() {
		t.Fatal("Connect starts no engine")
	}
	models, err := s.Catalog()
	if err != nil || len(models) == 0 {
		t.Fatal(models, err)
	}
	if err := s.Download("fake-stream", 0, nil); err != nil {
		t.Fatal(err)
	}
	if err := s.Load("fake-stream"); err != nil {
		t.Fatal(err)
	}
	o := embedded.MicSession("fake-stream")
	o.PreviewModel = "fake-stream"
	session, err := s.StartSession(o)
	if err != nil {
		t.Fatal(err)
	}
	seg := until[*protocol.SegmentEvent](t, s)
	if seg.Text != "alpha bravo charlie" || seg.Source == nil || *seg.Source != speech.SourceMic {
		t.Fatalf("%+v", seg)
	}
	if text, err := s.StopSession(session); err != nil || text != "alpha bravo charlie" {
		t.Fatal(text, err)
	}
	if _, err := s.StopSession(session); err == nil {
		t.Fatal("a second stop")
	}
}

func TestTheDeepgramLayerTakesTheFinalPass(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte(`{"results":{"channels":[{"alternatives":[{"transcript":"from the cloud"}]}]}}`))
	}))
	defer srv.Close()
	p := program(t)
	s, err := embedded.Connect(embedded.Options{
		App: "Consort", Engine: p, Models: p, TempDir: t.TempDir(), Microphone: fakeMic{testbin.Speech(900)},
		Layer: func(e speech.Engine) speech.Engine { return deepgram.NewFinal(e, deepgram.WithBaseURL("key", srv.URL)) },
	})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	session, err := s.StartSession(embedded.MicSession(deepgram.Model))
	if err != nil {
		t.Fatal(err)
	}
	until[*protocol.SegmentEvent](t, s)
	if text, err := s.StopSession(session); err != nil || text != "from the cloud" {
		t.Fatal(text, err)
	}
}

func TestAClientSourceOfAnUnknownSessionIsABadRequest(t *testing.T) {
	p := program(t)
	s, err := embedded.Connect(embedded.Options{App: "Notes", Engine: p, Models: p, TempDir: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	var e *embedded.Error
	if err := s.SessionAudio(7, []float32{0}); !errors.As(err, &e) || e.Code != protocol.CodeBadRequest {
		t.Fatal(err)
	}
}

func TestAnOldRecordingGoesAndARecentOneStays(t *testing.T) {
	dir := t.TempDir()
	old := filepath.Join(dir, "sayso-live-1-1-mic.wav")
	recent := filepath.Join(dir, "sayso-live-1-2-mic.wav")
	other := filepath.Join(dir, "notes.wav")
	for _, f := range []string{old, recent, other} {
		os.WriteFile(f, []byte("x"), 0o600)
	}
	past := time.Now().Add(-48 * time.Hour)
	os.Chtimes(old, past, past)
	os.Chtimes(other, past, past)
	s, err := embedded.Connect(embedded.Options{Engine: program(t), TempDir: dir})
	if err != nil {
		t.Fatal(err)
	}
	s.Close()
	if _, err := os.Stat(old); err == nil {
		t.Fatal("an old recording stays")
	}
	for _, f := range []string{recent, other} {
		if _, err := os.Stat(f); err != nil {
			t.Fatal(f, err)
		}
	}
}

func TestARepeatedPartialDoesNotReachTheApp(t *testing.T) {
	s, err := embedded.Connect(embedded.Options{Engine: program(t), TempDir: t.TempDir()})
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	src := speech.SourceMic
	for _, text := range []string{"hello", "hello", "hello there"} {
		s.Event(&protocol.PartialEvent{Session: 1, Source: &src, Tentative: text})
	}
	s.Event(&protocol.JobDone{Job: 1})
	if n := len(s.Events()); n != 2 {
		t.Fatal(n)
	}
}
