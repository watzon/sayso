package ui

import (
	"fmt"
	"math"
	"strings"
	"sync"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/models/languages"
	"github.com/watzon/sayso/internal/theme"
	"github.com/watzon/sayso/internal/update"
)

// tr is a catalog text, or fallback when this version's catalog lacks it.
func tr(key, fallback string) string {
	if t, ok := i18n.Translate(key); ok {
		return t
	}
	return fallback
}

// ---- AI and providers ----

// addProviderSel is the selection of the "Add a provider" card.
const addProviderSel = "+add"

var (
	logoMu    sync.Mutex
	logoCache = map[string]mu.ImageSource{}
)

// providerLogo is the decoded logo of a provider ("openai"), or nil.
func providerLogo(name string) mu.ImageSource {
	logoMu.Lock()
	defer logoMu.Unlock()
	if s, ok := logoCache[name]; ok {
		return s
	}
	var src mu.ImageSource
	if data := theme.ProviderLogo(name); data != nil {
		src, _ = mu.DecodeImage(data)
	}
	logoCache[name] = src
	return src
}

// logoName picks a provider's logo from its kind and address.
func logoName(pr config.Provider) string {
	switch pr.Kind {
	case config.KindClaudeCLI:
		return "claude-color"
	case config.KindChatGPT:
		return "openai"
	case config.KindAppleIntelligence:
		return "apple"
	case config.KindCleanupModel:
		// logo.rs cleanup: the publisher's app icon fills the plate.
		if strings.HasPrefix(pr.ModelName(), "spokenly") {
			return "spokenly"
		}
		return "superwhisper"
	case config.KindACP:
		for _, n := range []string{"opencode", "cursor", "gemini", "qwen", "kimi"} {
			if strings.Contains(strings.ToLower(pr.Command+" "+pr.Name), n) {
				if n == "gemini" || n == "qwen" || n == "kimi" {
					return n + "-color"
				}
				return n
			}
		}
	case config.KindOpenAICompatible:
		u := strings.ToLower(pr.BaseURL)
		switch {
		case strings.Contains(u, "openrouter"):
			return "openrouter"
		case strings.Contains(u, "openai.com"):
			return "openai"
		case strings.Contains(u, ":11434"), strings.Contains(u, "ollama"):
			return "ollama"
		case strings.Contains(u, ":1234"), strings.Contains(u, "lmstudio"):
			return "lmstudio"
		}
	}
	return ""
}

// logoTile is a provider's logo on a raised tile, or its first letter.
func logoTile(c *mu.Context, p theme.Colors, pr config.Provider, size float32) {
	r := size * 0.25
	t := mu.Box(c).Size(size, size).Shrink(0).Radius(r).Justify(mu.Center).AlignItems(mu.Center)
	if size > 40 {
		raised(t, p)
	} else {
		raisedSmall(t, p)
	}
	if pr.Kind == config.KindCleanupModel {
		// An app icon fills its plate (logo.rs Mark::AppIcon).
		if src := providerLogo(logoName(pr)); src != nil {
			t.Radius(float32(math.Round(float64(size * 0.27)))).Clip().Children(func() {
				mu.Image(c, src).Size(size, size)
			})
			return
		}
	}
	t.Children(func() {
		if src := providerLogo(logoName(pr)); src != nil {
			mu.Image(c, src).Size(size*0.6, size*0.6).TextColor(p.Ink)
			return
		}
		switch pr.Kind {
		case config.KindCleanupModel:
			mu.Icon(c, icon("audio")).FontSize(size * 0.55).TextColor(p.Ink)
		default:
			letter := "?"
			for _, r := range pr.Name {
				letter = strings.ToUpper(string(r))
				break
			}
			heading(c, p, letter, size*0.5)
		}
	})
}

// providerGroup is the list group of a provider: 0 subscriptions, 1 keys
// and servers, 2 on this computer.
func providerGroup(pr config.Provider) int {
	switch pr.Kind {
	case config.KindClaudeCLI, config.KindChatGPT, config.KindACP:
		return 0
	case config.KindOpenAICompatible:
		if pr.IsCloud() {
			return 1
		}
	}
	return 2
}

