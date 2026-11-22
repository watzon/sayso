package main

import (
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/providers"
)

// cleanupInUse is true when a cleanup model can run: AI is on and a provider
// is a cleanup model. Nothing loads in the runtime otherwise.
func cleanupInUse(c *config.Config) bool {
	if c == nil || !c.AI.Enabled {
		return false
	}
	for _, p := range c.AI.Providers {
		if p.Kind == config.KindCleanupModel {
			return true
		}
	}
	return false
}

// newStyler is the style step of the app, shared by dictation and History:
// the engine's language model for Apple Intelligence (nil without an engine)
// and the cleanup runtime for cleanup models (sayso-app's Services).
func newStyler(secrets providers.SecretStore, cfg func() *config.Config, lm enhance.LanguageModel, runtime *engine.CleanupClient, modelsDir string) *dictation.DefaultStyler {
	s := &dictation.DefaultStyler{Secrets: secrets, Config: cfg, LanguageModel: lm}
	if runtime != nil {
		s.Cleaner = &enhance.RuntimeCleaner{Runtime: runtime, ModelsDir: modelsDir}
	}
	return s
}

// cleanupSwitch turns the cleanup runtime on and off as the config changes,
// so its memory is freed when no cleanup model can run. It is called on the
// main goroutine; the runtime may be busy, so the change runs off it.
func cleanupSwitch(runtime *engine.CleanupClient, start *config.Config) func(*config.Config) {
	on := cleanupInUse(start)
	runtime.SetEnabled(on)
	return func(c *config.Config) {
		if now := cleanupInUse(c); now != on {
			on = now
			go runtime.SetEnabled(now)
		}
	}
}
