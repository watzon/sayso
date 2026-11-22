package ui

import (
	"fmt"
	"strings"
	"sync"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
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

func (a *App) providersSection(c *mu.Context, p theme.Colors) {
	ai := &a.cfg.AI
	a.group(c, p, "", func() {
		a.toggleRow(c, p, "providers.ai_enabled", i18n.T("settings.providers.use_ai"), i18n.T("settings.providers.use_ai_short"), &ai.Enabled, a.saveConfig)
		a.toggleRow(c, p, "providers.style_rules", i18n.T("settings.dictation.styles_by_app_site"), i18n.T("settings.dictation.styles_by_app_site_detail"), &ai.StyleRules.Enabled, a.saveConfig)
	})
	a.group(c, p, i18n.T("settings.providers.your_providers"), func() {
		if len(ai.Providers) == 0 {
			mu.Text(c, i18n.T("settings.providers.start_body")).FontSize(13).TextColor(p.Graphite).Padding(12, 0)
		}
		for i := range ai.Providers {
			a.providerRow(c, p, i)
		}
	})
	a.addProviderForm(c, p)
	a.group(c, p, i18n.T("settings.providers.group_on_this_computer", "computer", computer()), func() {
		for _, m := range enhance.CleanupCatalog() {
			ready := a.d.Paths.DataDir != "" && enhance.CleanupModelReady(a.d.Paths.ModelsDir(), &m)
			detail := i18n.T("settings.providers.cleanup_not_downloaded")
			if ready {
				detail = i18n.T("settings.providers.cleanup_ready", "computer", computer(), "languages", strings.Join(m.Languages, ", "))
			}
			a.row(c, p, m.Name, detail, func() {
				if ready {
					return
				}
				if a.btn(c, p, "providers.cleanup."+m.ID, i18n.T("settings.providers.download"), btnSecondary) {
					a.downloadCleanup(m)
				}
			})
		}
	})
	a.group(c, p, i18n.T("settings.dictation.ai"), func() {
		a.row(c, p, i18n.T("settings.dictation.ai_timeout"), i18n.T("settings.dictation.ai_timeout_detail"), func() {
			a.uintSelect(c, "providers.http_timeout", i18n.T("settings.dictation.ai_timeout"), &ai.HTTPTimeoutMs,
				[]uint64{2000, 4000, 8000, 15000, 30000}, secondsLabel)
		})
		a.row(c, p, i18n.T("settings.dictation.ai_timeout_cli"), i18n.T("settings.dictation.ai_timeout_cli_detail"), func() {
			a.uintSelect(c, "providers.cli_timeout", i18n.T("settings.dictation.ai_timeout_cli"), &ai.CLITimeoutMs,
				[]uint64{10000, 20000, 30000, 60000}, secondsLabel)
		})
	})
}

func secondsLabel(ms uint64) string {
	return i18n.T("settings.dictation.seconds", "seconds", i18n.Decimal(float64(ms)/1000, 0))
}

func (a *App) providerRow(c *mu.Context, p theme.Colors, i int) {
	ai := &a.cfg.AI
	pr := ai.Providers[i]
	isDefault := ai.DefaultProvider != nil && *ai.DefaultProvider == pr.ID
	place := i18n.T("hub.place.local")
	if pr.IsCloud() {
		place = i18n.T("hub.place.cloud")
	}
	detail := place
	switch pr.Kind {
	case config.KindChatGPT:
		detail = i18n.T("settings.providers.not_signed_in")
		if a.savedSecrets().chatGPT[pr.ID] {
			detail = i18n.T("settings.providers.signed_in")
		}
	default:
		if pr.ModelName() != "" {
			detail += " · " + pr.ModelName()
		}
	}
	mu.Column(c).Gap(8).Padding(12, 0).Children(func() {
		mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
			mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
				mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
					mu.Text(c, pr.Name).FontSize(14).FontWeight(600).TextColor(p.Ink)
					if isDefault {
						slip(c, p, i18n.T("settings.providers.default_badge"), p.Accent)
					}
				})
				mu.Text(c, detail).FontSize(12.5).TextColor(p.Graphite)
			})
			if !isDefault && a.btn(c, p, "providers.default."+pr.ID, i18n.T("settings.providers.default_provider"), btnGhost) {
				ai.DefaultProvider = ptr(pr.ID)
				a.saveConfig()
			}
			if a.btn(c, p, "providers.remove."+pr.ID, i18n.T("settings.providers.remove"), btnDanger) {
				a.removeProvider(pr.ID)
			}
		})
		switch pr.Kind {
		case config.KindChatGPT:
			a.chatGPTAccount(c, p, pr)
		case config.KindOpenAICompatible:
			a.apiKeyRow(c, p, pr)
			model := pr.ModelName()
			a.field(c, p, "providers.model."+pr.ID, i18n.T("settings.providers.model"), i18n.T("settings.providers.choose_model"), &model).OnChange(func() {
				for j := range ai.Providers {
					if ai.Providers[j].ID == pr.ID {
						ai.Providers[j].SetModel(model)
					}
				}
				a.saveConfig()
			})
		}
	})
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
		mu.TextInput(a.k(c, "providers.key."+pr.ID), &v).Label(i18n.T("settings.providers.api_key")).Placeholder(placeholder).Password().Grow(1)
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
	if has {
		mu.Text(c, i18n.TOS("settings.providers.key_saved", nil)).FontSize(12).TextColor(p.Pencil)
	}
}