// providerSub is a provider's line under its name: model and thinking.
func (a *App) providerSub(pr config.Provider) string {
	switch pr.Kind {
	case config.KindChatGPT:
		if !a.savedSecrets().chatGPT[pr.ID] {
			return i18n.T("settings.providers.not_signed_in")
		}
	case config.KindAppleIntelligence:
		return i18n.T("settings.providers.apple_model_detail")
	case config.KindCleanupModel:
		// "No prompts · English", as providers/mod.rs summary.
		for _, m := range enhance.CleanupCatalog() {
			if m.ID == pr.ModelName() {
				langs := i18n.TP("core.models.languages", len(m.Languages), nil)
				if len(m.Languages) == 1 {
					langs = languages.Name(m.Languages[0])
				}
				return i18n.T("settings.providers.no_prompts") + " · " + langs
			}
		}
		return i18n.T("hub.place.local")
	}
	parts := []string{}
	if m := pr.ModelName(); m != "" {
		parts = append(parts, m)
	}
	if t := pr.ThinkingChosen(); t != "" {
		parts = append(parts, t)
	}
	if len(parts) == 0 {
		if pr.IsCloud() {
			return i18n.T("hub.place.cloud")
		}
		return i18n.T("hub.place.local")
	}
	return strings.Join(parts, " · ")
}

// providerReady is false for a provider that cannot run yet.
func (a *App) providerReady(pr config.Provider) bool {
	switch pr.Kind {
	case config.KindChatGPT:
		return a.savedSecrets().chatGPT[pr.ID]
	case config.KindOpenAICompatible:
		return !pr.NeedsModel()
	}
	return true
}

func (a *App) providersSection(c *mu.Context, p theme.Colors, sub string) {
	ai := &a.cfg.AI
	mu.Row(c).AlignItems(mu.End).Gap(24).Children(func() {
		mu.Column(c).Gap(6).Grow(1).Basis(0).Children(func() {
			display(c, p, a.section.title(), 44).Role(mu.RoleHeading)
			uiText(c, sub, 15, 400, p.Graphite).LineHeight(22.0 / 15).MaxWidth(560)
		})
		mu.Row(c).Gap(14).AlignItems(mu.Center).Padding(0, 0, 4, 0).Children(func() {
			mu.Column(c).Gap(2).AlignItems(mu.End).Children(func() {
				uiText(c, i18n.T("settings.providers.use_ai"), 14, 600, p.Ink)
				uiText(c, i18n.T("settings.providers.use_ai_short"), 12, 400, p.Graphite)
			})
			a.toggle(c, "providers.ai_enabled", i18n.T("settings.providers.use_ai"), &ai.Enabled, a.saveConfig)
		})
	})
	sel := a.set.provSel
	// ai.Provider("") is the default provider, so "" (nothing picked yet)
	// must fall through to the pick below.
	if _, ok := ai.Provider(sel); (sel == "" || !ok) && sel != addProviderSel {
		sel = addProviderSel
		if len(ai.Providers) > 0 {
			sel = ai.Providers[0].ID
			if ai.DefaultProvider != nil {
				if _, ok := ai.Provider(*ai.DefaultProvider); ok {
					sel = *ai.DefaultProvider
				}
			}
		}
	}
	// The split fills the window below the header, as Rust's two panes do;
	// the AI group comes after it.
	_, winH := c.Size()
	mu.Row(c).AlignItems(mu.Stretch).MinHeight(max(520, winH-190)).BorderWidth(1, 0, 0, 0).BorderColor(p.Rule).Children(func() {
		mu.Column(c).Width(264).Shrink(0).Padding(16, 0, 16, 0).PaddingEnd(16).Gap(4).BorderEndWidth(1).BorderColor(p.Rule).Role(mu.RoleList).
			Label(i18n.T("settings.providers.your_providers")).Children(func() {
			groups := []string{i18n.T("settings.providers.group_subscriptions"), i18n.T("settings.providers.group_servers"),
				i18n.T("settings.providers.group_on_this_computer", "computer", computer())}
			for g, title := range groups {
				first := true
				// Apple Intelligence last in its group (providers/mod.rs grouped).
				order := make([]config.Provider, 0, len(ai.Providers))
				for _, apple := range []bool{false, true} {
					for _, pr := range ai.Providers {
						if (pr.Kind == config.KindAppleIntelligence) == apple {
							order = append(order, pr)
						}
					}
				}
				for _, pr := range order {
					if providerGroup(pr) != g {
						continue
					}
					if first {
						caps(c, p, title).Padding(10, 10, 6, 10)
						first = false
					}
					a.providerListRow(c, p, pr, pr.ID == sel)
				}
			}
			mu.Spacer(c)
			a.addProviderCard(c, p, sel == addProviderSel)
		})
		mu.Column(c).Grow(1).Basis(0).MinWidth(0).Padding(24, 0, 8, 0).PaddingStart(36).Children(func() {
			if sel == addProviderSel {
				a.addProviderForm(c, p)
				return
			}
			for i := range ai.Providers {
				if ai.Providers[i].ID == sel {
					a.providerDetail(c, p, i)
				}
			}
		})
	})
	a.group(c, p, i18n.T("settings.dictation.ai"), func() {
		a.toggleRow(c, p, "providers.style_rules", i18n.T("settings.dictation.styles_by_app_site"), i18n.T("settings.dictation.styles_by_app_site_detail"), &ai.StyleRules.Enabled, a.saveConfig)
		a.row(c, p, i18n.T("settings.dictation.ai_timeout"), i18n.T("settings.dictation.ai_timeout_detail"), func() {
			a.uintSelect(c, "providers.http_timeout", i18n.T("settings.dictation.ai_timeout"), &ai.HTTPTimeoutMs,
				[]uint64{2000, 4000, 8000, 15000, 30000}, secondsLabel)
		})
		a.row(c, p, i18n.T("settings.dictation.ai_timeout_cli"), i18n.T("settings.dictation.ai_timeout_cli_detail"), func() {
			a.uintSelect(c, "providers.cli_timeout", i18n.T("settings.dictation.ai_timeout_cli"), &ai.CLITimeoutMs,
				[]uint64{10000, 20000, 30000, 60000}, secondsLabel)
		})
	})
	a.group(c, p, i18n.T("settings.providers.group_on_this_computer", "computer", computer()), func() {
		for _, m := range enhance.CleanupCatalog() {
			ready := a.d.Paths.DataDir != "" && enhance.CleanupModelReady(a.d.Paths.ModelsDir(), &m)
			dl, active := a.set.cleanupDl[m.ID]
			downloading := active && !dl.failed
			detail := i18n.T("settings.providers.cleanup_not_downloaded")
			switch {
			case ready:
				detail = i18n.T("settings.providers.cleanup_ready", "computer", computer(), "languages", strings.Join(m.Languages, ", "))
			case downloading && cleanupFraction(dl.progress) < 0:
				detail = i18n.T("settings.providers.cleanup_downloading_bytes", "amount", models.FormatSize(dl.progress.BytesDone))
			case downloading:
				detail = i18n.T("settings.providers.cleanup_downloading", "percent", int(cleanupFraction(dl.progress)*100))
			case active:
				detail = i18n.T("settings.providers.cleanup_download_failed")
			}
			a.row(c, p, m.Name, detail, func() {
				switch {
				case ready:
				case downloading:
					track(c, p, "providers.cleanup.progress."+m.ID, cleanupFraction(dl.progress), 6,
						i18n.T("models.a11y.downloading", "name", m.Name)).Width(160).Shrink(0)
				default:
					if a.btn(c, p, "providers.cleanup."+m.ID, i18n.T("settings.providers.download"), btnSecondary) {
						a.downloadCleanup(m)
					}
				}
			})
		}
	})
}

