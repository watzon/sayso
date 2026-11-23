package ui

import (
	"math"
	"runtime"
	"slices"
	"strings"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/theme"
)

// Step 3: optional AI clean-up. Continue saves the choice; Skip turns AI
// off (onboarding/ai.rs).

type aiKind int

const (
	aiNone aiKind = iota
	aiApple
	aiLocal
	aiCloud
	aiClaude
	aiCleanup
)

// aiChoice is an option of the step: a kind, and for a cleanup model its
// place in the catalog.
type aiChoice struct {
	kind    aiKind
	cleanup int
}

func (ch aiChoice) key() string {
	switch ch.kind {
	case aiApple:
		return "apple"
	case aiLocal:
		return "ollama"
	case aiCloud:
		return "cloud"
	case aiClaude:
		return "claude"
	case aiCleanup:
		return "cleanup." + enhance.CleanupCatalog()[ch.cleanup].ID
	}
	return "none"
}

// aiError is what is wrong with the form. The form keeps the kind, not the
// text, so the text follows the interface language.
type aiError int

const (
	aiErrNone aiError = iota
	aiErrChoose
	aiErrApple
	aiErrOllama
	aiErrBaseURL
	aiErrAPIKey
	aiErrModel
	aiErrClaude
)

// aiPresets are the base URLs of the cloud option: label, id, URL.
var aiPresets = [3][3]string{
	{"OpenRouter", "openrouter", "https://openrouter.ai/api/v1"},
	{"OpenAI", "openai", "https://api.openai.com/v1"},
	{"Other", "custom", ""},
}

// detected are the AI tools found on this computer.
type detected struct {
	done, running bool
	claude        string
	ollama        []string
	ollamaOK      bool
	apple         string // the model name when Apple Intelligence runs here
	appleErr      string // why it does not, when the engine said
}

type aiForm struct {
	inited bool
	choice *aiChoice
	local  string // the chosen Ollama model; "" is the first
	// localOpen shows the list of Ollama models.
	localOpen bool
	preset    int
	baseURL   string
	key       string
	model     string
	// fetched is the URL and key the cloud form's model list (App.pick,
	// onbCloudList) was fetched with.
	fetched  string
	err      aiError
	testing  bool
	testDone bool
	testMs   uint64
	testErr  string
	found    detected
}

// detectProviders looks for Claude CLI, Ollama and Apple Intelligence off
// the UI thread. force asks again (Ollama may have a new model).
func (a *App) detectProviders(force bool) {
	f := &a.onb.ai.found
	if f.running || (f.done && !force) {
		return
	}
	f.running = true
	lm := a.engineLanguageModel()
	detect := a.detectAI
	if detect == nil {
		detect = detectAIHere
	}
	run := func() {
		d := detect(lm)
		d.done = true
		a.post(func() {
			a.onb.ai.found = d
			a.invalidate()
		})
	}
	if a.d.Post == nil {
		run() // no UI thread to come back to (tests)
		return
	}
	go run()
}

// detectAIHere finds the AI tools of this computer. It blocks.
func detectAIHere(lm enhance.LanguageModel) detected {
	var d detected
	d.claude = enhance.DetectClaudeCLI()
	d.ollama, d.ollamaOK = enhance.DetectOllama(600 * time.Millisecond)
	if lm != nil {
		if name, err := lm.ModelName(); err == nil {
			d.apple = name
		} else {
			d.appleErr = err.Error()
		}
	}
	return d
}

// engineLanguageModel is Apple Intelligence through the engine, on macOS.
func (a *App) engineLanguageModel() enhance.LanguageModel {
	if runtime.GOOS != "darwin" || a.d.Engine == nil {
		return nil
	}
	if e, ok := a.d.Engine.(interface{ LanguageModel() enhance.LanguageModel }); ok {
		return e.LanguageModel()
	}
	return nil
}