func (a *App) chatGPTAccount(c *mu.Context, p theme.Colors, pr config.Provider) {
	signedIn := a.savedSecrets().chatGPT[pr.ID]
	mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
		mu.Text(c, i18n.T("settings.providers.chatgpt_account_detail")).FontSize(12.5).TextColor(p.Graphite).Grow(1).Basis(0)
		if signedIn {
			if a.btn(c, p, "providers.chatgpt.sign_out."+pr.ID, i18n.T("settings.providers.sign_out"), btnSecondary) && a.d.Secrets != nil {
				enhance.ChatGPTSignOut(a.d.Secrets, pr.ID)
				a.refreshSecrets()
			}
			return
		}
		if a.btn(c, p, "providers.chatgpt.sign_in."+pr.ID, i18n.T("settings.providers.sign_in"), btnPrimary) && a.d.Secrets != nil {
			a.startChatGPTSignIn(pr.ID)
		}
	})
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
	if ai.DefaultProvider != nil && *ai.DefaultProvider == id {
		ai.DefaultProvider = nil
	}
	a.saveConfig()
	a.refreshSecrets()
}

// addProviderForm adds an OpenAI-compatible provider, or a ChatGPT one.
func (a *App) addProviderForm(c *mu.Context, p theme.Colors) {
	s := &a.set
	a.group(c, p, i18n.T("settings.providers.add_a_provider"), func() {
		mu.Text(c, i18n.T("settings.providers.add_a_provider_detail")).FontSize(12.5).TextColor(p.Graphite).Padding(10, 0, 0, 0)
		mu.Column(c).Gap(8).Padding(10, 0).Children(func() {
			a.field(c, p, "providers.new.name", i18n.T("settings.providers.name"), i18n.T("settings.providers.name_placeholder"), &s.provName)
			a.field(c, p, "providers.new.url", i18n.T("settings.providers.base_url"), "https://openrouter.ai/api/v1", &s.provURL)
			mu.TextInput(a.k(c, "providers.new.key"), &s.provKey).Label(i18n.T("settings.providers.api_key")).
				Placeholder(i18n.TOS("settings.providers.key_placeholder", nil)).Password()
			a.field(c, p, "providers.new.model", i18n.T("settings.providers.model"), i18n.T("settings.providers.choose_model"), &s.provModel)
			if s.provErr != "" {
				mu.Text(c, s.provErr).FontSize(12.5).TextColor(p.Danger)
			}
			mu.Row(c).Gap(8).Children(func() {
				if a.btn(c, p, "providers.add", i18n.T("settings.providers.add_provider"), btnPrimary) {
					a.AddProvider()
				}
				if a.btn(c, p, "providers.add_chatgpt", i18n.T("settings.providers.gallery_chatgpt"), btnSecondary) {
					a.addChatGPT()
				}
			})
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
	*s = settingsState{name: s.name, enginePath: s.enginePath, keyEdit: s.keyEdit, devices: s.devices, devicesRead: s.devicesRead}
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

func (a *App) downloadCleanup(m enhance.CleanupModelInfo) {
	dir := a.d.Paths.ModelsDir()
	if a.d.Paths.DataDir == "" {
		return
	}
	go func() {
		_, err := enhance.DownloadCleanupModel(dir, &m, "", nil, func(enhance.DownloadProgress) { a.post(a.invalidate) })
		a.post(func() {
			if err != nil {
				a.set.provErr = i18n.T("settings.providers.cleanup_download_failed_detail", "name", m.Name)
			}
			a.invalidate()
		})
	}()
}

// ---- Connected apps ----

func (a *App) appsSection(c *mu.Context, p theme.Colors) {
	if a.d.Apps == nil {
		mu.Text(c, i18n.T("apps.settings.empty")).FontSize(13).TextColor(p.Graphite)
		return
	}
	if a.d.Apps.IsBroken() {
		mu.Text(c, i18n.T("apps.settings.broken", "path", a.d.Apps.Path())).FontSize(13).TextColor(p.Danger)
	}
	apps := a.d.Apps.Current()
	if apps == nil || len(apps.Apps) == 0 {
		card(c, p).Children(func() { mu.Text(c, i18n.T("apps.settings.empty")).FontSize(13).TextColor(p.Graphite) })
		return
	}
	a.group(c, p, i18n.T("apps.settings.title"), func() {
		for i := range apps.Apps {
			rec := &apps.Apps[i]
			id := rec.ID()
			mu.Column(c).Gap(6).Padding(12, 0).Children(func() {
				mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
					mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
						mu.Text(c, rec.ShownName()).FontSize(14).FontWeight(600).TextColor(p.Ink)
						status := i18n.T("apps.settings.not_allowed")
						if rec.Allowed {
							status = i18n.T("settings.common.allowed")
						}
						mu.Text(c, status).FontSize(12.5).TextColor(p.Graphite)
					})
					if !rec.Allowed && a.btn(c, p, "apps.allow."+id, i18n.T("apps.settings.allow"), btnSecondary) {
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
					mu.Text(c, i18n.T("apps.settings.no_capabilities")).FontSize(12).TextColor(p.Pencil)
				}
				for _, cp := range caps {
					on := true
					mu.Checkbox(a.k(c, "apps.grant."+id+"."+string(cp)), &on, capabilityLabel(cp)).OnChange(func() {
						a.appsErr(a.d.Apps.SetGrant(id, cp, on))
					})
				}
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
			if a.btn(c, p, "updates.check_now", i18n.T("update.action.check"), btnSecondary) {
				a.CheckForUpdates()
			}
		})
	})
}