func (a *App) providerListRow(c *mu.Context, p theme.Colors, pr config.Provider, on bool) {
	r := mu.ButtonBase(a.k(c, "providers.select."+pr.ID)).Label(pr.Name).Role(mu.RoleListItem).Selected(on).
		Padding(9, 10).Gap(10).Radius(12).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	if on {
		r.Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).Shadow(0, 1, 2, 0, p.ShadowA(0.14)).Shadow(0, 6, 16, 0, p.ShadowA(0.10))
	} else if r.Hovered() {
		r.Background(p.Deboss.Alpha(0.4))
	}
	r.Children(func() {
		logoTile(c, p, pr, 28)
		mu.Column(c).Gap(1).Grow(1).Basis(0).MinWidth(0).Children(func() {
			uiText(c, pr.Name, 14, 600, p.Ink).SingleLine().Ellipsis("…")
			uiText(c, a.providerSub(pr), 12, 400, p.Graphite).SingleLine().Ellipsis("…")
		})
		dot := p.Success
		if !a.providerReady(pr) {
			dot = p.Pencil
		}
		mu.Box(c).Size(7, 7).Radius(99).Background(dot)
	})
	if clicked(c, r) {
		a.set.provSel = pr.ID
		// The list has the names and the thinking levels of the models.
		a.loadProviderModels(pr.ID, false)
	}
}

func (a *App) addProviderCard(c *mu.Context, p theme.Colors, on bool) {
	b := mu.ButtonBase(a.k(c, "providers.select.add")).Label(i18n.T("settings.providers.add_a_provider")).Padding(12).Gap(12).Radius(12).
		AlignItems(mu.Center).Border(1, p.DebossShade).Cursor(mu.CursorPointer)
	if on {
		b.Background(p.Deboss.Alpha(0.5))
	} else if b.Hovered() {
		b.Background(p.Deboss.Alpha(0.35))
	}
	b.Children(func() {
		raisedSmall(mu.Box(c).Size(28, 28).Radius(7).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
			mu.Icon(c, icon("plus")).FontSize(14).TextColor(p.Ink)
		})
		mu.Column(c).Gap(1).Grow(1).Basis(0).Children(func() {
			uiText(c, i18n.T("settings.providers.add_a_provider"), 14, 600, p.Ink)
			uiText(c, i18n.T("settings.providers.add_a_provider_short"), 12, 400, p.Graphite).SingleLine().Ellipsis("…")
		})
	})
	if clicked(c, b) {
		a.set.provSel = addProviderSel
	}
}

