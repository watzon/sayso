package ui

import (
	"errors"
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/providers"
)

// fakeLister answers the model picker's fetches from canned lists and
// records each call.
type fakeLister struct {
	mu    sync.Mutex
	calls []string
	lists map[string][]enhance.ModelChoice
	errs  map[string]error
	// gate, when set, holds every fetch until it is closed.
	gate chan struct{}
}

func (f *fakeLister) list(p config.Provider, secrets providers.SecretStore, _ enhance.LanguageModel, _ time.Duration) ([]enhance.ModelChoice, error) {
	if f.gate != nil {
		<-f.gate
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls = append(f.calls, p.ID)
	if err := f.errs[p.ID]; err != nil {
		return nil, err
	}
	return f.lists[p.ID], nil
}

func (f *fakeLister) count() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.calls)
}

func desc(s string) *string { return &s }

var claudeModels = []enhance.ModelChoice{
	{ID: "opus", Name: "Opus 5.5", Description: desc("For complex work and everyday tasks"), Thinking: []string{"low", "medium", "high", "xhigh", "max"}},
	{ID: "sonnet", Name: "Sonnet 5", Thinking: []string{"low", "medium", "high"}},
	{ID: "haiku", Name: "Haiku 4.5", Description: desc("Fastest for quick answers")},
}

func withLister(e *testEnv, f *fakeLister) *fakeLister {
	e.a.d.ListModels = f.list
	return f
}

// pressPrefix clicks the first control whose name starts with prefix.
func pressPrefix(t *testing.T, tt *mu.Tester, prefix string) {
	t.Helper()
	for _, a := range tt.AccessAll() {
		if strings.HasPrefix(a.Name, prefix) && a.Role != mu.RoleText && a.Role != mu.RoleNone {
			b := a.Bounds
			tt.ClickAt(b.X+b.W/2, b.Y+b.H/2)
			tt.Frame()
			return
		}
	}
	t.Fatalf("no control named %q…; texts %q", prefix, tt.Texts())
}

// optionOn is true when the row name shows and is the chosen one.
func optionOn(tt *mu.Tester, name string) bool {
	o, ok := tt.AccessByRole(mu.RoleOption, name)
	return ok && (o.Selected || o.Checked)
}

func hasOption(tt *mu.Tester, name string) bool {
	_, ok := tt.AccessByRole(mu.RoleOption, name)
	return ok
}

func trigger(label string) string { return i18n.T("model_picker.trigger_label", "model", label) }

func claudeEnv(t *testing.T) (*testEnv, *fakeLister) {
	e := newEnv(t)
	f := withLister(e, &fakeLister{lists: map[string][]enhance.ModelChoice{"claude-code": claudeModels}})
	e.a.cfg.AI.Providers = []config.Provider{{ID: "claude-code", Name: "Claude Code", Kind: config.KindClaudeCLI, Model: ptr("sonnet")}}
	e.a.cfg.AI.DefaultProvider = ptr("claude-code")
	e.a.OpenSettings(SecProviders)
	return e, f
}

func provider(e *testEnv, id string) config.Provider {
	pr, _ := e.a.cfg.AI.Provider(id)
	return pr
}

