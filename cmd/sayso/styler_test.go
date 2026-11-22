package main

import (
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
)

type fakeLM struct{ calls int }

func (f *fakeLM) ModelName() (string, error) { return "Apple Foundation Model", nil }
func (f *fakeLM) Generate(instructions, prompt string, _ *float32, timeout time.Duration) (enhance.Generated, error) {
	f.calls++
	if instructions == "" || !strings.Contains(prompt, "um hello") || timeout <= 0 {
		return enhance.Generated{}, &enhance.Error{Kind: enhance.ErrCLI, Message: "bad request"}
	}
	return enhance.Generated{Text: "Hello.", Model: "Apple Foundation Model"}, nil
}

func stylerConfig(p config.Provider) *config.Config {
	cfg := config.Default()
	cfg.AI.Enabled = true
	cfg.AI.Providers = []config.Provider{p}
	cfg.AI.DefaultProvider = &p.ID
	return &cfg
}

// The app's style step reaches the engine's language model and the cleanup
// runtime, so Apple Intelligence and cleanup-model styles run in dictation.
func TestTheStylerHasTheLanguageModelAndTheCleanupRuntime(t *testing.T) {
	lib := enhance.LoadStyles(t.TempDir())
	style, _ := lib.Get("rambling")

	apple := config.Provider{ID: "apple", Name: "Apple Intelligence", Kind: config.KindAppleIntelligence}
	cfg := stylerConfig(apple)
	lm := &fakeLM{}
	runtime := engine.NewCleanupClient(filepath.Join(t.TempDir(), "no-cleanup-runtime"))
	modelsDir := t.TempDir()
	s := newStyler(providers.NewMemoryStore(), func() *config.Config { return cfg }, lm, runtime, modelsDir)
	out := s.Run(dictation.StyleJob{Style: style, Provider: apple, Text: "so um hello", BasePrompt: lib.BasePrompt()})
	if out.EnhanceError != nil || out.Text != "Hello." || out.Enhance.Result != store.EnhanceApplied || lm.calls != 1 {
		t.Fatalf("%+v %v", out, out.EnhanceError)
	}
	if out.Timings.TimeoutMs != cfg.AI.HTTPTimeoutMs {
		t.Fatal(out.Timings.TimeoutMs)
	}

	rc, ok := s.Cleaner.(*enhance.RuntimeCleaner)
	if !ok || rc.Runtime != runtime || rc.ModelsDir != modelsDir {
		t.Fatalf("%#v", s.Cleaner)
	}
	if _, ok := s.Cleaner.(dictation.Preloader); !ok {
		t.Fatal("the cleaner cannot preload")
	}
	var _ dictation.Preparer = s

	// A cleanup model goes to the runtime cleaner: here it is not downloaded,
	// which only the cleaner says (the enhancer stub says something else).
	model := "s1-mini"
	cleanup := config.Provider{ID: "cleanup", Name: "S1-mini", Kind: config.KindCleanupModel, Model: &model}
	cfg = stylerConfig(cleanup)
	clean, _ := lib.Get("clean")
	out = s.Run(dictation.StyleJob{Style: clean, Provider: cleanup, Text: "so um hello there", Language: "en"})
	if out.Enhance.Result != store.EnhanceFailed || !strings.Contains(out.Enhance.Reason, "is not downloaded") || runtime.IsRunning() {
		t.Fatalf("%+v", out.Enhance)
	}

	// Without an engine there is no language model, not a typed nil.
	if s := newStyler(providers.NewMemoryStore(), func() *config.Config { return cfg }, nil, nil, ""); s.LanguageModel != nil || s.Cleaner != nil {
		t.Fatalf("%#v", s)
	}
}

func TestCleanupInUse(t *testing.T) {
	model := "s1-mini"
	cleanup := config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &model}
	on := stylerConfig(cleanup)
	off := stylerConfig(config.Provider{ID: "a", Kind: config.KindAppleIntelligence})
	disabled := stylerConfig(cleanup)
	disabled.AI.Enabled = false
	if !cleanupInUse(on) || cleanupInUse(off) || cleanupInUse(disabled) || cleanupInUse(nil) {
		t.Fatal("cleanupInUse")
	}
	runtime := engine.NewCleanupClient(filepath.Join(t.TempDir(), "none"))
	sw := cleanupSwitch(runtime, off)
	sw(on)
	sw(off)
}