// providerDetail is the right pane: the provider's head, its rows, and the
// footer with Remove.
func (a *App) providerDetail(c *mu.Context, p theme.Colors, i int) {
	ai := &a.cfg.AI
	pr := ai.Providers[i]
	isDefault := ai.DefaultProvider != nil && *ai.DefaultProvider == pr.ID
	mu.Row(c).Gap(16).AlignItems(mu.Center).Padding(0, 0, 20, 0).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		logoTile(c, p, pr, 52)
		mu.Column(c).Gap(4).Grow(1).Basis(0).MinWidth(0).Children(func() {
			mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
				heading(c, p, pr.Name, 30).LineHeight(1.15).SingleLine().Ellipsis("…")
				if isDefault {
					badge(c, p, i18n.T("settings.providers.default_badge"), toneMuted, "")
				}
			})
			mu.Row(c).Gap(6).AlignItems(mu.Center).Children(func() {
				dot, line := p.Success, a.providerSub(pr)
				if !a.providerReady(pr) {
					dot = p.Pencil
				}
				if pr.Kind == config.KindChatGPT && a.savedSecrets().chatGPT[pr.ID] {
					line = i18n.T("settings.providers.signed_in")
				} else if pr.IsCloud() && pr.Kind != config.KindChatGPT {
					line += " · " + i18n.T("hub.place.cloud")
				}
				mu.Box(c).Size(7, 7).Radius(99).Background(dot)
				uiText(c, line, 13, 400, p.Graphite).SingleLine().Ellipsis("…")
			})
		})
	})
	switch pr.Kind {
	case config.KindChatGPT:
		a.row(c, p, i18n.T("settings.providers.login"), i18n.T("settings.providers.chatgpt_note"), func() { a.chatGPTAccount(c, p, pr) })
	case config.KindOpenAICompatible:
		a.row(c, p, i18n.T("settings.providers.api_key"), a.keyLine(pr), func() { a.apiKeyRow(c, p, pr) })
	case config.KindClaudeCLI:
		a.row(c, p, i18n.T("settings.providers.config_folder"), i18n.T("settings.providers.config_folder_detail"), func() {
			dir := pr.ConfigDirSet()
			a.fieldW(c, p, "providers.config_dir."+pr.ID, i18n.T("settings.providers.config_folder"), "~/.claude", &dir, 240).OnChange(func() {
				a.editProvider(pr.ID, func(p *config.Provider) {
					p.ConfigDir = nil
					if strings.TrimSpace(dir) != "" {
						p.ConfigDir = ptr(dir)
					}
				})
				// Another folder is another login, with its own models.
				a.dropModels(pr.ID)
			})
		})
	}
	// detail.rs: every provider with a model to choose gets the picker and
	// the thinking levels of the chosen model. Apple Intelligence keeps its
	// name in the head, and a cleanup model has one fixed model.
	switch pr.Kind {
	case config.KindChatGPT, config.KindClaudeCLI, config.KindOpenAICompatible:
		a.row(c, p, i18n.T("settings.providers.model"), i18n.T("settings.providers.model_detail"), func() {
			a.providerModelPicker(c, p, pr)
		})
		a.thinkingRow(c, p, pr)
	}
	a.row(c, p, i18n.T("settings.providers.default_provider"), i18n.T("settings.providers.default_provider_detail"), func() {
		on := isDefault
		a.toggle(c, "providers.default."+pr.ID, i18n.T("settings.providers.default_provider"), &on, func() {
			if on {
				ai.DefaultProvider = ptr(pr.ID)
			} else if isDefault {
				ai.DefaultProvider = nil
			}
			a.saveConfig()
		})
	})
	users := a.stylesUsing(pr.ID, isDefault)
	detail := i18n.T("settings.providers.used_by_none")
	if len(users) > 0 {
		detail = i18n.TP("settings.providers.used_by_count", len(users), nil)
	}
	a.row(c, p, i18n.T("settings.providers.used_by"), detail, func() {
		mu.Row(c).Gap(6).AlignItems(mu.Center).Children(func() {
			for j, u := range users {
				if j == 3 {
					badge(c, p, fmt.Sprintf("+%d", len(users)-3), toneMuted, "")
					break
				}
				debossed(mu.Box(c).Height(22).PaddingX(8).Radius(6).Justify(mu.Center), p).Children(func() {
					uiText(c, u, 12, 600, p.Ink).SingleLine()
				})
			}
		})
	})
	mu.Row(c).Gap(12).Padding(28, 0, 0, 0).AlignItems(mu.Center).Children(func() {
		where := pr.BaseURL
		if pr.Path != nil {
			where = *pr.Path
		}
		mono(c, i18n.Isolate(where), 12, p.Graphite).Grow(1).Basis(0).Ellipsis("…")
		if a.button(c, p, "providers.remove."+pr.ID, i18n.T("settings.providers.remove_named", "name", pr.Name), bopt{kind: btnDanger}) {
			a.removeProvider(pr.ID)
		}
	})
	if a.set.provErr != "" {
		uiText(c, a.set.provErr, 13, 500, p.Danger).PaddingY(8)
	}
}

