// Package bridgeengine is the speech engine as the Host API uses it: an
// engine.Client that follows the start and crash events of the engine and
// gives bridge.Engine. The app (internal/ui) and the test host
// (cmd/sayso-testhost) share it.
package bridgeengine

import (
	"context"
	"encoding/json"
	"log"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/models"
)

// Adapter keeps the engine.Client and follows its events.
type Adapter struct {
	c       *engine.Client
	tmp     string
	state   atomic.Int32 // bridge.EngineStart
	timeout time.Duration

	mu   sync.Mutex
	subs map[int]chan engine.Event
	next int
	// onEvent sees every event (on the reader goroutine).
	onEvent func(engine.Event)
	// preload gives the model to load once the engine starts ("" loads none).
	preload func() string
}

var _ bridge.Engine = (*Adapter)(nil)

// Start starts the engine program and follows its events. Each time the
// engine starts it asks for the state of every local model and then loads the
// model preload gives, when that model is on disk. onEvent sees each event on
// the reader goroutine.
func Start(opts engine.Options, preload func() string, onEvent func(engine.Event)) (*Adapter, error) {
	c, events, err := engine.Start(opts)
	if err != nil {
		return nil, err
	}
	tmp := opts.CacheDir
	if tmp == "" {
		tmp = os.TempDir()
	}
	e := &Adapter{c: c, tmp: tmp, timeout: 2 * time.Minute, subs: map[int]chan engine.Event{}, onEvent: onEvent, preload: preload}
	go e.follow(events)
	return e, nil
}

func (e *Adapter) follow(events <-chan engine.Event) {
	for ev := range events {
		switch ev.Kind {
		case engine.EventStarted, engine.EventRestarted:
			e.state.Store(int32(bridge.EngineStarted))
			go e.syncModels()
		case engine.EventStartFailed, engine.EventCrashed:
			e.state.Store(int32(bridge.EngineFailed))
		}
		if e.onEvent != nil {
			e.onEvent(ev)
		}
		e.mu.Lock()
		for _, ch := range e.subs {
			select {
			case ch <- ev:
			default: // a slow subscriber misses events rather than block the engine
			}
		}
		e.mu.Unlock()
	}
}

// localModelRefs are the local models of the catalog with their engine objects.
func localModelRefs() []engine.ModelRef {
	local := models.LocalModels()
	refs := make([]engine.ModelRef, 0, len(local))
	for _, m := range local {
		refs = append(refs, engine.ModelRef{ID: string(m.ID), Engine: Spec(string(m.ID))})
	}
	return refs
}

// syncModels asks the engine which models are on disk (their model_state
// events fill Status), then loads the preload model when it is on disk.
func (e *Adapter) syncModels() {
	ctx, cancel := e.Ctx()
	defer cancel()
	if err := e.c.ListModels(ctx, localModelRefs()); err != nil {
		log.Printf("engine: list models: %v", err)
		return
	}
	if e.preload == nil {
		return
	}
	model := e.preload()
	if _, ok := models.FindLocal(models.ModelID(model)); !ok {
		return
	}
	if s := e.Status(model); s.IsOnDisk() && s.State != engine.StateReady {
		if err := e.Load(model); err != nil {
			log.Printf("engine: preload %s: %v", model, err)
		}
	}
}

// Client is the engine client.
func (e *Adapter) Client() *engine.Client { return e.c }

// Close stops the engine.
func (e *Adapter) Close() error { return e.c.Close() }

// Ctx is the context of one request to the engine.
func (e *Adapter) Ctx() (context.Context, context.CancelFunc) {
	return context.WithTimeout(context.Background(), e.timeout)
}

// Spec is the engine field of a model, from the catalog.
func Spec(model string) json.RawMessage {
	m, ok := models.Find(models.ModelID(model))
	if !ok {
		return nil
	}
	b, err := json.Marshal(m.Engine)
	if err != nil {
		return nil
	}
	return b
}

// Running is true once the engine answered hello.
func (e *Adapter) Running() bool {
	return bridge.EngineStart(e.state.Load()) == bridge.EngineStarted
}

// Download downloads a model and blocks until it is on disk.
func (e *Adapter) Download(model string) error {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Hour)
	defer cancel()
	var size uint64
	if m, ok := models.Find(models.ModelID(model)); ok {
		size = m.SizeBytes
	}
	return e.c.Download(ctx, model, Spec(model), size)
}

// Delete deletes a model's files.
func (e *Adapter) Delete(model string) error {
	ctx, cancel := e.Ctx()
	defer cancel()
	return e.c.Delete(ctx, model, Spec(model))
}

// Status is the last status the engine reported for a model.
func (e *Adapter) Status(model string) engine.ModelStatus {
	s, _ := e.c.Status(model)
	return s
}

// WaitReady waits until the model is ready, failed, or the deadline.
func (e *Adapter) WaitReady(model string, deadline time.Time) engine.ModelStatus {
	for {
		s := e.Status(model)
		if s.State == engine.StateReady || s.State == engine.StateFailed || !time.Now().Before(deadline) {
			return s
		}
		time.Sleep(50 * time.Millisecond)
	}
}