// initAI starts the form from the saved provider when onboarding resumes
// at this step.
func (a *App) initAI() {
	f := &a.onb.ai
	if f.inited {
		return
	}
	f.inited = true
	f.baseURL = aiPresets[0][2]
	ai := a.cfg.AI
	if !ai.Enabled {
		return
	}
	pr, ok := ai.Provider("")
	if !ok {
		return
	}
	switch pr.Kind {
	case config.KindOpenAICompatible:
		if config.IsLocalURL(pr.BaseURL) {
			f.choice, f.local = &aiChoice{kind: aiLocal}, pr.ModelName()
			return
		}
		f.choice, f.preset, f.baseURL, f.model = &aiChoice{kind: aiCloud}, 2, pr.BaseURL, pr.ModelName()
		for i, ps := range aiPresets {
			if ps[2] == pr.BaseURL {
				f.preset = i
			}
		}
	case config.KindClaudeCLI:
		f.choice = &aiChoice{kind: aiClaude}
	case config.KindAppleIntelligence:
		f.choice = &aiChoice{kind: aiApple}
	case config.KindCleanupModel:
		for i, m := range enhance.CleanupCatalog() {
			if m.ID == pr.ModelName() {
				f.choice = &aiChoice{kind: aiCleanup, cleanup: i}
			}
		}
	}
}

// aiChoice is the selected option. None (ok false) until the user chose
// one: Continue is then off, and Skip is the way on. Claude Code comes
// first when it is on this computer: a larger model cleans up better.
func (a *App) aiChoice() (aiChoice, bool) {
	f := &a.onb.ai
	if f.choice != nil {
		return *f.choice, true
	}
	if f.found.claude != "" {
		return aiChoice{kind: aiClaude}, true
	}
	if ch, ok := a.suggestedCleanup(); ok {
		return ch, true
	}
	if len(f.found.ollama) > 0 {
		return aiChoice{kind: aiLocal}, true
	}
	return aiChoice{}, false
}

// suggestedCleanup is the first cleanup model that knows the dictation
// language, as the speech model hears it.
func (a *App) suggestedCleanup() (aiChoice, bool) {
	m, _ := models.Find(a.FinalModel())
	lang := m.LanguageFor(a.cfg.Dictation.Language)
	for i, info := range enhance.CleanupCatalog() {
		if slices.Contains(info.Languages, lang) {
			return aiChoice{kind: aiCleanup, cleanup: i}, true
		}
	}
	return aiChoice{}, false
}

// aiDownloads is true when Continue downloads a cleanup model.
func (a *App) aiDownloads() bool {
	ch, ok := a.aiChoice()
	return ok && ch.kind == aiCleanup
}

// aiProvider is the provider of the form and its API key, or what to fix.
func (a *App) aiProvider() (config.Provider, string, aiError) {
	f := &a.onb.ai
	ch, ok := a.aiChoice()
	if !ok {
		return config.Provider{}, "", aiErrChoose
	}
	switch ch.kind {
	case aiApple:
		if f.found.apple == "" {
			return config.Provider{}, "", aiErrApple
		}
		return config.Provider{ID: "apple-intelligence", Name: "Apple Intelligence", Kind: config.KindAppleIntelligence}, "", aiErrNone
	case aiLocal:
		model := f.local
		if model == "" && len(f.found.ollama) > 0 {
			model = f.found.ollama[0]
		}
		if model == "" {
			return config.Provider{}, "", aiErrOllama
		}
		return config.Provider{ID: "ollama", Name: "Ollama", Kind: config.KindOpenAICompatible, BaseURL: enhance.OllamaURL + "/v1", Model: ptr(model)}, "", aiErrNone
	case aiCloud:
		url := strings.TrimRight(strings.TrimSpace(f.baseURL), "/")
		key := strings.TrimSpace(f.key)
		if !strings.HasPrefix(url, "https://") && !strings.HasPrefix(url, "http://") {
			return config.Provider{}, "", aiErrBaseURL
		}
		if key == "" && !config.IsLocalURL(url) {
			return config.Provider{}, "", aiErrAPIKey
		}
		if f.model == "" {
			return config.Provider{}, "", aiErrModel
		}
		id, name := "custom", "Custom endpoint"
		if ps := aiPresets[f.preset]; ps[2] == url {
			id, name = ps[1], ps[0]
		}
		pr := config.Provider{ID: id, Name: name, Kind: config.KindOpenAICompatible, BaseURL: url, Model: ptr(f.model)}
		if key != "" {
			pr.APIKeyAccount = ptr("provider." + id)
		}
		return pr, key, aiErrNone
	case aiCleanup:
		info := enhance.CleanupCatalog()[ch.cleanup]
		return config.Provider{ID: info.ID, Name: info.Name, Kind: config.KindCleanupModel, Model: ptr(info.ID)}, "", aiErrNone
	case aiClaude:
		if f.found.claude == "" {
			return config.Provider{}, "", aiErrClaude
		}
		return config.Provider{ID: "claude-cli", Name: "Claude CLI", Kind: config.KindClaudeCLI, Path: ptr(f.found.claude), Model: ptr("haiku")}, "", aiErrNone
	}
	return config.Provider{}, "", aiErrChoose
}

