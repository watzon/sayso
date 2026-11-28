package speech

import (
	"context"
	"errors"
	"os"
	"sync"
	"time"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/engine"
)

// EngineAdapter is an engine of the engine package as an Engine. It must be
// the only reader of the events of the engine: Subscribe gives them on.
type EngineAdapter struct {
	e       *engine.Engine
	tmp     string
	timeout time.Duration

	mu     sync.Mutex
	status map[string]ModelStatus
	subs   map[int]chan EngineEvent
	next   int
	done   chan struct{}
}

var _ Engine = (*EngineAdapter)(nil)

// FromEngine follows the events of e. tmp holds the WAV files of its
// requests; "" is the temporary folder of the system.
func FromEngine(e *engine.Engine, tmp string) *EngineAdapter {
	if tmp == "" {
		tmp = os.TempDir()
	}
	a := &EngineAdapter{e: e, tmp: tmp, timeout: 10 * time.Minute, status: map[string]ModelStatus{}, subs: map[int]chan EngineEvent{}, done: make(chan struct{})}
	go a.follow()
	return a
}

// Done is closed when the engine stopped and its events ended.
func (a *EngineAdapter) Done() <-chan struct{} { return a.done }

func (a *EngineAdapter) follow() {
	defer func() {
		a.mu.Lock()
		for id, ch := range a.subs {
			close(ch)
			delete(a.subs, id)
		}
		close(a.done)
		a.mu.Unlock()
	}()
	for m := range a.e.Events() {
		var ev EngineEvent
		switch m.Type {
		case "model_state":
			var s engine.ModelState
			if m.Decode(&s) != nil {
				continue
			}
			st := ModelStatus{State: s.State, Message: s.Message, Reason: s.Reason}
			a.SetStatus(s.Model, st)
			ev = EngineEvent{Kind: EventModelStatus, Model: s.Model, Status: st}
		case "partial":
			var p engine.Partial
			if m.Decode(&p) != nil {
				continue
			}
			ev = EngineEvent{Kind: EventPartial, Session: p.Session, Committed: p.Committed, Tentative: p.Tentative}
		case "speech_start", "speech_end":
			var s engine.Speech
			if m.Decode(&s) != nil {
				continue
			}
			ev = EngineEvent{Kind: EventSpeechStart, Session: s.Session, StartMs: s.StartMs, EndMs: s.EndMs}
			if m.Type == "speech_end" {
				ev.Kind = EventSpeechEnd
			}
		default:
			continue
		}
		a.mu.Lock()
		for _, ch := range a.subs {
			select {
			case ch <- ev:
			default:
			}
		}
		a.mu.Unlock()
	}
}

// SetStatus sets the state of a model, for example from the catalog.
func (a *EngineAdapter) SetStatus(model string, s ModelStatus) {
	a.mu.Lock()
	a.status[model] = s
	a.mu.Unlock()
}

func (a *EngineAdapter) ctx() (context.Context, context.CancelFunc) {
	return context.WithTimeout(context.Background(), a.timeout)
}

// asEngineError gives the code of an error reply of the engine.
func asEngineError(err error) error {
	var ee *engine.Error
	switch {
	case err == nil:
		return nil
	case errors.As(err, &ee):
		return &EngineError{Code: ErrorCode(ee.Code), Message: ee.Message}
	case errors.Is(err, engine.ErrStopped):
		return &EngineError{Code: CodeUnavailable, Message: "the engine stopped"}
	}
	return err
}

func (a *EngineAdapter) Features() []string { return a.e.Hello().Features }

func (a *EngineAdapter) Status(model string) ModelStatus {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.status[model]
}

func (a *EngineAdapter) Load(model string) error {
	ctx, cancel := a.ctx()
	defer cancel()
	if _, err := a.e.Load(ctx, model, nil); err != nil {
		return asEngineError(err)
	}
	a.SetStatus(model, ModelStatus{State: StateReady})
	return nil
}

func (a *EngineAdapter) wav(samples []float32) (string, func(), error) {
	f, err := os.CreateTemp(a.tmp, "sayso-sdk-*.wav")
	if err != nil {
		return "", nil, err
	}
	_, err = f.Write(audio.EncodeWAV(audio.FloatsToI16(samples)))
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	path := f.Name()
	done := func() { os.Remove(path) }
	if err != nil {
		done()
		return "", nil, err
	}
	return path, done, nil
}

