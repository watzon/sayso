package ui

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/models"
)

// EngineAdapter is the speech engine as the app uses it: it keeps the
// engine.Client, follows its start and crash events, and gives the
// interfaces of the dictation controller (dictation.Engine), the hub
// (Engine) and the Host API (bridge.Engine).
type EngineAdapter struct {
	c       *engine.Client
	tmp     string
	state   atomic.Int32 // bridge.EngineStart
	timeout time.Duration

	mu   sync.Mutex
	subs map[int]chan engine.Event
	next int
	// OnEvent sees every event (on the reader goroutine).
	onEvent func(engine.Event)
	// preload gives the model to load once the engine starts ("" loads none).
	preload func() string
}

var (
	_ Engine        = (*EngineAdapter)(nil)
	_ bridge.Engine = bridgeEngine{}
)

// StartEngine starts the engine program and follows its events. Each time the
// engine starts it asks for the state of every local model and then loads the
// model preload gives, when that model is on disk. onEvent sees each event on
// the reader goroutine.
func StartEngine(opts engine.Options, preload func() string, onEvent func(engine.Event)) (*EngineAdapter, error) {
	c, events, err := engine.Start(opts)
	if err != nil {
		return nil, err
	}
	tmp := opts.CacheDir
	if tmp == "" {
		tmp = os.TempDir()
	}
	e := &EngineAdapter{c: c, tmp: tmp, timeout: 2 * time.Minute, subs: map[int]chan engine.Event{}, onEvent: onEvent, preload: preload}
	go e.follow(events)
	return e, nil
}

func (e *EngineAdapter) follow(events <-chan engine.Event) {
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
		refs = append(refs, engine.ModelRef{ID: string(m.ID), Engine: engineSpec(string(m.ID))})
	}
	return refs
}

// PreloadModel is the model to load when the engine starts: the dictation
// model when Preload model is on, else "".
func PreloadModel(cfg *config.Config) string {
	if cfg == nil || !cfg.Dictation.PreloadModel {
		return ""
	}
	return string(cfg.Dictation.Model)
}