// Settings › Providers: the Claude CLI's model is a picker over the list
// the CLI reports. A pick saves the model, fits the thinking level to its
// levels, and closes the sheet; the Thinking row follows the picked model.
func TestClaudeModelPickerPicksAndFitsThinking(t *testing.T) {
	e, f := claudeEnv(t)
	e.a.cfg.AI.Providers[0].Thinking = ptr("high")
	tt := e.hub(t)
	tt.Frame()
	// Before the list is here, the trigger shows the id and the default level.
	if _, ok := tt.Access(trigger("sonnet · High")); !ok {
		t.Fatalf("no trigger: %q", tt.Texts())
	}
	press(t, tt, trigger("sonnet · High"))
	tt.Frame()
	if f.count() != 1 || e.a.pick.open != "providers.model.claude-code" {
		t.Fatalf("calls %v, open %q", f.calls, e.a.pick.open)
	}
	for _, name := range []string{"Opus 5.5", "Sonnet 5", "Haiku 4.5"} {
		if !hasOption(tt, name) {
			t.Fatalf("no row %q: %q", name, tt.Texts())
		}
	}
	if !tt.HasText(strings.ToUpper(i18n.TP("model_picker.count", 3, map[string]any{"count": 3}))) || !tt.HasText("Fastest for quick answers") {
		t.Errorf("no count or note: %q", tt.Texts())
	}
	if !optionOn(tt, "Sonnet 5") || optionOn(tt, "Opus 5.5") {
		t.Error("the chosen model is not marked")
	}
	press(t, tt, "Opus 5.5")
	tt.Frame()
	if pr := provider(e, "claude-code"); pr.ModelName() != "opus" || pr.ThinkingSent() != "high" || e.a.pick.open != "" {
		t.Fatalf("model %q thinking %q open %q", pr.ModelName(), pr.ThinkingSent(), e.a.pick.open)
	}
	// The trigger has the provider's name now, and Thinking the levels of Opus.
	if _, ok := tt.Access(trigger("Opus 5.5 · High")); !ok || !tt.HasText("Max") {
		t.Fatalf("after the pick: %q", tt.Texts())
	}
	// Haiku has no levels: the level goes, and so does the Thinking row.
	press(t, tt, trigger("Opus 5.5 · High"))
	tt.Frame()
	press(t, tt, "Haiku 4.5")
	tt.Frame()
	if pr := provider(e, "claude-code"); pr.ModelName() != "haiku" || pr.ThinkingChosen() != "" {
		t.Fatalf("model %q thinking %q", pr.ModelName(), pr.ThinkingChosen())
	}
	if len(e.a.thinkingChoices(provider(e, "claude-code"))) != 0 || slices.Contains(tt.Texts(), "Medium") {
		t.Errorf("a model without levels still shows Thinking: %q", tt.Texts())
	}
	// One fetch served every open.
	if f.count() != 1 {
		t.Errorf("fetched %d times", f.count())
	}
}

// The thinking level in the sheet is the pen line, a slider with a stop
// for each level of the chosen model. It leaves the sheet open.
func TestModelPickerThinkingLine(t *testing.T) {
	e, _ := claudeEnv(t)
	e.a.cfg.AI.Providers[0].Model = ptr("opus")
	tt := e.hub(t)
	tt.Frame()
	press(t, tt, trigger("opus · Low"))
	tt.Frame()
	s, ok := tt.AccessByRole(mu.RoleSlider, i18n.T("model_picker.thinking"))
	if !ok || s.Value != "Low" {
		t.Fatalf("no thinking line: %+v %q", s, tt.Texts())
	}
	// The right end is the most thinking.
	tt.ClickAt(s.Bounds.X+s.Bounds.W-2, s.Bounds.Y+s.Bounds.H/2)
	tt.Frame()
	if pr := provider(e, "claude-code"); pr.ThinkingChosen() != "max" || e.a.pick.open == "" {
		t.Fatalf("thinking %q open %q", pr.ThinkingChosen(), e.a.pick.open)
	}
	// The arrows move it one stop.
	tt.Key(0, mu.KeyLeft)
	tt.Frame()
	if pr := provider(e, "claude-code"); pr.ThinkingChosen() != "xhigh" {
		t.Fatalf("thinking %q after Left", pr.ThinkingChosen())
	}
	if !tt.HasText(config.ThinkingLabel("xhigh")) {
		t.Errorf("the name does not follow: %q", tt.Texts())
	}
}

func serverEnv(t *testing.T) (*testEnv, *fakeLister) { return serverEnvPosting(t, nil) }

