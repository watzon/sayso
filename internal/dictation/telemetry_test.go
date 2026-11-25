package dictation

import (
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/telemetry"
)

type fakeTelemetry struct {
	mu   sync.Mutex
	sent []telemetry.Dictation
}

func (f *fakeTelemetry) DictationCompleted(d telemetry.Dictation) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.sent = append(f.sent, d)
}

func (f *fakeTelemetry) signals() []telemetry.Dictation {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]telemetry.Dictation(nil), f.sent...)
}

func dictate(t *testing.T, r *rig, end string) {
	t.Helper()
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.audio.speak(16000)
	r.c.Toggle()
	r.obs.waitFor(t, end)
	time.Sleep(20 * time.Millisecond)
}

func TestAnInsertedDictationIsCountedWithoutItsWords(t *testing.T) {
	r := newRig(t, nil)
	tel := &fakeTelemetry{}
	r.c.d.Telemetry = tel
	dictate(t, r, "done")
	got := tel.signals()
	want := telemetry.Dictation{ModelID: string(r.cfg.Dictation.Model), ModelKind: telemetry.ModelLocal, StyleKind: telemetry.StyleRaw}
	if len(got) != 1 || got[0] != want {
		t.Fatalf("signals %+v, want %+v", got, want)
	}
	for _, v := range []string{got[0].ModelID, got[0].ModelKind, got[0].StyleKind} {
		if strings.Contains(strings.ToLower(v), "hello") || strings.Contains(strings.ToLower(v), "world") {
			t.Fatalf("the transcript is in %+v", got[0])
		}
	}
}

func TestIncognitoAndFailedInsertsAreNotCounted(t *testing.T) {
	r := newRig(t, nil)
	tel := &fakeTelemetry{}
	r.c.d.Telemetry = tel
	r.incog = true
	dictate(t, r, "done")
	if got := tel.signals(); len(got) != 0 {
		t.Fatalf("incognito counted: %+v", got)
	}

	r = newRig(t, nil)
	r.c.d.Telemetry = tel
	r.ins.fail = true
	dictate(t, r, "failed")
	if got := tel.signals(); len(got) != 0 {
		t.Fatalf("a failed insert counted: %+v", got)
	}
}

func TestTelemetryDictationNamesOnlyPublicModels(t *testing.T) {
	local := "http://localhost:8080/v1"
	cfg := config.Default()
	cfg.Speech.Providers = []config.SpeechProvider{
		{ID: "my-groq", Name: "Work account", Kind: config.SpeechGroq},
		{ID: "home", Name: "Home server", Kind: config.SpeechOpenAICompatible, BaseURL: &local},
	}
	cfg.AI.Providers = []config.Provider{
		{ID: "claude", Name: "Claude", Kind: config.KindClaudeCLI},
		{ID: "apple", Name: "Apple", Kind: config.KindAppleIntelligence},
		{ID: "ollama", Name: "Ollama", Kind: config.KindOpenAICompatible, BaseURL: "http://127.0.0.1:11434/v1"},
	}
	applied := func(id string) *store.EnhanceOutcome {
		return &store.EnhanceOutcome{Result: store.EnhanceApplied, ProviderID: id}
	}
	catalog := models.DefaultModel()
	for _, tc := range []struct {
		model models.ModelID
		e     *store.EnhanceOutcome
		want  telemetry.Dictation
	}{
		{catalog, nil, telemetry.Dictation{ModelID: string(catalog), ModelKind: "local", StyleKind: "raw"}},
		{"my-groq:whisper-large-v3", applied("claude"), telemetry.Dictation{ModelID: "groq:whisper-large-v3", ModelKind: "cloud", StyleKind: "ai_cloud"}},
		{"my-groq:acme-finetune-for-jane", applied("apple"), telemetry.Dictation{ModelID: "groq", ModelKind: "cloud", StyleKind: "ai_local"}},
		{"home:whisper-medical", applied("ollama"), telemetry.Dictation{ModelID: "open_ai_compatible", ModelKind: "local", StyleKind: "ai_local"}},
		{"gone:whisper-1", nil, telemetry.Dictation{ModelID: "other", ModelKind: "cloud", StyleKind: "raw"}},
		{"/Users/jane/models/mine.bin", &store.EnhanceOutcome{Result: store.EnhanceFailed, ProviderID: "claude"}, telemetry.Dictation{ModelID: "other", ModelKind: "local", StyleKind: "raw"}},
	} {
		if got := TelemetryDictation(&cfg, tc.model, tc.e); got != tc.want {
			t.Errorf("%s: %+v, want %+v", tc.model, got, tc.want)
		}
	}
}
