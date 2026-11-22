package main

import (
	"sync"

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
// main goroutine; the runtime may be busy, so the change runs off it. Each
// warm function runs after the switch, at the start and at each change.
func cleanupSwitch(runtime *engine.CleanupClient, start *config.Config, warm ...func(*config.Config)) func(*config.Config) {
	on := cleanupInUse(start)
	runtime.SetEnabled(on)
	for _, w := range warm {
		w(start)
	}
	return func(c *config.Config) {
		if now := cleanupInUse(c); now != on {
			on = now
			go func() {
				runtime.SetEnabled(now)
				for _, w := range warm {
					w(c)
				}
			}()
			return
		}
		for _, w := range warm {
			w(c)
		}
	}
}

// cleanupWarmModel is the cleanup model to load before the first dictation:
// the default provider when it is a cleanup model, else the first cleanup
// model provider. "" when no cleanup model can run.
func cleanupWarmModel(c *config.Config) string {
	if !cleanupInUse(c) {
		return ""
	}
	if p, ok := c.AI.Provider(""); ok && p.Kind == config.KindCleanupModel && p.Model != nil {
		return *p.Model
	}
	for _, p := range c.AI.Providers {
		if p.Kind == config.KindCleanupModel && p.Model != nil {
			return *p.Model
		}
	}
	return ""
}

// cleanupWarmer loads the cleanup model ahead, in the background, when it
// becomes the model to warm, so the first dictation does not wait for the
// load (on a Mac the first load compiles the Metal shaders, about 5 s).
// preload must not block; RuntimeCleaner.Preload loads in a goroutine and
// only a downloaded model.
func cleanupWarmer(preload func(model string)) func(*config.Config) {
	var mu sync.Mutex
	last := ""
	return func(c *config.Config) {
		m := cleanupWarmModel(c)
		mu.Lock()
		changed := m != last
		last = m
		mu.Unlock()
		if changed && m != "" {
			preload(m)
		}
	}
}