// serverEnvPosting is serverEnv with post as Deps.Post, set before
// Settings starts its own goroutines.
func serverEnvPosting(t *testing.T, post func(func())) (*testEnv, *fakeLister) {
	e := newEnv(t)
	e.a.d.Post = post
	f := withLister(e, &fakeLister{lists: map[string][]enhance.ModelChoice{"my-server": {
		{ID: "llama3.2", Name: "llama3.2"}, {ID: "qwen2.5", Name: "qwen2.5"}, {ID: "openai/gpt-5", Name: "GPT-5"},
	}}})
	e.a.cfg.AI.Providers = []config.Provider{{ID: "my-server", Name: "My Server", Kind: config.KindOpenAICompatible, BaseURL: "http://localhost:8080/v1"}}
	e.a.OpenSettings(SecProviders)
	return e, f
}

// The filter narrows the rows by name or id; the arrows move the
// highlight and Enter picks it; Escape closes without a pick. An
// OpenAI-compatible server can run a model it does not list: the typed id
// is a row of its own.
func TestModelPickerFilterKeysAndTypedModel(t *testing.T) {
	e, _ := serverEnv(t)
	tt := e.hub(t)
	tt.Frame()
	choose := trigger(i18n.T("settings.providers.choose_model"))
	press(t, tt, choose)
	tt.Frame()
	if !tt.Focused(i18n.T("model_picker.filter")) {
		t.Fatal("the filter does not have the focus")
	}
	tt.Type("qw")
	tt.Frame()
	if !hasOption(tt, "qwen2.5") || hasOption(tt, "llama3.2") || hasOption(tt, "GPT-5") {
		t.Fatalf("filter qw: %q", tt.Texts())
	}
	tt.Key(0, mu.KeyEnter)
	tt.Frame()
	if pr := provider(e, "my-server"); pr.ModelName() != "qwen2.5" || e.a.pick.open != "" {
		t.Fatalf("model %q open %q", pr.ModelName(), e.a.pick.open)
	}

	// The id matches too ("openai/" is in GPT-5's id), and Down moves on.
	press(t, tt, trigger("qwen2.5"))
	tt.Frame()
	if !optionOn(tt, "qwen2.5") {
		t.Fatal("the chosen model is not marked")
	}
	tt.Type("l")
	tt.Frame()
	tt.Key(0, mu.KeyDown)
	tt.Frame()
	tt.Key(0, mu.KeyEnter)
	tt.Frame()
	// "l" matches llama3.2 and the typed "l"; Down goes from llama to the typed row.
	if pr := provider(e, "my-server"); pr.ModelName() != "l" {
		t.Fatalf("model %q after Down, Enter", pr.ModelName())
	}

	press(t, tt, trigger("l"))
	tt.Frame()
	tt.Type("zzz")
	tt.Frame()
	if !tt.HasText(i18n.T("model_picker.no_match")) {
		t.Errorf("no match shows nothing: %q", tt.Texts())
	}
	typed := i18n.T("model_picker.use_typed", "model", "zzz")
	if !hasOption(tt, typed) {
		t.Fatalf("no typed row: %q", tt.Texts())
	}
	tt.Key(0, mu.KeyEscape)
	tt.Frame()
	if pr := provider(e, "my-server"); pr.ModelName() != "l" || e.a.pick.open != "" {
		t.Fatalf("Escape: model %q open %q", pr.ModelName(), e.a.pick.open)
	}
	press(t, tt, trigger("l"))
	tt.Frame()
	tt.Type("my-own-model")
	tt.Frame()
	press(t, tt, i18n.T("model_picker.use_typed", "model", "my-own-model"))
	tt.Frame()
	if pr := provider(e, "my-server"); pr.ModelName() != "my-own-model" {
		t.Fatalf("typed model %q", pr.ModelName())
	}
}

// A press outside the sheet closes it without a pick.
func TestModelPickerClosesOnOutsidePress(t *testing.T) {
	e, _ := serverEnv(t)
	tt := e.hub(t)
	tt.Frame()
	press(t, tt, trigger(i18n.T("settings.providers.choose_model")))
	tt.Frame()
	if e.a.pick.open == "" {
		t.Fatal("not open")
	}
	tt.ClickAt(HubWidth-10, HubHeight-10)
	tt.Frame()
	if e.a.pick.open != "" || provider(e, "my-server").ModelName() != "" {
		t.Fatalf("open %q model %q", e.a.pick.open, provider(e, "my-server").ModelName())
	}
}