// saveProvider saves pr (replacing one with its id), its key, makes it
// the default provider and turns AI on (model.rs save_provider).
func (a *App) saveProvider(pr config.Provider, key string) error {
	if key != "" && pr.APIKeyAccount != nil && a.d.Secrets != nil {
		if err := a.d.Secrets.Set(*pr.APIKeyAccount, key); err != nil {
			return err
		}
	}
	ai := &a.cfg.AI
	ai.Providers = slices.DeleteFunc(ai.Providers, func(x config.Provider) bool { return x.ID == pr.ID })
	ai.Providers = append(ai.Providers, pr)
	ai.DefaultProvider = ptr(pr.ID)
	ai.Enabled = true
	a.saveConfig()
	a.refreshSecrets()
	return nil
}

// saveAI saves the choice. It returns false when the form needs a fix.
func (a *App) saveAI() bool {
	f := &a.onb.ai
	pr, key, e := a.aiProvider()
	if e != aiErrNone {
		f.err = e
		return false
	}
	f.err = aiErrNone
	if err := a.saveProvider(pr, key); err != nil {
		f.testErr, f.testDone = err.Error(), true
		return false
	}
	// Keep the list the user picked from, so Styles shows its names.
	if ch, _ := a.aiChoice(); ch.kind == aiCloud {
		if l := a.list(onbCloudList); l != nil && l.phase == listReady {
			copied := *l
			a.pick.lists[pr.ID] = &copied
		}
	}
	// A cleanup model downloads in the background: the step does not wait.
	if ch, _ := a.aiChoice(); ch.kind == aiCleanup {
		info := enhance.CleanupCatalog()[ch.cleanup]
		if a.d.Paths.DataDir != "" && !enhance.CleanupModelReady(a.d.Paths.ModelsDir(), &info) {
			a.downloadCleanup(info)
		}
	}
	return true
}

// skipAI leaves the step without a provider. A provider that "Test
// connection" saved stays in Settings, but AI is off.
func (a *App) skipAI() {
	a.onb.ai.err = aiErrNone
	if a.cfg.AI.Enabled {
		a.cfg.AI.Enabled = false
		a.saveConfig()
	}
}

// testAI saves the provider of the form and makes one call to it.
func (a *App) testAI() {
	f := &a.onb.ai
	pr, key, e := a.aiProvider()
	if e != aiErrNone {
		f.err = e
		return
	}
	if err := a.saveProvider(pr, key); err != nil {
		f.testDone, f.testErr = true, err.Error()
		return
	}
	f.err, f.testing, f.testDone = aiErrNone, true, false
	en := enhance.BuildEnhancer(pr, a.d.Secrets, a.cfg.AI, a.engineLanguageModel())
	a.background(func() {
		ms, err := enhance.TestProvider(en)
		a.post(func() {
			f.testing, f.testDone, f.testMs, f.testErr = false, true, ms, ""
			if err != nil {
				f.testErr = err.Error()
			}
			a.invalidate()
		})
	})
}

func (a *App) selectAI(ch aiChoice) {
	f := &a.onb.ai
	f.choice = &ch
	f.err, f.testDone = aiErrNone, false
	// Ask Ollama again: the user can pull a model while this step is open.
	if ch.kind == aiLocal {
		a.detectProviders(true)
	}
}