// syncModels asks the engine which models are on disk (their model_state
// events fill Status), then loads the preload model when it is on disk.
func (e *EngineAdapter) syncModels() {
	ctx, cancel := e.ctx()
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

// Close stops the engine.
func (e *EngineAdapter) Close() error { return e.c.Close() }

func (e *EngineAdapter) ctx() (context.Context, context.CancelFunc) {
	return context.WithTimeout(context.Background(), e.timeout)
}

// engineSpec is the engine field of a model, from the catalog.
func engineSpec(model string) json.RawMessage {
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

// ---- Engine (the hub) ----

// Running is true once the engine answered hello.
func (e *EngineAdapter) Running() bool {
	return bridge.EngineStart(e.state.Load()) == bridge.EngineStarted
}

// Download downloads a model and blocks until it is on disk.
func (e *EngineAdapter) Download(model string) error {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Hour)
	defer cancel()
	var size uint64
	if m, ok := models.Find(models.ModelID(model)); ok {
		size = m.SizeBytes
	}
	return e.c.Download(ctx, model, engineSpec(model), size)
}

// Delete deletes a model's files.
func (e *EngineAdapter) Delete(model string) error {
	ctx, cancel := e.ctx()
	defer cancel()
	return e.c.Delete(ctx, model, engineSpec(model))
}

// ---- dictation.Engine ----

// Status is the last status the engine reported for a model.
func (e *EngineAdapter) Status(model string) engine.ModelStatus {
	s, _ := e.c.Status(model)
	return s
}

// WaitReady waits until the model is ready, failed, or the deadline.
func (e *EngineAdapter) WaitReady(model string, deadline time.Time) engine.ModelStatus {
	for {
		s := e.Status(model)
		if s.State == engine.StateReady || s.State == engine.StateFailed || !time.Now().Before(deadline) {
			return s
		}
		time.Sleep(50 * time.Millisecond)
	}
}

// Load starts loading a model.
func (e *EngineAdapter) Load(model string) error {
	ctx, cancel := e.ctx()
	defer cancel()
	_, err := e.c.Load(ctx, model, engineSpec(model))
	return err
}

// Transcribe runs the final pass.
func (e *EngineAdapter) Transcribe(samples []float32, o dictation.SessionOptions) (dictation.Transcript, error) {
	ctx, cancel := e.ctx()
	defer cancel()
	f, err := e.c.TranscribeSamples(ctx, string(o.FinalModel), samples, o.Language, o.Vocabulary)
	if err != nil {
		return dictation.Transcript{}, err
	}
	return dictation.Transcript{Text: f.Text, Model: o.FinalModel, ElapsedMs: f.ElapsedMs}, nil
}

// PreviewReady is true when the live preview model is in memory.
func (e *EngineAdapter) PreviewReady(model string) bool {
	s := e.Status(model)
	return s.State == engine.StateReady || s.Preview
}

// StartStream starts the live preview of a session.
func (e *EngineAdapter) StartStream(session uint64, o dictation.SessionOptions) error {
	ctx, cancel := e.ctx()
	defer cancel()
	return e.c.StreamStart(ctx, session, string(o.PreviewModel), o.Language)
}

// PushAudio sends audio to a stream.
func (e *EngineAdapter) PushAudio(session uint64, samples []float32) error {
	return e.c.StreamAudio(session, samples)
}

// EndStream ends a stream without waiting.
func (e *EngineAdapter) EndStream(session uint64) {
	go func() {
		ctx, cancel := e.ctx()
		defer cancel()
		e.c.StreamEnd(ctx, session)
	}()
}

// ---- bridge.Engine (the Host API) ----

// StartState is how far the engine got.
func (e *EngineAdapter) StartState() bridge.EngineStart { return bridge.EngineStart(e.state.Load()) }

// Features are the engine's features from hello.
func (e *EngineAdapter) Features() []string { return e.c.Features().Features }

// TranscribeFor transcribes for a connected app (bridge.Engine.Transcribe).
func (e *EngineAdapter) transcribeText(samples []float32, model, language string) (string, error) {
	ctx, cancel := e.ctx()
	defer cancel()
	f, err := e.c.TranscribeSamples(ctx, model, samples, language, nil)
	return f.Text, err
}

// bridgeEngine gives the Host API its view of the engine; it differs from
// dictation.Engine only in Transcribe's signature.
type bridgeEngine struct{ *EngineAdapter }

// BridgeEngine is the engine for bridge.Services.
func (e *EngineAdapter) BridgeEngine() bridge.Engine { return bridgeEngine{e} }

func (b bridgeEngine) Transcribe(samples []float32, model, language string) (string, error) {
	return b.transcribeText(samples, model, language)
}

func (b bridgeEngine) StartStream(session uint64, model, language string) error {
	ctx, cancel := b.ctx()
	defer cancel()
	return b.c.StreamStart(ctx, session, model, language)
}

func (e *EngineAdapter) wav(samples []float32) (string, func(), error) {
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
func (e *EngineAdapter) TranscribeSegments(samples []float32, model, language string) ([]bridge.Segment, error) {
	path, done, err := e.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := e.ctx()
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
func (e *EngineAdapter) DetectSpeech(model string, samples []float32) ([]bridge.SpeechPart, error) {
	path, done, err := e.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := e.ctx()
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
func (e *EngineAdapter) Diarize(model string, samples []float32, speakers *uint32) ([]bridge.RawTurn, error) {
	path, done, err := e.wav(samples)
	if err != nil {
		return nil, err
	}
	defer done()
	ctx, cancel := e.ctx()
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
func (e *EngineAdapter) StartVad(session uint64, model string) error {
	ctx, cancel := e.ctx()
	defer cancel()
	return e.c.VadStart(ctx, session, model)
}

// EndVad ends a VAD stream and waits for its last events.
func (e *EngineAdapter) EndVad(session uint64) error {
	ctx, cancel := e.ctx()
	defer cancel()
	_, err := e.c.StreamEnd(ctx, session)
	return err
}

// Subscribe gives the engine's events until stop runs.
func (e *EngineAdapter) Subscribe() (<-chan engine.Event, func()) {
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

// FindEngine is the engine program: the config's path, else the one next
// to this executable, else engine.DefaultEnginePath.
func FindEngine(configured string) (string, error) {
	if configured != "" {
		if _, err := os.Stat(configured); err != nil {
			return "", fmt.Errorf("engine %s: %w", configured, err)
		}
		return configured, nil
	}
	if exe, err := os.Executable(); err == nil {
		p := filepath.Join(filepath.Dir(exe), engine.EngineProgram())
		if _, err := os.Stat(p); err == nil {
			return p, nil
		}
	}
	p := engine.DefaultEnginePath()
	if _, err := os.Stat(p); err != nil {
		return "", errors.New("no speech engine is installed")
	}
	return p, nil
}