// A failed fetch says why; Refresh fetches again; an empty list says so.
func TestModelPickerErrorEmptyAndRefresh(t *testing.T) {
	e, f := serverEnv(t)
	f.errs = map[string]error{"my-server": errors.New("HTTP 401: bad key")}
	tt := e.hub(t)
	tt.Frame()
	press(t, tt, trigger(i18n.T("settings.providers.choose_model")))
	tt.Frame()
	failed := i18n.T("model_picker.load_failed", "error", "HTTP 401: bad key")
	if !tt.HasText(failed) || !slices.Contains(tt.Alerts(), failed) && !tt.HasText(failed) {
		t.Fatalf("no error: %q", tt.Texts())
	}
	f.errs, f.lists["my-server"] = nil, []enhance.ModelChoice{}
	press(t, tt, i18n.T("model_picker.refresh"))
	tt.Frame()
	if f.count() != 2 || !tt.HasText(i18n.T("model_picker.no_models")) {
		t.Fatalf("calls %d: %q", f.count(), tt.Texts())
	}
}

// The fetch runs off the UI thread: the sheet shows Loading and builds
// frames while it waits, Refresh waits for it, and an answer for a list
// that was dropped meanwhile is thrown away.
func TestModelPickerLoadsInTheBackground(t *testing.T) {
	posts := make(chan func(), 16)
	e, f := serverEnvPosting(t, func(fn func()) { posts <- fn })
	f.gate = make(chan struct{})
	tt := e.hub(t)
	tt.Frame()
	press(t, tt, trigger(i18n.T("settings.providers.choose_model")))
	tt.Frame()
	if !tt.HasText(i18n.T("model_picker.loading")) {
		t.Fatalf("no Loading: %q", tt.Texts())
	}
	if r, _ := tt.Access(i18n.T("model_picker.refresh")); !r.Disabled {
		t.Error("Refresh is on while the list loads")
	}
	close(f.gate)
	drain := func(done func() bool) {
		t.Helper()
		deadline := time.After(3 * time.Second)
		for !done() {
			select {
			case fn := <-posts:
				fn()
			case <-deadline:
				t.Fatal("the fetch never answered")
			}
		}
	}
	drain(func() bool { l := e.a.list("my-server"); return l != nil && l.phase == listReady })
	tt.Frame()
	if !hasOption(tt, "qwen2.5") {
		t.Fatalf("the answer does not show: %q", tt.Texts())
	}

	// An answer for a list that was dropped (a new key) is not kept.
	f.gate = make(chan struct{})
	e.a.loadProviderModels("my-server", true)
	e.a.dropModels("my-server")
	for len(posts) > 0 { // what came before, as the redraw of the first answer
		(<-posts)()
	}
	close(f.gate)
	// The next post is the second fetch's answer.
	select {
	case fn := <-posts:
		fn()
	case <-time.After(3 * time.Second):
		t.Fatal("the second fetch never answered")
	}
	if e.a.list("my-server") != nil {
		t.Fatal("a stale answer came back")
	}
}

// ChatGPT has a model picker too, over the account's list. A level the
// new model lacks becomes its lowest one.
func TestChatGPTModelPicker(t *testing.T) {
	e := newEnv(t)
	withLister(e, &fakeLister{lists: map[string][]enhance.ModelChoice{"chatgpt": {
		{ID: "gpt-5.1-codex-mini", Name: "GPT-5.1 Codex Mini", Thinking: []string{"low", "medium", "high"}},
		{ID: "gpt-5.1", Name: "GPT-5.1", Thinking: []string{"minimal", "low", "medium", "high"}},
	}}})
	e.a.cfg.AI.Providers = []config.Provider{{ID: "chatgpt", Name: "ChatGPT", Kind: config.KindChatGPT, Model: ptr("gpt-5.1"), Thinking: ptr("minimal")}}
	e.a.OpenSettings(SecProviders)
	tt := e.hub(t)
	tt.Frame()
	press(t, tt, trigger("gpt-5.1 · Minimal"))
	tt.Frame()
	if !optionOn(tt, "GPT-5.1") {
		t.Fatalf("no chosen GPT-5.1: %q", tt.Texts())
	}
	press(t, tt, "GPT-5.1 Codex Mini")
	tt.Frame()
	if pr := provider(e, "chatgpt"); pr.ModelName() != "gpt-5.1-codex-mini" || pr.ThinkingChosen() != "low" {
		t.Fatalf("model %q thinking %q", pr.ModelName(), pr.ThinkingChosen())
	}
	if slices.Contains(tt.Texts(), "Minimal") {
		t.Errorf("Thinking still offers a level the model lacks: %q", tt.Texts())
	}
}