func (e aiError) text(f *aiForm) string {
	switch e {
	case aiErrChoose:
		return i18n.T("onboarding.ai.error_choose")
	case aiErrApple:
		if f.found.appleErr != "" {
			return i18n.T("onboarding.ai.error_apple_reason", "reason", f.found.appleErr)
		}
		return i18n.T("onboarding.ai.error_apple")
	case aiErrOllama:
		return i18n.T("onboarding.ai.error_ollama")
	case aiErrBaseURL:
		return i18n.T("onboarding.ai.error_base_url")
	case aiErrAPIKey:
		return i18n.T("onboarding.ai.error_api_key", "store", i18n.TOS("os.secret_store", nil))
	case aiErrModel:
		return i18n.T("onboarding.ai.error_model")
	case aiErrClaude:
		return i18n.T("onboarding.ai.error_claude")
	}
	return ""
}

func (a *App) onbAI(c *mu.Context, p theme.Colors) {
	a.initAI()
	a.detectProviders(false)
	f := &a.onb.ai
	d := f.found
	choice, chosen := a.aiChoice()
	// The options this computer has, in order: the larger models first,
	// Apple Intelligence last.
	var order []aiChoice
	if d.claude != "" {
		order = append(order, aiChoice{kind: aiClaude})
	}
	order = append(order, aiChoice{kind: aiCloud})
	if d.ollamaOK {
		order = append(order, aiChoice{kind: aiLocal})
	}
	for i := range enhance.CleanupCatalog() {
		order = append(order, aiChoice{kind: aiCleanup, cleanup: i})
	}
	if d.apple != "" {
		order = append(order, aiChoice{kind: aiApple})
	}
	is := func(ch aiChoice) bool { return chosen && ch == choice }
	sel := -1
	if chosen {
		sel = slices.Index(order, choice)
	}
	stepColumn(c, p, i18n.T("onboarding.ai.title"), i18n.T("onboarding.ai.lead"), func() {
		mu.Column(a.k(c, "onboarding.ai.options")).Gap(8).Role(mu.RoleRadioGroup).Label(i18n.T("onboarding.ai.title")).Children(func() {
			if !d.done {
				uiText(c, i18n.T("onboarding.ai.looking", "computer", computer()), 13, 400, p.Graphite).PaddingX(4)
			}
			for i, ch := range order {
				switch ch.kind {
				case aiClaude:
					a.aiRow(c, p, ch, is(ch), i, order, sel, "Claude CLI", "", i18n.T("onboarding.ai.claude_desc", "path", tildePath(d.claude)),
						func() { badge(c, p, i18n.T("onboarding.ai.cloud"), toneAccent, "") }, nil, nil)
				case aiCloud:
					var extra func()
					if is(ch) {
						extra = func() { a.cloudForm(c, p) }
					}
					a.aiRow(c, p, ch, is(ch), i, order, sel, i18n.T("onboarding.ai.cloud_api"), "",
						i18n.T("onboarding.ai.cloud_desc", "store", i18n.TOS("os.secret_store", nil)),
						func() { badge(c, p, i18n.T("onboarding.ai.cloud"), toneAccent, "") }, nil, extra)
				case aiLocal:
					if len(d.ollama) == 0 {
						a.aiRow(c, p, ch, is(ch), i, order, sel, "Ollama", "", i18n.T("onboarding.ai.ollama_empty", "computer", computer()), nil, nil, nil)
						continue
					}
					var extra func()
					if f.localOpen && is(ch) {
						extra = func() { a.ollamaList(c, p) }
					}
					a.aiRow(c, p, ch, is(ch), i, order, sel, "Ollama", "", i18n.T("onboarding.ai.ollama_running", "computer", computer()),
						nil, func() { a.ollamaPicker(c, p) }, extra)
				case aiCleanup:
					info := enhance.CleanupCatalog()[ch.cleanup]
					a.aiRow(c, p, ch, is(ch), i, order, sel, info.Name, i18n.T("onboarding.ai.by", "publisher", info.Publisher), cleanupDesc(info),
						func() { sizeBadge(c, p, info.SizeBytes) }, nil, nil)
				case aiApple:
					a.aiRow(c, p, ch, is(ch), i, order, sel, "Apple Intelligence", "", i18n.T("onboarding.ai.apple_desc", "model", d.apple), nil, nil, nil)
				}
			}
		})
		if f.err != aiErrNone && !(chosen && choice.kind == aiCloud) {
			banner(c, p, bannerWarning, f.err.text(f))
		}
	})
}

