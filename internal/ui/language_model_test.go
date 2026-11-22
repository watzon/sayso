package ui

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
)

type fakeLMEngine struct {
	state bridge.EngineStart
	lm    engine.LanguageModelState
	lmErr error
	gen   engine.Generated
	err   error
	got   struct {
		instructions, prompt string
		timeout              time.Duration
		temperature          *float64
	}
}

func (f *fakeLMEngine) StartState() bridge.EngineStart { return f.state }
func (f *fakeLMEngine) languageModel(context.Context) (engine.LanguageModelState, error) {
	return f.lm, f.lmErr
}
func (f *fakeLMEngine) generate(_ context.Context, instructions, prompt string, timeout time.Duration, temperature *float64) (engine.Generated, error) {
	f.got.instructions, f.got.prompt, f.got.timeout, f.got.temperature = instructions, prompt, timeout, temperature
	return f.gen, f.err
}

func TestEngineLanguageModel(t *testing.T) {
	f := &fakeLMEngine{state: bridge.EngineStarted, lm: engine.LanguageModelState{Available: true, Model: "Apple Foundation Model"}, gen: engine.Generated{Text: "Hi.", Model: "Apple Foundation Model"}}
	m := engineLanguageModel{f}
	if name, err := m.ModelName(); err != nil || name != "Apple Foundation Model" {
		t.Fatal(name, err)
	}
	temp := float32(0.25)
	g, err := m.Generate("instr", "prompt", &temp, 3*time.Second)
	if err != nil || g.Text != "Hi." || g.Model != "Apple Foundation Model" {
		t.Fatal(g, err)
	}
	if f.got.instructions != "instr" || f.got.prompt != "prompt" || f.got.timeout != 3*time.Second || f.got.temperature == nil || *f.got.temperature != 0.25 {
		t.Fatalf("%+v", f.got)
	}
	// The whole enhancer on it.
	r, err := enhance.NewEngineModel("apple", m).Enhance(enhance.Request{SystemPrompt: "s", Transcript: "t", Timeout: time.Second})
	if err != nil || r.Text != "Hi." || r.Mode != "guided_generation" {
		t.Fatal(r, err)
	}

	f.lm = engine.LanguageModelState{Available: false, Message: "Apple Intelligence is off"}
	if _, err := m.ModelName(); !enhance.IsKind(err, enhance.ErrNotConfigured) || err.Error() != "the provider is not set up: Apple Intelligence is off" {
		t.Fatal(err)
	}
	f.lmErr = &engine.Error{Message: "unknown request", Code: "bad_request"}
	if _, err := m.ModelName(); !enhance.IsKind(err, enhance.ErrNotConfigured) {
		t.Fatal(err)
	}

	for code, kind := range map[string]enhance.ErrorKind{
		engine.ReasonTimeout: enhance.ErrTimeout, engine.ReasonUnavailable: enhance.ErrNotConfigured,
		engine.ReasonRefused: enhance.ErrRefused, "other": enhance.ErrCLI,
	} {
		f.err = &engine.Error{Message: "m", Code: code}
		if _, err := m.Generate("i", "p", nil, 2*time.Second); !enhance.IsKind(err, kind) {
			t.Errorf("%s: %v", code, err)
		}
	}
	f.err = &engine.Error{Message: "m", Code: engine.ReasonTimeout}
	if _, err := m.Generate("i", "p", nil, 2*time.Second); err.(*enhance.Error).Ms != 2000 {
		t.Fatal(err)
	}
	f.err = errors.New("pipe closed")
	if _, err := m.Generate("i", "p", nil, time.Second); !enhance.IsKind(err, enhance.ErrCLI) {
		t.Fatal(err)
	}
	f.err, f.gen = nil, engine.Generated{}
	if _, err := m.Generate("i", "p", nil, time.Second); !enhance.IsKind(err, enhance.ErrInvalidOutput) {
		t.Fatal(err)
	}

	f.state = bridge.EngineFailed
	if _, err := m.ModelName(); !enhance.IsKind(err, enhance.ErrNotConfigured) {
		t.Fatal(err)
	}
	if _, err := m.Generate("i", "p", nil, time.Second); !enhance.IsKind(err, enhance.ErrNotConfigured) {
		t.Fatal(err)
	}
}
