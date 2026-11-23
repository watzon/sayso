package ui

import (
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
)

// routeEnv has AI on with Claude Code (sonnet) the default provider, a
// local OpenAI-compatible server, and the Cleanup 1 model.
func routeEnv(t *testing.T) *testEnv {
	e := newEnv(t)
	e.a.cfg.AI.Enabled = true
	e.a.cfg.AI.Providers = []config.Provider{
		{ID: "claude-code", Name: "Claude Code", Kind: config.KindClaudeCLI, Model: ptr("sonnet")},
		{ID: "ollama", Name: "Ollama", Kind: config.KindOpenAICompatible, BaseURL: "http://localhost:11434/v1", Model: ptr("llama3")},
		{ID: "spokenly-cleanup-1", Name: "Cleanup 1", Kind: config.KindCleanupModel, Model: ptr("spokenly-cleanup-1")},
	}
	e.a.cfg.AI.DefaultProvider = ptr("claude-code")
	return e
}

// cleanStyle is the built-in Clean: a prompt and a [cleanup] table.
func cleanStyle(provider, model *string) *enhance.Style {
	return &enhance.Style{ID: "clean", Name: "Clean", Prompt: "Keep the speaker's own words.",
		Cleanup: enhance.CleanupField{State: enhance.CleanupSettingsSet}, Provider: provider, Model: model}
}

// The detail of a style row names where the style runs, in the color of
// that route (Rust ext.rs style_route, styles.rs row_detail), not the
// default provider's.
func TestStyleRowRoute(t *testing.T) {
	e := routeEnv(t)
	p := e.a.themes.get(e.a.cfg.Appearance.Ink, false).Colors
	list := enhance.CleanupSettings{Layout: ptr("list")}
	cannot := cleanStyle(ptr("spokenly-cleanup-1"), nil)
	cannot.Cleanup.Settings = list
	cases := []struct {
		name  string
		st    *enhance.Style
		line  string
		color mu.Color
	}{
		{"cleanup model", cleanStyle(ptr("spokenly-cleanup-1"), nil),
			i18n.T("styles.route.local_no_model", "provider", "Cleanup 1"), p.Success},
		{"local provider", cleanStyle(ptr("ollama"), nil),
			i18n.T("styles.route.local", "provider", "Ollama", "model", "llama3"), p.Success},
		{"local provider and model", cleanStyle(ptr("ollama"), ptr("qwen3")),
			i18n.T("styles.route.local", "provider", "Ollama", "model", "qwen3"), p.Success},
		{"cloud provider by id", cleanStyle(ptr("claude-code"), ptr("opus")),
			i18n.T("styles.route.cloud", "provider", "Claude Code", "model", "opus"), p.Accent},
		{"model only", cleanStyle(nil, ptr("haiku")),
			i18n.T("styles.route.cloud", "provider", "Claude Code", "model", "haiku"), p.Accent},
		{"provider gone", cleanStyle(ptr("gone"), nil), i18n.T("styles.route.needs_provider"), p.Pencil},
		{"cleanup cannot run", cannot, i18n.T("styles.route.cleanup_cannot_run", "model", "Cleanup 1"), p.Danger},
		{"default provider", cleanStyle(nil, nil), "", p.Graphite},
	}
	for _, c := range cases {
		line, col, target := e.a.styleRowDetail(c.st, p)
		if line != c.line || col != c.color || target {
			t.Errorf("%s: %q in %v (target %v), want %q in %v", c.name, line, col, target, c.line, c.color)
		}
	}
	// The default provider's route, for the lines that name it.
	if r := e.a.styleRoute(cleanStyle(nil, nil)); r.line() != i18n.T("styles.route.cloud", "provider", "Claude Code", "model", "sonnet") {
		t.Errorf("default route %q", r.line())
	}
}

// Chris's case: Clean on Cleanup 1 while Claude Code is the default. The
// Styles page shows Cleanup 1 on Clean's row.
func TestStylesPageCleanupRoute(t *testing.T) {
	e := routeEnv(t)
	st, ok := e.a.styles.Get("clean")
	if !ok {
		t.Fatal("no Clean style")
	}
	st.Provider = ptr("spokenly-cleanup-1")
	e.a.SetPage(PageStyles)
	tt := e.hub(t)
	tt.Frame()
	if !tt.HasText(i18n.T("styles.route.local_no_model", "provider", "Cleanup 1")) {
		t.Errorf("no Cleanup 1 route: %q", tt.Texts())
	}
	if tt.HasText(i18n.T("styles.route.cloud", "provider", "Claude Code", "model", "sonnet")) {
		t.Error("a row says Claude Code")
	}
}
