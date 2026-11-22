package main

import (
	"errors"
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
	preload := func(m string) error {
		mu.Lock()
		defer mu.Unlock()
		got = append(got, m)
		return nil
	}
	off := stylerConfig(config.Provider{ID: "a", Kind: config.KindAppleIntelligence})
	onS1 := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &s1})
	onSp := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &sp})
	waitFor := func(what string, ok func() bool) {
		t.Helper()
		deadline := time.Now().Add(2 * time.Second)
		for !ok() {
			if time.Now().After(deadline) {
				t.Fatal("timed out waiting for", what)
			}
			time.Sleep(5 * time.Millisecond)
		}
	}
	warms := func(n int) func() bool {
		return func() bool {
			mu.Lock()
			defer mu.Unlock()
			return len(got) >= n
		}
	}
	disabled := func() bool {
		return errors.Is(runtime.EnsureLoaded("x", "/none", 1), engine.ErrCleanupDisabled)
	}
	sw := cleanupSwitch(runtime, onS1, cleanupWarmer(preload))
	waitFor("the first warm", warms(1))
	sw(onS1) // no change
	sw(onSp) // another model
	waitFor("the second warm", warms(2))
	sw(off)
	waitFor("the switch off", disabled)
	sw(onS1) // on again: after the switch, off the main goroutine
	waitFor("the third warm", warms(3))
	time.Sleep(30 * time.Millisecond)
	mu.Lock()
	defer mu.Unlock()
	if !slices.Equal(got, []string{s1, sp, s1}) {
		t.Fatal(got)
	}
	if disabled() {
		t.Fatal("the runtime is still off")
	}
}

// The newest config decides: configs that a newer one replaced before they
// ran are skipped, so the runtime never ends off when the last config is on.
func TestCleanupSwitchEndsWithTheNewestConfig(t *testing.T) {
	s1 := "s1-mini"
	runtime := engine.NewCleanupClient(filepath.Join(t.TempDir(), "none"))
	off := stylerConfig(config.Provider{ID: "a", Kind: config.KindAppleIntelligence})
	on := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &s1})
	sw := cleanupSwitch(runtime, on)
	for range 50 {
		sw(off)
		sw(on)
	}
	time.Sleep(200 * time.Millisecond)
	if errors.Is(runtime.EnsureLoaded("x", "/none", 1), engine.ErrCleanupDisabled) {
		t.Fatal("the runtime ended off; the newest config is on")
	}
}

// Configs that change while a warm runs end with the last model wanted: the
// ones in between are skipped, never loaded after it.
func TestCleanupWarmerLoadsTheLastModelWanted(t *testing.T) {
	s1, sp := "s1-mini", "spokenly-cleanup-1"
	cfgS1 := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &s1})
	cfgSp := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &sp})
	release := make(chan struct{})
	started := make(chan struct{}, 1)
	var mu sync.Mutex
	var got []string
	warm := cleanupWarmer(func(m string) error {
		mu.Lock()
		got = append(got, m)
		first := len(got) == 1
		mu.Unlock()
		if first {
			started <- struct{}{}
			<-release
		}
		return nil
	})
	warm(cfgSp)
	<-started
	warm(cfgS1)
	warm(cfgSp)
	warm(cfgS1)
	close(release)
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		mu.Lock()
		n := len(got)
		mu.Unlock()
		if n == 2 {
			break
		}
		time.Sleep(5 * time.Millisecond)
	}
	time.Sleep(50 * time.Millisecond)
	mu.Lock()
	defer mu.Unlock()
	if !slices.Equal(got, []string{sp, s1}) {
		t.Fatal(got)
	}
}

// A failed warm (the model was not downloaded yet) is not remembered: the next
// config with the same model tries again, and a warm that worked is not
// repeated.
func TestCleanupWarmerRetriesAfterAFailure(t *testing.T) {
	s1 := "s1-mini"
	cfg := stylerConfig(config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &s1})
	calls := make(chan string, 4)
	fail := true
	var mu sync.Mutex
	warm := cleanupWarmer(func(m string) error {
		mu.Lock()
		defer mu.Unlock()
		calls <- m
		if fail {
			fail = false
			return errors.New("s1-mini is not downloaded")
		}
		return nil
	})
	wait := func() {
		t.Helper()
		select {
		case <-calls:
		case <-time.After(2 * time.Second):
			t.Fatal("no warm")
		}
	}
	warm(cfg)
	wait()
	// The goroutine forgets the model after the failure; wait for that.
	deadline := time.Now().Add(2 * time.Second)
	for {
		warm(cfg)
		select {
		case <-calls:
		case <-time.After(20 * time.Millisecond):
			if time.Now().Before(deadline) {
				continue
			}
			t.Fatal("a failed warm was not tried again")
		}
		break
	}
	warm(cfg)
	select {
	case m := <-calls:
		t.Fatal("a warm that worked ran again:", m)
	case <-time.After(100 * time.Millisecond):
	}
}