// Load starts loading a model.
func (e *Adapter) Load(model string) error {
	ctx, cancel := e.Ctx()
	defer cancel()
	_, err := e.c.Load(ctx, model, Spec(model))
	return err
}

// PreviewReady is true when the live preview model is in memory.
func (e *Adapter) PreviewReady(model string) bool {
	s := e.Status(model)
	return s.State == engine.StateReady || s.Preview
}

// PushAudio sends audio to a stream.
func (e *Adapter) PushAudio(session uint64, samples []float32) error {
	return e.c.StreamAudio(session, samples)
}

// EndStream ends a stream without waiting.
func (e *Adapter) EndStream(session uint64) {
	go func() {
		ctx, cancel := e.Ctx()
		defer cancel()
		e.c.StreamEnd(ctx, session)
	}()
}

// StartState is how far the engine got.
func (e *Adapter) StartState() bridge.EngineStart { return bridge.EngineStart(e.state.Load()) }

// Features are the engine's features from hello.
func (e *Adapter) Features() []string { return e.c.Features().Features }

// Transcribe runs the final pass for a connected app.
func (e *Adapter) Transcribe(samples []float32, model, language string) (string, error) {
	ctx, cancel := e.Ctx()
	defer cancel()
	f, err := e.c.TranscribeSamples(ctx, model, samples, language, nil)
	return f.Text, err
}

// StartStream starts a stream for a connected app.
func (e *Adapter) StartStream(session uint64, model, language string) error {
	ctx, cancel := e.Ctx()
	defer cancel()
	return e.c.StreamStart(ctx, session, model, language)
}

func (e *Adapter) wav(samples []float32) (string, func(), error) {
	f, err := os.CreateTemp(e.tmp, "sayso-host-*.wav")
	if err != nil {
		return "", nil, err
	}
	_, err = f.Write(engine.EncodeWAV(engine.ToPCM16(samples)))
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

// TranscribeSegments transcribes with segment times.
func (e *Adapter) TranscribeSegments(samples []float32, model, language string) ([]bridge.Segment, error) {
	path, done, err := e.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := e.Ctx()
	defer cancel()
	f, err := e.c.Transcribe(ctx, model, path, language, nil, true)
	if err != nil {
		return nil, err
	}
	out := make([]bridge.Segment, len(f.Segments))
	for i, s := range f.Segments {
		out[i] = bridge.Segment{StartMs: s.StartMs, EndMs: s.EndMs, Text: s.Text}
		for _, w := range s.Words {
			out[i].Words = append(out[i].Words, bridge.Word{StartMs: w.StartMs, EndMs: w.EndMs, Text: w.Text})
		}
	}
	return out, nil
}

// DetectSpeech runs a VAD model on samples.
func (e *Adapter) DetectSpeech(model string, samples []float32) ([]bridge.SpeechPart, error) {
	path, done, err := e.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := e.Ctx()
	defer cancel()
	parts, err := e.c.Vad(ctx, model, path)
	if err != nil {
		return nil, err
	}
	out := make([]bridge.SpeechPart, len(parts))
	for i, p := range parts {
		out[i] = bridge.SpeechPart{StartMs: p.StartMs, EndMs: p.EndMs}
	}
	return out, nil
}

// Diarize finds who speaks when.
func (e *Adapter) Diarize(model string, samples []float32, speakers *uint32) ([]bridge.RawTurn, error) {
	path, done, err := e.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := e.Ctx()
	defer cancel()
	var n uint32
	if speakers != nil {
		n = *speakers
	}
	d, err := e.c.Diarize(ctx, model, path, n)
	if err != nil {
		return nil, err
	}
	out := make([]bridge.RawTurn, len(d.Turns))
	for i, t := range d.Turns {
		out[i] = bridge.RawTurn{StartMs: t.StartMs, EndMs: t.EndMs, Speaker: t.Speaker}
	}
	return out, nil
}

// StartVad starts VAD on a stream.
func (e *Adapter) StartVad(session uint64, model string) error {
	ctx, cancel := e.Ctx()
	defer cancel()
	return e.c.VadStart(ctx, session, model)
}

// EndVad ends a VAD stream and waits for its last events.
func (e *Adapter) EndVad(session uint64) error {
	ctx, cancel := e.Ctx()
	defer cancel()
	_, err := e.c.StreamEnd(ctx, session)
	return err
}

// Subscribe gives the engine's events until stop runs.
func (e *Adapter) Subscribe() (<-chan engine.Event, func()) {
	ch := make(chan engine.Event, 64)
	e.mu.Lock()
	id := e.next
	e.next++
	e.subs[id] = ch
	e.mu.Unlock()
	var once sync.Once
	return ch, func() {
		once.Do(func() {
			e.mu.Lock()
			delete(e.subs, id)
			e.mu.Unlock()
			close(ch)
		})
	}
}