// aiRow is one option. The radio button holds the dot, the title and the
// description, and trailing (a badge). control and extra are outside it,
// because a button cannot hold another control.
func (a *App) aiRow(c *mu.Context, p theme.Colors, ch aiChoice, selected bool, i int, order []aiChoice, sel int, title, by, desc string, trailing, control, extra func()) {
	n := len(order)
	card := mu.Column(a.k(c, "onboarding.ai.card."+ch.key())).Radius(12).Background(p.SheetRaised)
	if !selected && card.Hovered() {
		card.Background(p.Sheet)
	}
	if selected {
		card.Shadow(0, 0, 0, 2, p.Ink).Shadow(0, 1, 2, 0, p.ShadowA(0.14))
	} else {
		card.InsetShadow(0, 1, 0, 0, p.Highlight(1)).InsetShadow(0, -1, 0, 0, p.ShadowA(0.18)).Shadow(0, 1, 1.5, 0, p.ShadowA(0.16))
	}
	card.Children(func() {
		mu.Row(c).Gap(14).Padding(14, 16).AlignItems(mu.Center).Children(func() {
			name := title
			if by != "" {
				name += " " + by
			}
			b := mu.ButtonBase(a.k(c, "onboarding.ai."+ch.key())).Label(name).Role(mu.RoleRadio).Checked(selected).
				PositionInSet(i+1, n).Description(desc).Grow(1).Basis(0).MinWidth(0).Gap(14).AlignItems(mu.Center).Cursor(mu.CursorPointer)
			a.radioArrows(b, i, sel, n, func(j int) string { return "ai:" + order[j].key() }, func(j int) { a.selectAI(order[j]) })
			b.Children(func() {
				radioDot(c, p, selected)
				mu.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(2).Children(func() {
					mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
						uiText(c, title, 15, 600, p.Ink).FixedLineHeight(18).SingleLine()
						if by != "" {
							uiText(c, by, 13, 400, p.Graphite).FixedLineHeight(18).SingleLine()
						}
					})
					uiText(c, desc, 13, 400, p.Graphite).FixedLineHeight(16)
				})
				if trailing != nil {
					trailing()
				}
			})
			if clicked(c, b) {
				a.selectAI(ch)
			}
			if control != nil {
				control()
			}
		})
		if extra != nil {
			mu.Column(c).Padding(0, 0, 14, 0).PaddingStart(48).PaddingEnd(16).Children(extra)
		}
	})
}

// ollamaPicker is the button with the chosen Ollama model.
func (a *App) ollamaPicker(c *mu.Context, p theme.Colors) {
	f := &a.onb.ai
	current := f.local
	if current == "" && len(f.found.ollama) > 0 {
		current = f.found.ollama[0]
	}
	b := mu.ButtonBase(a.k(c, "onboarding.ai.ollama_model")).Label(i18n.T("onboarding.ai.model_button", "name", i18n.T("onboarding.ai.ollama_model"), "value", current)).
		Expanded(f.localOpen).Height(30).PaddingX(10).Gap(6).Radius(8).Shrink(0).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	debossed(b, p)
	b.Children(func() {
		uiText(c, current, 13, 400, p.Ink).SingleLine()
		mu.Icon(c, icon("chevron-down")).FontSize(10).TextColor(p.Graphite)
	})
	if clicked(c, b) {
		f.choice = &aiChoice{kind: aiLocal}
		f.localOpen = !f.localOpen
	}
}

// ollamaList is the list of Ollama models under the row.
func (a *App) ollamaList(c *mu.Context, p theme.Colors) {
	f := &a.onb.ai
	current := f.local
	if current == "" && len(f.found.ollama) > 0 {
		current = f.found.ollama[0]
	}
	debossed(mu.Column(a.k(c, "onboarding.ai.ollama_models")).Padding(4).Radius(10), p).Role(mu.RoleRadioGroup).
		Label(i18n.T("onboarding.ai.ollama_model")).Children(func() {
		for i, name := range f.found.ollama {
			on := name == current
			it := a.menuItemBase(c, "onboarding.ai.ollama."+name, name).Role(mu.RoleRadio).Checked(on).PositionInSet(i+1, len(f.found.ollama))
			if it.Hovered() {
				it.Background(p.SheetRaised)
			}
			it.Children(func() {
				mu.Box(c).Width(14).Shrink(0).Children(func() {
					if on {
						mu.Icon(c, icon("check")).FontSize(12).TextColor(p.Ink)
					}
				})
				w := 400
				if on {
					w = 600
				}
				uiText(c, name, 13, w, p.Ink)
			})
			a.radioArrows(it, i, slices.Index(f.found.ollama, current), len(f.found.ollama),
				func(j int) string { return "ollama:" + f.found.ollama[j] },
				func(j int) { f.local, f.testDone = f.found.ollama[j], false })
			if clicked(c, it) {
				f.local, f.localOpen, f.testDone = name, false, false
			}
		}
	})
}

