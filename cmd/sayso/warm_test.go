package main

import (
	"path/filepath"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
)

func TestCleanupWarmModel(t *testing.T) {
	s1, sp := "s1-mini", "spokenly-cleanup-1"
	apple := config.Provider{ID: "a", Kind: config.KindAppleIntelligence}
	cfg := stylerConfig(apple)
	if cleanupWarmModel(cfg) != "" {
		t.Fatal("no cleanup model")
	}
	cfg.AI.Providers = append(cfg.AI.Providers, config.Provider{ID: "c1", Kind: config.KindCleanupModel, Model: &s1},
		config.Provider{ID: "c2", Kind: config.KindCleanupModel, Model: &sp})
	if cleanupWarmModel(cfg) != s1 {
		t.Fatal("the first cleanup model when the default is not one")
	}
	def := "c2"
	cfg.AI.DefaultProvider = &def
	if cleanupWarmModel(cfg) != sp {
		t.Fatal("the default provider first")
	}
	cfg.AI.Enabled = false
	if cleanupWarmModel(cfg) != "" {
		t.Fatal("AI off")
	}
}

// The model loads ahead at the start and when it changes, once each time,
// and after the runtime is switched on.
func TestCleanupWarmerLoadsAheadAfterTheSwitch(t *testing.T) {
	s1, sp := "s1-mini", "spokenly-cleanup-1"
	runtime := engine.NewCleanupClient(filepath.Join(t.TempDir(), "none"))
	var mu sync.Mutex
	var got []string
	preload := func(m string) {
		mu.Lock()
		defer mu.Unlock()
		got = append(got, m)
	}
	off := stylerConfig(config.Provider{ID: "a", Kind: config.KindAppleIntelligence})
	onS1 := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &s1})
	onSp := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &sp})
	sw := cleanupSwitch(runtime, onS1, cleanupWarmer(preload))
	sw(onS1) // no change
	sw(onSp) // another model
	sw(off)
	sw(onS1) // on again: after the switch, off the main goroutine
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		mu.Lock()
		n := len(got)
		mu.Unlock()
		if n == 3 {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	mu.Lock()
	defer mu.Unlock()
	if !slices.Equal(got, []string{s1, sp, s1}) {
		t.Fatal(got)
	}
}