// Styles: a style can name one of its provider's models, or "Provider's
// model", which leaves the choice to the provider.
func TestStyleModelPicker(t *testing.T) {
	e, f := claudeEnv(t)
	e.a.SetPage(PageStyles)
	tt := mu.NewTester(e.a.HubView, HubWidth, 1800)
	press(t, tt, i18n.T("styles.new_style"))
	tt.Frame()
	label := i18n.T("styles.editor.provider_model_with", "model", "sonnet")
	press(t, tt, trigger(label))
	tt.Frame()
	extra := i18n.T("styles.editor.provider_model")
	if !optionOn(tt, extra) {
		t.Fatalf("no chosen Provider's model row: %q", tt.Texts())
	}
	// Styles have no thinking level of their own.
	if _, ok := tt.AccessByRole(mu.RoleSlider, i18n.T("model_picker.thinking")); ok {
		t.Error("the style's sheet has a thinking line")
	}
	press(t, tt, "Haiku 4.5")
	tt.Frame()
	if e.a.sty.draft.model != "haiku" {
		t.Fatalf("draft model %q", e.a.sty.draft.model)
	}
	press(t, tt, trigger("Haiku 4.5"))
	tt.Frame()
	press(t, tt, extra)
	tt.Frame()
	if e.a.sty.draft.model != "" || f.count() != 1 {
		t.Fatalf("draft model %q, fetched %d", e.a.sty.draft.model, f.count())
	}
	// The provider's model stays the provider's.
	if provider(e, "claude-code").ModelName() != "sonnet" {
		t.Error("the style's pick changed the provider")
	}
}

// Every interactive part of the sheet has its stable key, for
// mygo-automation.
func TestModelPickerKeys(t *testing.T) {
	e, _ := claudeEnv(t)
	tt := e.hub(t)
	tt.Frame()
	press(t, tt, trigger("sonnet · Low"))
	tt.Frame()
	for _, k := range []string{"providers.model.claude-code", "model_picker.filter", "model_picker.refresh", "model_picker.rows",
		"model_picker.count", "model_picker.row.opus", "model_picker.row.haiku", "model_picker.thinking"} {
		if !e.a.keys[k] {
			t.Errorf("no key %q", k)
		}
	}
}

func TestShortModel(t *testing.T) {
	for in, want := range map[string]string{"openai/gpt-5": "gpt-5", "haiku": "haiku", "a/": "a/"} {
		if got := shortModel(in); got != want {
			t.Errorf("shortModel(%q) = %q, want %q", in, got, want)
		}
	}

}

// ---- Screenshots ----