// cloudLabelWidth is the width of the cloud form's label column: the
// widest label in the interface language, at least 70 DIPs, so a long one
// ("API-Schlüssel") stays on one line.
func cloudLabelWidth() float32 {
	w := float32(70)
	for _, k := range []string{"onboarding.ai.base_url", "onboarding.ai.api_key", "onboarding.ai.model"} {
		lw, _ := mu.ShapeText(i18n.T(k), mu.Font{Family: theme.UI, Size: 13, Weight: 500}, 0).Size()
		w = max(w, float32(math.Ceil(float64(lw))))
	}
	return w
}

// cloudForm is the form of the Cloud API option: a preset, the URL, the
// key, the model from the endpoint's list, and a test.
func (a *App) cloudForm(c *mu.Context, p theme.Colors) {
	f := &a.onb.ai
	mu.Column(c).Gap(10).Children(func() {
		mu.Row(a.k(c, "onboarding.ai.presets")).Gap(6).Role(mu.RoleRadioGroup).Label(i18n.T("onboarding.ai.preset_label")).Children(func() {
			for i, ps := range aiPresets {
				label := ps[0]
				if ps[1] == "custom" {
					label = i18n.T("onboarding.ai.preset_other")
				}
				choose := func(j int) { f.preset, f.baseURL, f.model, f.testDone = j, aiPresets[j][2], "", false }
				b := a.chipBase(c, p, "onboarding.ai.preset."+ps[1], label, f.preset == i, "", mu.RoleRadio).PositionInSet(i+1, len(aiPresets))
				a.radioArrows(b, i, f.preset, len(aiPresets), func(j int) string { return "preset:" + aiPresets[j][1] }, choose)
				if clicked(c, b) {
					choose(i)
				}
			}
		})
		field := func(key, label string, v *string, secret bool, placeholder string) {
			mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
				uiText(c, label, 13, 500, p.Graphite).Width(cloudLabelWidth()).Shrink(0)
				in := mu.TextInputBase(a.k(c, key), v).Label(label).Placeholder(placeholder).PlaceholderColor(p.Pencil).
					Height(34).PaddingX(12).Radius(9).FontSize(13).Font(theme.Mono).TextColor(p.Ink).Grow(1).Basis(0).MinWidth(0)
				debossed(in, p)
				if secret {
					in.Password()
				} else if key == "onboarding.ai.base_url" {
					ltr(in)
				}
			})
		}
		field("onboarding.ai.base_url", i18n.T("onboarding.ai.base_url"), &f.baseURL, false, "https://…/v1")
		field("onboarding.ai.api_key", i18n.T("onboarding.ai.api_key"), &f.key, true, i18n.T("onboarding.ai.key_placeholder"))
		a.cloudModel(c, p)
		mu.Row(c).Gap(12).PaddingStart(cloudLabelWidth() + 12).AlignItems(mu.Center).Children(func() {
			if a.button(c, p, "onboarding.ai.test", i18n.T("onboarding.ai.test_connection"), bopt{size: sizeSmall, disabled: f.testing}) && !f.testing {
				a.testAI()
			}
			switch {
			case f.testing:
				uiText(c, i18n.T("onboarding.ai.testing"), 13, 400, p.Graphite).Role(mu.RoleStatus)
			case f.testDone && f.testErr == "":
				checkLine(c, p, i18n.T("onboarding.ai.test_works", "seconds", i18n.Decimal(float64(f.testMs)/1000, 1)), 400).Role(mu.RoleStatus)
			case f.testDone:
				uiText(c, i18n.T("onboarding.ai.test_failed", "error", f.testErr), 13, 400, p.Danger).Role(mu.RoleAlert).Grow(1).Basis(0).MinWidth(0)
			}
		})
		if f.err != aiErrNone {
			banner(c, p, bannerWarning, f.err.text(f))
		}
	})
}