func (a *EngineAdapter) final(samples []float32, model, language string, segments bool) (engine.Final, error) {
	path, done, err := a.wav(samples)
	if err != nil {
		return engine.Final{}, err
	}
	defer done()
	ctx, cancel := a.ctx()
	defer cancel()
	f, err := a.e.Transcribe(ctx, engine.TranscribeRequest{Model: model, Path: path, Language: language, Segments: segments})
	return f, asEngineError(err)
}

func (a *EngineAdapter) Transcribe(samples []float32, model, language string) (string, error) {
	f, err := a.final(samples, model, language, false)
	return f.Text, err
}

func (a *EngineAdapter) TranscribeSegments(samples []float32, model, language string) ([]Segment, error) {
	f, err := a.final(samples, model, language, true)
	if err != nil {
		return nil, err
	}
	out := make([]Segment, len(f.Segments))
	for i, s := range f.Segments {
		out[i] = Segment{StartMs: s.StartMs, EndMs: s.EndMs, Text: s.Text}
		for _, w := range s.Words {
			out[i].Words = append(out[i].Words, Word{StartMs: w.StartMs, EndMs: w.EndMs, Text: w.Text})
		}
	}
	if len(out) == 0 && f.Text != "" {
		out = []Segment{{StartMs: 0, EndMs: uint64(len(samples)) * 1000 / SampleRate, Text: f.Text}}
	}
	return out, nil
}

func (a *EngineAdapter) DetectSpeech(model string, samples []float32) ([]SpeechPart, error) {
	path, done, err := a.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := a.ctx()
	defer cancel()
	parts, err := a.e.Vad(ctx, model, path)
	if err != nil {
		return nil, asEngineError(err)
	}
	out := make([]SpeechPart, len(parts))
	for i, p := range parts {
		out[i] = SpeechPart{StartMs: p.StartMs, EndMs: p.EndMs}
	}
	return out, nil
}

func (a *EngineAdapter) Diarize(model string, samples []float32, speakers *uint32) ([]RawTurn, error) {
	path, done, err := a.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := a.ctx()
	defer cancel()
	var n uint32
	if speakers != nil {
		n = *speakers
	}
	d, err := a.e.Diarize(ctx, model, path, n)
	if err != nil {
		return nil, asEngineError(err)
	}
	out := make([]RawTurn, len(d.Turns))
	for i, t := range d.Turns {
		out[i] = RawTurn{StartMs: t.StartMs, EndMs: t.EndMs, Speaker: t.Speaker}
	}
	return out, nil
}

func (a *EngineAdapter) StartStream(session uint64, model, language string) error {
	ctx, cancel := a.ctx()
	defer cancel()
	if language == "auto" {
		language = ""
	}
	return asEngineError(a.e.StreamStart(ctx, session, model, language))
}

func (a *EngineAdapter) StartVad(session uint64, model string) error {
	ctx, cancel := a.ctx()
	defer cancel()
	return asEngineError(a.e.VadStart(ctx, session, model))
}

func (a *EngineAdapter) PushAudio(session uint64, samples []float32) error {
	return asEngineError(a.e.StreamAudio(session, audio.FloatsToI16(samples)))
}

func (a *EngineAdapter) EndStream(session uint64) {
	go func() {
		ctx, cancel := a.ctx()
		defer cancel()
		a.e.StreamEnd(ctx, session)
	}()
}

func (a *EngineAdapter) EndVad(session uint64) error {
	ctx, cancel := a.ctx()
	defer cancel()
	_, err := a.e.StreamEnd(ctx, session)
	return asEngineError(err)
}

func (a *EngineAdapter) Subscribe() (<-chan EngineEvent, func()) {
	ch := make(chan EngineEvent, 4096)
	a.mu.Lock()
	select {
	case <-a.done:
		a.mu.Unlock()
		close(ch)
		return ch, func() {}
	default:
	}
	id := a.next
	a.next++
	a.subs[id] = ch
	a.mu.Unlock()
	var once sync.Once
	return ch, func() {
		once.Do(func() {
			a.mu.Lock()
			if _, ok := a.subs[id]; ok {
				delete(a.subs, id)
				close(ch)
			}
			a.mu.Unlock()
		})
	}
}
