package ui

import (
	"context"
	"errors"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
)

// LanguageModel is the engine's language model (Apple Intelligence) for the
// Apple Intelligence provider (sayso-engine-client's LanguageModel for EngineClient).
func (e *EngineAdapter) LanguageModel() enhance.LanguageModel { return engineLanguageModel{e} }

// languageModelEngine is what engineLanguageModel needs of the engine.
type languageModelEngine interface {
	StartState() bridge.EngineStart
	languageModel(ctx context.Context) (engine.LanguageModelState, error)
	generate(ctx context.Context, instructions, prompt string, timeout time.Duration, temperature *float64) (engine.Generated, error)
}

func (e *EngineAdapter) languageModel(ctx context.Context) (engine.LanguageModelState, error) {
	return e.Client().LanguageModel(ctx)
}

func (e *EngineAdapter) generate(ctx context.Context, instructions, prompt string, timeout time.Duration, temperature *float64) (engine.Generated, error) {
	return e.Client().Generate(ctx, instructions, prompt, timeout, temperature)
}

type engineLanguageModel struct{ e languageModelEngine }

// languageModelRequestTimeout bounds the language_model question.
const languageModelRequestTimeout = 10 * time.Second

// errEngineDown is the error while the engine failed to start, as when the
// Rust app drops its language model on StartFailed.
func errEngineDown() error {
	return &enhance.Error{Kind: enhance.ErrNotConfigured, Message: "the engine does not run"}
}

func (m engineLanguageModel) ModelName() (string, error) {
	if m.e.StartState() == bridge.EngineFailed {
		return "", errEngineDown()
	}
	ctx, cancel := context.WithTimeout(context.Background(), languageModelRequestTimeout)
	defer cancel()
	s, err := m.e.languageModel(ctx)
	if err != nil {
		// An engine without the request answers an error: it has no language model.
		return "", &enhance.Error{Kind: enhance.ErrNotConfigured, Message: engineMessage(err)}
	}
	if !s.Available {
		msg := s.Message
		if msg == "" {
			msg = "the language model cannot run"
		}
		return "", &enhance.Error{Kind: enhance.ErrNotConfigured, Message: msg}
	}
	if s.Model == "" {
		return "language model", nil
	}
	return s.Model, nil
}

func (m engineLanguageModel) Generate(instructions, prompt string, temperature *float32, timeout time.Duration) (enhance.Generated, error) {
	if m.e.StartState() == bridge.EngineFailed {
		return enhance.Generated{}, errEngineDown()
	}
	var temp *float64
	if temperature != nil {
		t := float64(*temperature)
		temp = &t
	}
	g, err := m.e.generate(context.Background(), instructions, prompt, timeout, temp)
	if err != nil {
		return enhance.Generated{}, generateError(err, timeout)
	}
	if g.Text == "" {
		return enhance.Generated{}, &enhance.Error{Kind: enhance.ErrInvalidOutput, Message: "the engine sent no text"}
	}
	model := g.Model
	if model == "" {
		model = "language model"
	}
	return enhance.Generated{Text: g.Text, Model: model}, nil
}

// generateError maps an engine error by its code, as into_enhance_error does.
func generateError(err error, timeout time.Duration) error {
	var ee *engine.Error
	if errors.As(err, &ee) {
		switch ee.Code {
		case engine.ReasonTimeout:
			return &enhance.Error{Kind: enhance.ErrTimeout, Ms: uint64(timeout.Milliseconds())}
		case engine.ReasonUnavailable:
			return &enhance.Error{Kind: enhance.ErrNotConfigured, Message: ee.Message}
		case engine.ReasonRefused:
			return &enhance.Error{Kind: enhance.ErrRefused, Message: ee.Message}
		}
		return &enhance.Error{Kind: enhance.ErrCLI, Message: ee.Message}
	}
	return &enhance.Error{Kind: enhance.ErrCLI, Message: err.Error()}
}

func engineMessage(err error) string {
	var ee *engine.Error
	if errors.As(err, &ee) {
		return ee.Message
	}
	return err.Error()
}