// The picker open in Settings › Providers (Claude CLI, ChatGPT, an
// OpenAI-compatible server) and in the style editor, light and dark,
// written to $SAYSO_PICKER_SHOTS (skipped without it).
func TestModelPickerShots(t *testing.T) {
	out := os.Getenv("SAYSO_PICKER_SHOTS")
	if out == "" {
		t.Skip("SAYSO_PICKER_SHOTS not set")
	}
	lists := map[string][]enhance.ModelChoice{
		"claude-code": claudeModels,
		"chatgpt": {
			{ID: "gpt-5.1-codex-mini", Name: "GPT-5.1 Codex Mini", Description: desc("Faster, cheaper Codex"), Thinking: []string{"medium", "high"}},
			{ID: "gpt-5.1-codex", Name: "GPT-5.1 Codex", Description: desc("Optimized for coding"), Thinking: []string{"low", "medium", "high"}},
			{ID: "gpt-5.1", Name: "GPT-5.1", Description: desc("Broad world knowledge"), Thinking: []string{"minimal", "low", "medium", "high"}},
		},
		"openrouter": {
			{ID: "anthropic/claude-haiku-4.5", Name: "Anthropic: Claude Haiku 4.5"},
			{ID: "google/gemini-2.5-flash", Name: "Google: Gemini 2.5 Flash"},
			{ID: "meta-llama/llama-3.3-70b-instruct", Name: "Meta: Llama 3.3 70B Instruct"},
			{ID: "openai/gpt-5-mini", Name: "OpenAI: GPT-5 Mini"},
			{ID: "qwen/qwen3-32b", Name: "Qwen: Qwen3 32B"},
		},
	}
	all := []config.Provider{
		{ID: "claude-code", Name: "Claude Code", Kind: config.KindClaudeCLI, Model: ptr("sonnet"), Thinking: ptr("medium")},
		{ID: "chatgpt", Name: "ChatGPT", Kind: config.KindChatGPT, Model: ptr("gpt-5.1-codex")},
		{ID: "openrouter", Name: "OpenRouter", Kind: config.KindOpenAICompatible, BaseURL: "https://openrouter.ai/api/v1", Model: ptr("openai/gpt-5-mini")},
	}
	save := func(tt *mu.Tester, name string) {
		t.Helper()
		if err := os.MkdirAll(out, 0o755); err != nil {
			t.Fatal(err)
		}
		f, err := os.Create(filepath.Join(out, name))
		if err != nil {
			t.Fatal(err)
		}
		defer f.Close()
		if err := png.Encode(f, tt.Image()); err != nil {
			t.Fatal(err)
		}
	}
	env := func(dark bool) *testEnv {
		e := newEnv(t)
		withLister(e, &fakeLister{lists: lists})
		e.a.cfg.AI.Providers = append([]config.Provider(nil), all...)
		e.a.cfg.AI.DefaultProvider = ptr("claude-code")
		e.a.cfg.AI.Enabled = true
		if dark {
			e.a.cfg.Appearance.Theme = config.ThemeDark
		}
		return e
	}
	for _, dark := range []bool{false, true} {
		mode := map[bool]string{false: "light", true: "dark"}[dark]
		for _, pr := range all {
			e := env(dark)
			e.a.OpenSettings(SecProviders)
			e.a.set.provSel = pr.ID
			e.a.loadProviderModels(pr.ID, false)
			tt := e.hub(t)
			tt.SetDark(dark)
			tt.Frame()
			press(t, tt, trigger(e.a.providerModelLabel(provider(e, pr.ID))))
			tt.Frame()
			if pr.ID == "openrouter" {
				tt.Type("gpt")
				tt.Frame()
			}
			tt.Frame()
			save(tt, fmt.Sprintf("providers-%s-%s.png", strings.ReplaceAll(string(pr.Kind), "_", "-"), mode))
		}
		e := env(dark)
		e.a.SetPage(PageStyles)
		tt := mu.NewTester(e.a.HubView, HubWidth, HubHeight)
		tt.SetDark(dark)
		press(t, tt, i18n.T("styles.new_style"))
		tt.Frame()
		e.a.loadProviderModels("claude-code", false)
		tt.Frame()
		pressPrefix(t, tt, trigger(i18n.T("styles.editor.provider_model")))
		tt.Frame()
		save(tt, "styles-"+mode+".png")
	}
}