// keyLine says whether a provider's API key is saved.
func (a *App) keyLine(pr config.Provider) string {
	if a.savedSecrets().keys[keyAccount(pr)] {
		return i18n.TOS("settings.providers.key_saved", nil)
	}
	return i18n.T("settings.providers.no_key")
}

// stylesUsing names the styles that run on a provider.
func (a *App) stylesUsing(id string, isDefault bool) []string {
	var out []string
	for _, st := range a.styles.Styles() {
		if !st.UsesAI() {
			continue
		}
		if (st.Provider != nil && *st.Provider == id) || (st.Provider == nil && isDefault) {
			out = append(out, styleName(&st))
		}
	}
	return out
}

func secondsLabel(ms uint64) string {
	return i18n.T("settings.dictation.seconds", "seconds", i18n.Decimal(float64(ms)/1000, 0))
}

func (a *App) apiKeyRow(c *mu.Context, p theme.Colors, pr config.Provider) {
	if a.set.keyEdit == nil {
		a.set.keyEdit = map[string]string{}
	}
	account := keyAccount(pr)
	has := a.savedSecrets().keys[account]
	mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
		v := a.set.keyEdit[pr.ID]
		placeholder := i18n.TOS("settings.providers.key_placeholder", nil)
		if has {
			placeholder = "••••••••"
		}
		a.fieldW(c, p, "providers.key."+pr.ID, i18n.T("settings.providers.api_key"), placeholder, &v, 200).Password()
		a.set.keyEdit[pr.ID] = v
		label := i18n.T("settings.providers.add_key")
		if has {
			label = i18n.T("settings.providers.replace")
		}
		if a.btn(c, p, "providers.save_key."+pr.ID, label, btnSecondary) && a.d.Secrets != nil && strings.TrimSpace(v) != "" {
			if err := a.d.Secrets.Set(account, strings.TrimSpace(v)); err != nil {
				a.set.provErr = err.Error()
			} else {
				a.set.keyEdit[pr.ID] = ""
				a.dropModels(pr.ID)
				for j := range a.cfg.AI.Providers {
					if a.cfg.AI.Providers[j].ID == pr.ID {
						a.cfg.AI.Providers[j].APIKeyAccount = ptr(account)
					}
				}
				a.saveConfig()
				a.refreshSecrets()
			}
		}
	})
}

func (a *App) chatGPTAccount(c *mu.Context, p theme.Colors, pr config.Provider) {
	signedIn := a.savedSecrets().chatGPT[pr.ID]
	if signedIn {
		if a.btn(c, p, "providers.chatgpt.sign_out."+pr.ID, i18n.T("settings.providers.sign_out"), btnSecondary) && a.d.Secrets != nil {
			enhance.ChatGPTSignOut(a.d.Secrets, pr.ID)
			a.dropModels(pr.ID)
			a.refreshSecrets()
		}
		return
	}
	if a.btn(c, p, "providers.chatgpt.sign_in."+pr.ID, i18n.T("settings.providers.sign_in"), btnPrimary) && a.d.Secrets != nil {
		a.startChatGPTSignIn(pr.ID)
	}
}

func (a *App) startChatGPTSignIn(id string) {
	texts := enhance.SignInTexts{DoneTitle: i18n.T("enhance.sign_in_done_title"), DoneBody: i18n.T("enhance.sign_in_done_body"),
		FailedTitle: i18n.T("enhance.sign_in_failed_title"), FailedBody: i18n.T("enhance.sign_in_failed_body")}
	s, err := enhance.StartChatGPTSignIn(a.d.Secrets, id, 10*time.Minute, nil, texts)
	if err != nil {
		a.set.provErr = i18n.T("app.sign_in.start_failed", "error", err.Error())
		return
	}
	go func() {
		for ev := range s.Events {
			ev := ev
			a.post(func() {
				if ev.URL != "" {
					a.openURL(ev.URL)
				}
				if ev.Finished != nil && ev.Finished.State != "signed_in" {
					a.set.provErr = i18n.T("app.sign_in.not_finished")
				}
				if ev.Finished != nil {
					a.refreshSecrets()
				}
				if ev.Finished != nil && ev.Finished.State == "signed_in" {
					// The names of the models come with the list.
					a.loadProviderModels(id, true)
				}
				a.invalidate()
			})
		}
	}()
}

