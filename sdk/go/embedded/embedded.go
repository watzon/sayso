// Package embedded is a live session in the process of the app, with an
// engine layer of the app (such as deepgram.Final) and models that the app
// keeps itself. It never talks to Sayso, and its calls block. sayso.Speech
// is the API for most apps.
package embedded

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/speech"
)

// Options of Connect.
type Options struct {
	// App is the name of the app.
	App string
	// Engine is the speech engine; Models are its models. A ProgramEngine is
	// both.
	Engine speech.Engine
	Models Models
	// Layer wraps the engine of the sessions, for example with
	// deepgram.NewFinal. nil uses Engine.
	Layer func(speech.Engine) speech.Engine
	// TempDir holds the temporary recordings of the sessions. Connect makes
	// it.
	TempDir string
	// Microphone nil is a CaptureMicrophone of the default device.
	Microphone speech.Microphone
}

// SessionOptions are the options of one live session.
type SessionOptions struct {
	Sources      []speech.Source
	FinalModel   string
	PreviewModel string
	Vad          speech.VadRun
	// Language is a code such as "en", or "auto".
	Language     string
	SilenceLimit time.Duration
}

// MicSession is a session of the microphone with one final model, Basic
// VAD, no preview, and no silence limit.
func MicSession(finalModel string) SessionOptions {
	return SessionOptions{Sources: []speech.Source{speech.SourceMic}, FinalModel: finalModel, Vad: speech.VadRun{Basic: true}, Language: "auto"}
}

// Event is what comes on Speech.Events: *protocol.PartialEvent,
// *protocol.SegmentEvent, *protocol.SilenceLimit, or *protocol.PartFailed.
type Event = protocol.Event

// Error is an error of a session or of the engine, with its code.
type Error struct {
	Code    protocol.ErrorCode
	Message string
}

func (e *Error) Error() string { return e.Message }

func asError(err error) error {
	var le *speech.LiveError
	var ee *speech.EngineError
	switch {
	case err == nil:
		return nil
	case errors.As(err, &le):
		return &Error{Code: le.Code, Message: le.Message}
	case errors.As(err, &ee):
		return &Error{Code: ee.Code, Message: ee.Message}
	}
	return err
}

func liveErr(e *speech.LiveError) error {
	if e == nil {
		return nil
	}
	return &Error{Code: e.Code, Message: e.Message}
}

// localConnection is the one owner of the sessions of this package.
const localConnection = 1

// staleRecording: a recording older than this comes from an app that
// crashed. A recording is private, so it does not stay.
const staleRecording = 24 * time.Hour

// Speech runs the live sessions of the app.
type Speech struct {
	app    string
	models Models
	bus    *speech.Bus
	live   speech.LiveSessions
	events chan Event

	mu       sync.Mutex
	partials map[uint64][2]string
	closed   bool
}

// Connect gets ready. It starts no engine and loads no model.
func Connect(opts Options) (*Speech, error) {
	if opts.Engine == nil {
		return nil, errors.New("embedded: Options.Engine is nil")
	}
	if err := os.MkdirAll(opts.TempDir, 0o700); err != nil {
		return nil, err
	}
	removeStaleRecordings(opts.TempDir)
	eng := opts.Engine
	if opts.Layer != nil {
		eng = opts.Layer(eng)
	}
	m := opts.Microphone
	if m == nil {
		m = &CaptureMicrophone{}
	}
	s := &Speech{app: opts.App, models: opts.Models, events: make(chan Event, 1024), partials: map[uint64][2]string{}}
	s.bus = speech.StartBus(eng)
	s.live = speech.NewLiveSessions(speech.LiveDeps{Engine: eng, Bus: s.bus, Microphone: m, RecordingDir: opts.TempDir})
	return s, nil
}

// Events are the events of the sessions. Read them from one place; when
// nobody reads, the oldest wait and new ones are dropped.
func (s *Speech) Events() <-chan Event { return s.events }

// Event passes an event of a session on: partials that changed, segments,
// silence limits, and failed parts.
func (s *Speech) Event(ev protocol.Event) {
	switch e := ev.(type) {
	case *protocol.PartialEvent:
		s.mu.Lock()
		text := [2]string{e.Committed, e.Tentative}
		same := s.partials[e.Session] == text
		s.partials[e.Session] = text
		s.mu.Unlock()
		if same {
			return
		}
	case *protocol.SegmentEvent, *protocol.SilenceLimit, *protocol.PartFailed:
	default:
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return
	}
	select {
	case s.events <- ev:
	default:
	}
}

func (s *Speech) needModels() error {
	if s.models == nil {
		return errors.New("embedded: Options.Models is nil")
	}
	return nil
}

// Catalog is the models of the engine and their state. It starts the engine.
func (s *Speech) Catalog() ([]ModelInfo, error) {
	if err := s.needModels(); err != nil {
		return nil, err
	}
	m, err := s.models.Catalog()
	return m, asError(err)
}

// Download downloads a model at once; the app asks its own user first.
func (s *Speech) Download(model string, sizeBytes uint64, onProgress func(engine.DownloadProgress)) error {
	if err := s.needModels(); err != nil {
		return err
	}
	return asError(s.models.Download(model, sizeBytes, onProgress))
}

// Delete deletes the files of a model.
func (s *Speech) Delete(model string) error {
	if err := s.needModels(); err != nil {
		return err
	}
	return asError(s.models.Delete(model))
}

// Load reads a model into memory, and blocks while it loads.
func (s *Speech) Load(model string) error {
	if err := s.needModels(); err != nil {
		return err
	}
	return asError(s.models.Load(model))
}

// Unload takes a model out of memory.
func (s *Speech) Unload(model string) error {
	if err := s.needModels(); err != nil {
		return err
	}
	return asError(s.models.Unload(model))
}

// StartSession starts a live session, and blocks while its models load. At
// most one session runs at a time.
func (s *Speech) StartSession(o SessionOptions) (uint64, error) {
	lang := o.Language
	if lang == "" {
		lang = "auto"
	}
	spec := speech.SessionSpec{Sources: o.Sources, FinalModel: o.FinalModel, PreviewModel: o.PreviewModel, Vad: o.Vad, Language: lang, SilenceLimit: o.SilenceLimit}
	started, err := s.live.Start(speech.Owner{Connection: localConnection, AppID: s.app}, spec, s)
	if err != nil {
		return 0, liveErr(err)
	}
	return started.Session, nil
}

// SessionAudio is audio of the source client: 16 kHz mono from -1 to 1.
func (s *Speech) SessionAudio(session uint64, samples []float32) error {
	return liveErr(s.live.Audio(localConnection, session, protocol.EncodeF32(samples)))
}

// StopSession ends the audio, waits for the last parts, and gives the text.
// The segment events come before it returns.
func (s *Speech) StopSession(session uint64) (string, error) {
	text, err := s.live.Stop(localConnection, session)
	return text, liveErr(err)
}

// CancelSession stops the session and forgets the parts that wait. It
// returns at once.
func (s *Speech) CancelSession() { s.live.ConnectionClosed(localConnection) }

// Close cancels the session and closes Events.
func (s *Speech) Close() {
	s.live.Close()
	s.bus.Stop()
	s.mu.Lock()
	if !s.closed {
		s.closed = true
		close(s.events)
	}
	s.mu.Unlock()
}

func removeStaleRecordings(dir string) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}
	for _, e := range entries {
		if !strings.HasPrefix(e.Name(), "sayso-live-") {
			continue
		}
		if info, err := e.Info(); err == nil && time.Since(info.ModTime()) > staleRecording {
			os.Remove(filepath.Join(dir, e.Name()))
		}
	}
}
