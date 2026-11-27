package ui

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/engine/bridgeengine"
)

// EngineAdapter is the speech engine as the app uses it: the engine of the
// Host API (bridgeengine.Adapter), with the interfaces of the dictation
// controller (dictation.Engine) and the hub (Engine).
type EngineAdapter struct {
	*bridgeengine.Adapter
}

var _ Engine = (*EngineAdapter)(nil)

// StartEngine starts the engine program and follows its events. Each time the
// engine starts it asks for the state of every local model and then loads the
// model preload gives, when that model is on disk. onEvent sees each event on
// the reader goroutine.
func StartEngine(opts engine.Options, preload func() string, onEvent func(engine.Event)) (*EngineAdapter, error) {
	a, err := bridgeengine.Start(opts, preload, onEvent)
	if err != nil {
		return nil, err
	}
	return &EngineAdapter{a}, nil
}

// PreloadModel is the model to load when the engine starts: the dictation
// model when Preload model is on, else "".
func PreloadModel(cfg *config.Config) string {
	if cfg == nil || !cfg.Dictation.PreloadModel {
		return ""
	}
	return string(cfg.Dictation.Model)
}

// Transcribe runs the final pass of a dictation.
func (e *EngineAdapter) Transcribe(samples []float32, o dictation.SessionOptions) (dictation.Transcript, error) {
	ctx, cancel := e.Ctx()
	defer cancel()
	f, err := e.Client().TranscribeSamples(ctx, string(o.FinalModel), samples, o.Language, o.Vocabulary)
	if err != nil {
		return dictation.Transcript{}, err
	}
	return dictation.Transcript{Text: f.Text, Model: o.FinalModel, ElapsedMs: f.ElapsedMs}, nil
}

// StartStream starts the live preview of a dictation.
func (e *EngineAdapter) StartStream(session uint64, o dictation.SessionOptions) error {
	return e.Adapter.StartStream(session, string(o.PreviewModel), o.Language)
}

// BridgeEngine is the engine for bridge.Services.
func (e *EngineAdapter) BridgeEngine() bridge.Engine { return e.Adapter }

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