func (a *App) removeProvider(id string) {
	ai := &a.cfg.AI
	out := ai.Providers[:0:0]
	for _, pr := range ai.Providers {
		if pr.ID == id {
			if pr.APIKeyAccount != nil && a.d.Secrets != nil {
				a.d.Secrets.Delete(*pr.APIKeyAccount)
			}
			continue
		}
		out = append(out, pr)
	}
	ai.Providers = out
	a.dropModels(id)
	if ai.DefaultProvider != nil && *ai.DefaultProvider == id {
		ai.DefaultProvider = nil
	}
	a.saveConfig()
	a.refreshSecrets()
}

// addProviderForm adds an OpenAI-compatible provider, or a ChatGPT one.
func (a *App) addProviderForm(c *mu.Context, p theme.Colors) {
	s := &a.set
	mu.Column(c).Gap(14).Children(func() {
		heading(c, p, i18n.T("settings.providers.add_a_provider"), 30).LineHeight(1.15)
		uiText(c, i18n.T("settings.providers.add_a_provider_detail"), 14, 400, p.Graphite).LineHeight(1.4)
		a.labelled(c, p, i18n.T("settings.providers.name"), func() {
			a.field(c, p, "providers.new.name", i18n.T("settings.providers.name"), i18n.T("settings.providers.name_placeholder"), &s.provName)
		})
		a.labelled(c, p, i18n.T("settings.providers.base_url"), func() {
			ltr(a.field(c, p, "providers.new.url", i18n.T("settings.providers.base_url"), "https://openrouter.ai/api/v1", &s.provURL))
		})
		a.labelled(c, p, i18n.T("settings.providers.api_key"), func() {
			a.field(c, p, "providers.new.key", i18n.T("settings.providers.api_key"), i18n.TOS("settings.providers.key_placeholder", nil), &s.provKey).Password()
		})
		a.labelled(c, p, i18n.T("settings.providers.model"), func() {
			a.field(c, p, "providers.new.model", i18n.T("settings.providers.model"), i18n.T("settings.providers.choose_model"), &s.provModel)
		})
		if s.provErr != "" {
			uiText(c, s.provErr, 13, 500, p.Danger)
		}
		mu.Row(c).Gap(8).Children(func() {
			if a.button(c, p, "providers.add", i18n.T("settings.providers.add_provider"), bopt{kind: btnPrimary, icon: "plus"}) {
				a.AddProvider()
			}
			if a.btn(c, p, "providers.add_chatgpt", i18n.T("settings.providers.gallery_chatgpt"), btnSecondary) {
				a.addChatGPT()
			}
		})
	})
}

// AddProvider adds the provider of the form.
func (a *App) AddProvider() {
	s := &a.set
	name, url := strings.TrimSpace(s.provName), strings.TrimSpace(s.provURL)
	switch {
	case name == "":
		s.provErr = i18n.T("settings.providers.error_no_name")
		return
	case !strings.HasPrefix(url, "http://") && !strings.HasPrefix(url, "https://"):
		s.provErr = i18n.T("settings.providers.error_bad_url")
		return
	}
	id := styleID(name)
	if id == "" {
		id = "provider"
	}
	for n := 2; ; n++ {
		if _, taken := a.cfg.AI.Provider(id); !taken {
			break
		}
		id = fmt.Sprintf("%s-%d", styleID(name), n)
	}
	pr := config.Provider{ID: id, Name: name, Kind: config.KindOpenAICompatible, BaseURL: url}
	if m := strings.TrimSpace(s.provModel); m != "" {
		pr.SetModel(m)
	}
	if k := strings.TrimSpace(s.provKey); k != "" && a.d.Secrets != nil {
		account := "provider." + id
		if err := a.d.Secrets.Set(account, k); err != nil {
			s.provErr = err.Error()
			return
		}
		pr.APIKeyAccount = ptr(account)
	}
	a.cfg.AI.Providers = append(a.cfg.AI.Providers, pr)
	if a.cfg.AI.DefaultProvider == nil {
		a.cfg.AI.DefaultProvider = ptr(id)
	}
	a.cfg.AI.Enabled = true
	*s = settingsState{name: s.name, enginePath: s.enginePath, engineEdited: s.engineEdited, keyEdit: s.keyEdit, devices: s.devices, devicesRead: s.devicesRead, provSel: id}
	a.saveConfig()
	a.refreshSecrets()
	a.invalidate()
}

