package ui

import (
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/store"
)

// A default provider that is gone (deleted, or a hand-edited id) or turned
// off is no provider. Dictation inserts the raw text then, so the pages must
// not name another provider in its place.
func TestNoDefaultProviderShowsNoProvider(t *testing.T) {
	for name, def := range map[string]*string{"dangling id": ptr("deleted"), "no default": nil} {
		e := routeEnv(t)
		e.a.cfg.AI.DefaultProvider = def
		if pr, ok := e.a.defaultProvider(); ok {
			t.Errorf("%s: default provider %q, want none", name, pr.ID)
		}
		if _, ok := e.a.cfg.AI.StyleProvider(nil); ok {
			t.Fatalf("%s: dictation finds a provider", name)
		}
		if r := e.a.styleRoute(cleanStyle(nil, nil)); r.kind != routeNeedsProvider {
			t.Errorf("%s: route %q, want needs provider", name, r.line())
		}
		// A style that names its own provider still runs there.
		if r := e.a.styleRoute(cleanStyle(ptr("ollama"), nil)); r.kind != routeLocal {
			t.Errorf("%s: own provider route %q", name, r.line())
		}
	}
}

type jobStyler struct{ jobs chan dictation.StyleJob }

func (s jobStyler) Run(j dictation.StyleJob) enhance.PipelineOutput {
	s.jobs <- j
	return enhance.PipelineOutput{Text: j.Text}
}

// Enhance again in History runs the default style where dictation would: on
// the style's own provider, not on the default provider.
func TestReEnhanceUsesTheStylesProvider(t *testing.T) {
	e := routeEnv(t)
	st, ok := e.a.styles.Get(e.a.ActiveStyle())
	if !ok {
		t.Fatal("no active style")
	}
	st.Provider = ptr("ollama")
	s := jobStyler{jobs: make(chan dictation.StyleJob, 1)}
	e.a.d.Styler = s
	e.a.reEnhance(store.HistoryEntry{ID: 1, Transcript: "hello"})
	select {
	case j := <-s.jobs:
		if j.Provider.ID != "ollama" {
			t.Fatalf("ran on %q, want ollama", j.Provider.ID)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("no style job")
	}
}

// Home's default style card says where that style runs: on its own provider
// when it names one, as dictation does.
func TestHomeStyleDetailUsesTheStylesProvider(t *testing.T) {
	e := routeEnv(t)
	st := cleanStyle(ptr("ollama"), nil)
	got := e.a.styleDetail(st)
	want := i18n.T("home.style.detail_local", "computer", computer(), "provider", "Ollama", "model", "llama3")
	if !strings.Contains(got, want) {
		t.Fatalf("detail %q, want it to contain %q", got, want)
	}
}