// onbCloudList is the key of the cloud form's model list in App.pick.
const onbCloudList = "onboarding.cloud"

// cloudDraft is the provider the cloud form would save, for its model
// list. Its key stays in memory.
func (a *App) cloudDraft() (config.Provider, providers.SecretStore) {
	f := &a.onb.ai
	account := "onboarding.draft"
	secrets := providers.NewMemoryStore()
	if key := strings.TrimSpace(f.key); key != "" {
		secrets.Set(account, key)
	}
	return config.Provider{ID: onbCloudList, Name: "draft", Kind: config.KindOpenAICompatible,
		BaseURL: strings.TrimRight(strings.TrimSpace(f.baseURL), "/"), APIKeyAccount: &account}, secrets
}

// cloudModel is the model row of the cloud form: a picker over the models
// the endpoint reports, fetched when it opens (onboarding/ai.rs cloud_model).
func (a *App) cloudModel(c *mu.Context, p theme.Colors) {
	f := &a.onb.ai
	mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
		uiText(c, i18n.T("onboarding.ai.model"), 13, 500, p.Graphite).Width(cloudLabelWidth()).Shrink(0)
		label := i18n.T("onboarding.ai.choose_model")
		if f.model != "" {
			label = f.model
			if m, ok := a.listedModel(onbCloudList, f.model); ok {
				label = m.Name
			}
		}
		if a.pick.open == "onboarding.ai.model" {
			a.loadCloudModels(false)
		}
		draft, secrets := a.cloudDraft()
		a.modelPicker(c, p, pickReq{
			key: "onboarding.ai.model", listKey: onbCloudList, provider: draft, secrets: secrets,
			label: label, chosen: f.model != "", selected: f.model, field: true, small: true,
			// The form's URL and key decide the list: one fetch for them.
			load: func() { a.loadCloudModels(false) },
			a11y: i18n.T("onboarding.ai.model_button", "name", i18n.T("onboarding.ai.model"), "value", label),
			pick: func(id string) {
				if id != "" {
					f.model, f.testDone, f.err = id, false, aiErrNone
				}
			},
		})
	})
}

// loadCloudModels fetches the model list for the URL and key of the form.
// A changed URL or key fetches again even without force; the answer of
// the old one is dropped.
func (a *App) loadCloudModels(force bool) {
	f := &a.onb.ai
	draft, secrets := a.cloudDraft()
	sig := draft.BaseURL + "\n" + strings.TrimSpace(f.key)
	if f.fetched != sig {
		f.fetched, force = sig, true
		a.dropModels(onbCloudList)
	}
	a.loadModels(onbCloudList, draft, secrets, force)
}

// cleanupDesc is "English only. Formats emails and lists."
func cleanupDesc(info enhance.CleanupModelInfo) string {
	formats := i18n.T("onboarding.ai.cleanup_no_formats")
	if len(info.Formats) > 1 || len(info.Layouts) > 1 {
		formats = i18n.T("onboarding.ai.cleanup_formats")
	}
	first := ""
	if len(info.Languages) > 0 {
		first = i18n.LanguageName(info.Languages[0])
	}
	if len(info.Languages) <= 1 {
		return i18n.T("onboarding.ai.cleanup_desc_one", "language", first, "formats", formats)
	}
	n := len(info.Languages) - 1
	return i18n.TP("onboarding.ai.cleanup_desc_many", n, map[string]any{"count": n, "language": first, "formats": formats})
}

// sizeBadge is the download size, in the green of a local choice.
func sizeBadge(c *mu.Context, p theme.Colors, bytes uint64) {
	mu.Row(c).Height(22).PaddingX(8).Shrink(0).Radius(99).Background(p.Success.Alpha(0.14)).AlignItems(mu.Center).Children(func() {
		uiText(c, sizeText(bytes), 12, 600, p.Success).SingleLine()
	})
}