func (a *App) addChatGPT() {
	id := "chatgpt"
	if _, ok := a.cfg.AI.Provider(id); ok {
		return
	}
	a.cfg.AI.Providers = append(a.cfg.AI.Providers, config.Provider{ID: id, Name: "ChatGPT", Kind: config.KindChatGPT})
	a.set.provSel = id
	if a.cfg.AI.DefaultProvider == nil {
		a.cfg.AI.DefaultProvider = ptr(id)
	}
	a.saveConfig()
	a.refreshSecrets()
}

// secretSnapshot is what Settings › Providers knows of the saved secrets.
// A Keychain read can wait on a system prompt, so refreshSecrets reads them
// on another goroutine and the views read only this copy.
type secretSnapshot struct {
	keys    map[string]bool // account → an API key is saved
	chatGPT map[string]bool // provider id → signed in
}

// secretsCache holds the latest secretSnapshot; gen drops the answer of an
// older refresh that finishes after a newer one.
type secretsCache struct {
	mu   sync.Mutex
	gen  uint64
	snap secretSnapshot
}

func keyAccount(pr config.Provider) string {
	if pr.APIKeyAccount != nil {
		return *pr.APIKeyAccount
	}
	return "provider." + pr.ID
}

// savedSecrets is the last snapshot (never a Keychain read).
func (a *App) savedSecrets() secretSnapshot {
	a.secrets.mu.Lock()
	defer a.secrets.mu.Unlock()
	return a.secrets.snap
}

// refreshSecrets reads, off the UI thread, which providers have a saved key
// or a ChatGPT sign-in, then redraws.
func (a *App) refreshSecrets() {
	store := a.d.Secrets
	if store == nil {
		return
	}
	var accounts, chatGPT []string
	for _, pr := range a.cfg.AI.Providers {
		switch pr.Kind {
		case config.KindChatGPT:
			chatGPT = append(chatGPT, pr.ID)
		case config.KindOpenAICompatible:
			accounts = append(accounts, keyAccount(pr))
		}
	}
	a.secrets.mu.Lock()
	a.secrets.gen++
	gen := a.secrets.gen
	a.secrets.mu.Unlock()
	go func() {
		snap := secretSnapshot{keys: map[string]bool{}, chatGPT: map[string]bool{}}
		for _, acc := range accounts {
			snap.keys[acc] = store.Get(acc) != ""
		}
		for _, id := range chatGPT {
			snap.chatGPT[id] = enhance.ChatGPTStatus(store, id).State == "signed_in"
		}
		a.secrets.mu.Lock()
		fresh := a.secrets.gen == gen
		if fresh {
			a.secrets.snap = snap
		}
		a.secrets.mu.Unlock()
		if fresh {
			a.post(a.invalidate)
		}
	}()
}

// cleanupFraction is how far a cleanup model download is, or -1 while
// its size is not known.
func cleanupFraction(d enhance.DownloadProgress) float32 {
	if d.BytesTotal == 0 {
		return -1
	}
	return min(float32(float64(d.BytesDone)/float64(d.BytesTotal)), 1)
}

func (a *App) downloadCleanup(m enhance.CleanupModelInfo) {
	dir := a.d.Paths.ModelsDir()
	if a.d.Paths.DataDir == "" {
		return
	}
	if dl, ok := a.set.cleanupDl[m.ID]; ok && !dl.failed {
		return
	}
	if a.set.cleanupDl == nil {
		a.set.cleanupDl = map[string]cleanupDownload{}
	}
	a.set.cleanupDl[m.ID] = cleanupDownload{progress: enhance.DownloadProgress{BytesTotal: m.SizeBytes}}
	a.invalidate()
	go func() {
		_, err := enhance.DownloadCleanupModel(dir, &m, "", nil, func(d enhance.DownloadProgress) {
			a.post(func() {
				if dl, ok := a.set.cleanupDl[m.ID]; ok && !dl.failed {
					a.set.cleanupDl[m.ID] = cleanupDownload{progress: d}
				}
				a.invalidate()
			})
		})
		a.post(func() {
			if err != nil {
				a.set.provErr = i18n.T("settings.providers.cleanup_download_failed_detail", "name", m.Name)
				a.set.cleanupDl[m.ID] = cleanupDownload{failed: true}
			} else {
				delete(a.set.cleanupDl, m.ID)
			}
			a.invalidate()
		})
	}()
}

// ---- Connected apps ----