// A server whose model list fails, or lists nothing, still takes a typed
// model: the Use row shows under the error, and Enter saves it.
func TestModelPickerTypedModelWhenTheListFails(t *testing.T) {
	for _, fail := range []bool{true, false} {
		e, f := serverEnv(t)
		if fail {
			f.errs = map[string]error{"my-server": errors.New("HTTP 404: no /models")}
		} else {
			f.lists["my-server"] = []enhance.ModelChoice{}
		}
		tt := e.hub(t)
		tt.Frame()
		press(t, tt, trigger(i18n.T("settings.providers.choose_model")))
		tt.Frame()
		state := i18n.T("model_picker.no_models")
		if fail {
			state = i18n.T("model_picker.load_failed", "error", "HTTP 404: no /models")
		}
		if !tt.HasText(state) {
			t.Fatalf("fail %v: no state: %q", fail, tt.Texts())
		}
		tt.Type("my-model")
		tt.Frame()
		if !hasOption(tt, i18n.T("model_picker.use_typed", "model", "my-model")) || !tt.HasText(state) {
			t.Fatalf("fail %v: no Use row: %q", fail, tt.Texts())
		}
		tt.Key(0, mu.KeyEnter)
		tt.Frame()
		if pr := provider(e, "my-server"); pr.ModelName() != "my-model" || e.a.pick.open != "" {
			t.Fatalf("fail %v: model %q open %q", fail, pr.ModelName(), e.a.pick.open)
		}
	}
}

// A cleanup model has no list: its list is empty at once, never Loading,
// and the style editor has no Model field for it (styles.rs).
func TestCleanupProviderHasNoModelPicker(t *testing.T) {
	e := newEnv(t)
	f := withLister(e, &fakeLister{})
	cleanup := config.Provider{ID: "cleanup", Name: "Cleanup 1", Kind: config.KindCleanupModel, Model: ptr("sayso-cleanup-1")}
	e.a.cfg.AI.Providers = []config.Provider{cleanup}
	e.a.cfg.AI.DefaultProvider = ptr("cleanup")
	e.a.loadModels("cleanup", cleanup, nil, false)
	if l := e.a.list("cleanup"); l == nil || l.phase != listReady || len(l.models) != 0 || f.count() != 0 {
		t.Fatalf("list %+v, fetches %d", l, f.count())
	}
	e.a.SetPage(PageStyles)
	tt := mu.NewTester(e.a.HubView, HubWidth, 1800)
	press(t, tt, i18n.T("styles.new_style"))
	tt.Frame()
	if e.a.keys["styles.editor.model"] || tt.HasText(i18n.T("model_picker.loading")) {
		t.Fatalf("a cleanup model has a Model field: %q", tt.Texts())
	}
	// Another provider brings it back.
	e.a.cfg.AI.Providers = append(e.a.cfg.AI.Providers, config.Provider{ID: "claude-code", Name: "Claude Code", Kind: config.KindClaudeCLI, Model: ptr("sonnet")})
	e.a.sty.draft.provider = "claude-code"
	tt.Frame()
	if !e.a.keys["styles.editor.model"] {
		t.Fatal("no Model field for the Claude CLI")
	}
}

// The onboarding cloud form's picker fetches its list once as it opens.
func TestOnboardingCloudPickerFetchesOnce(t *testing.T) {
	e := newEnv(t)
	f := withLister(e, &fakeLister{lists: map[string][]enhance.ModelChoice{onbCloudList: {{ID: "openai/gpt-5-mini", Name: "GPT-5 Mini"}}}})
	e.a.goToStep(stepAI)
	tt := onboardingTester(e)
	tt.Frame()
	press(t, tt, i18n.T("onboarding.ai.cloud_api"))
	tt.Frame()
	e.a.onb.ai.key = "sk-test"
	tt.Frame()
	pressPrefix(t, tt, i18n.T("onboarding.ai.model_button", "name", i18n.T("onboarding.ai.model"), "value", i18n.T("onboarding.ai.choose_model")))
	for range 3 {
		tt.Frame()
	}
	if f.count() != 1 || !hasOption(tt, "GPT-5 Mini") {
		t.Fatalf("fetched %d times: %q", f.count(), tt.Texts())
	}
	// A new key is another list: one more fetch.
	e.a.onb.ai.key = "sk-other"
	tt.Frame()
	tt.Frame()
	if f.count() != 2 {
		t.Fatalf("fetched %d times after a new key", f.count())
	}
}