func (a *App) appsSection(c *mu.Context, p theme.Colors) {
	if a.d.Apps == nil {
		uiText(c, i18n.T("apps.settings.empty"), 15, 400, p.Graphite).PaddingY(4)
		return
	}
	if a.d.Apps.IsBroken() {
		uiText(c, i18n.T("apps.settings.broken", "path", a.d.Apps.Path()), 13, 500, p.Danger)
	}
	apps := a.d.Apps.Current()
	if apps == nil || len(apps.Apps) == 0 {
		// connected_apps.rs: a plain line, no group.
		uiText(c, i18n.T("apps.settings.empty"), 15, 400, p.Graphite).PaddingY(4)
		return
	}
	a.group(c, p, i18n.T("apps.settings.title"), func() {
		for i := range apps.Apps {
			rec := &apps.Apps[i]
			id := rec.ID()
			mu.Column(c).Gap(10).PaddingY(14).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
				mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
					a.appBadge(c, rec.ShownName(), "", 28)
					mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
						uiText(c, rec.ShownName(), 15, 600, p.Ink)
						status := i18n.T("apps.settings.not_allowed")
						if rec.Allowed {
							status = i18n.T("settings.common.allowed")
						}
						uiText(c, status, 13, 400, p.Graphite)
					})
					if !rec.Allowed && a.btn(c, p, "apps.allow."+id, i18n.T("apps.settings.allow"), btnPrimary) {
						a.appsErr(a.d.Apps.Allow(id))
					}
					if a.btn(c, p, "apps.revoke."+id, i18n.T("apps.settings.remove"), btnDanger) {
						a.appsErr(a.d.Apps.Remove(id))
						if a.d.Host != nil {
							a.d.Host.StopWorkWithoutGrant()
						}
					}
				})
				caps := rec.Capabilities()
				if len(caps) == 0 {
					uiText(c, i18n.T("apps.settings.no_capabilities"), 13, 400, p.Graphite).PaddingStart(40)
				}
				mu.Row(c).Gap(6).Wrap().PaddingStart(40).Children(func() {
					for _, cp := range caps {
						if a.chip(c, p, "apps.grant."+id+"."+string(cp), capabilityLabel(cp), true) {
							a.appsErr(a.d.Apps.SetGrant(id, cp, false))
						}
					}
				})
			})
		}
	})
}

func (a *App) appsErr(err error) {
	if err != nil {
		a.flash = i18n.T("apps.settings.save_failed", "path", a.d.Apps.Path())
	}
	a.invalidate()
}

func capabilityLabel(cp bridge.Capability) string {
	return i18n.T("apps.capability." + strings.ReplaceAll(string(cp), ".", "_"))
}

// ---- Updates ----

func (a *App) updatesSection(c *mu.Context, p theme.Colors) {
	u := &a.cfg.Updates
	a.group(c, p, i18n.T("settings.general.updates"), func() {
		if !update.IsOfficialBuild() {
			a.row(c, p, i18n.T("settings.general.no_self_update"), i18n.T("settings.general.no_self_update_detail", "version", a.d.Version), nil)
		}
		a.toggleRow(c, p, "updates.check", i18n.T("settings.general.check_updates"), i18n.T("settings.general.check_updates_detail"), &u.Check, func() {
			a.saveConfig()
			if a.d.Updater != nil {
				a.d.Updater.SetAutoCheck(u.Check)
			}
		})
		a.toggleRow(c, p, "updates.automatic", i18n.T("settings.general.install_automatically"), i18n.T("settings.general.install_automatically_detail"), &u.Automatic, func() {
			a.saveConfig()
			if a.d.Updater != nil {
				a.d.Updater.SetAutomatic(u.Automatic)
			}
		})
		a.row(c, p, a.updateHeadline(), a.updateDetail(), func() {
			a.updateButtons(c, p, "updates", true)
		})
		if a.upd.status.Kind == update.StatusDownloading {
			a.updateTrack(c, p, "updates.progress", 6).Margin(10, 0, 4, 0)
		}

	})
}

// thinkingRow picks the thinking level of a provider: the levels of the
// chosen model from its list (model.rs thinking_choices). A model without
// levels has no row.
func (a *App) thinkingRow(c *mu.Context, p theme.Colors, pr config.Provider) {
	levels := a.thinkingChoices(pr)
	if len(levels) == 0 {
		return
	}
	sent := pr.ThinkingSent()
	keys, labels := make([]string, len(levels)), make([]string, len(levels))
	chosen := 0
	for i, l := range levels {
		keys[i], labels[i] = "providers.thinking."+pr.ID+"."+l, config.ThinkingLabel(l)
		if l == sent {
			chosen = i
		}
	}
	a.row(c, p, i18n.T("settings.providers.thinking"), i18n.T("settings.providers.thinking_detail"), func() {
		if i := a.choice(c, p, keys, labels, chosen, false); i >= 0 {
			a.editProvider(pr.ID, func(p *config.Provider) { p.SetThinking(ptr(levels[i])) })
		}
	})
}
