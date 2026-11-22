package ui

import (
	"fmt"
	"log"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/theme"
	"github.com/watzon/sayso/internal/update"
)

// Secondary windows: onboarding, the permissions guide, What's New, the
// update window and the app approval dialog. Each is a view of App; the
// shell makes the window.

// Secondary window sizes, in DIPs.
const (
	OnboardingW, OnboardingH = 720, 620
	WhatsNewW, WhatsNewH     = 520, 600
	UpdateW, UpdateH         = 460, 300
	ApprovalW, ApprovalH     = 460, 420
	PermissionsW             = 520
	PermissionsH             = 440
)

func (a *App) showWindow(w Window, show bool) {
	if a.d.Shell != nil {
		a.d.Shell.ShowWindow(w, show)
	}
}

// secondaryFrame is the paper sheet every secondary window draws on.
func (a *App) secondaryFrame(c *mu.Context, fn func(p theme.Colors)) {
	p := a.look(c)
	c.Root().Background(p.Sheet)
	bar := c.TitleBar()
	mu.Column(c).Fill().Padding(max(bar.Bottom, 0)+24, 28, 24, 28).Gap(14).Children(func() { fn(p) })
}

// ---- Onboarding ----

// OnboardingSteps is the number of onboarding steps.
const OnboardingSteps = 7

type onboardingState struct {
	step int
}

// NeedsOnboarding is true until the user finished onboarding.
func (a *App) NeedsOnboarding() bool { return !a.cfg.Onboarding.Completed }

// OnboardingStep is the step shown, from 0.
func (a *App) OnboardingStep() int { return a.onb.step }

func (a *App) setOnboardingStep(s int) {
	a.onb.step = min(max(s, 0), OnboardingSteps-1)
	a.cfg.Onboarding.Step = uint8(a.onb.step)
	a.saveConfig()
	a.invalidate()
}

// FinishOnboarding closes onboarding for good.
func (a *App) FinishOnboarding() {
	a.cfg.Onboarding.Completed = true
	a.saveConfig()
	// A new user has nothing to compare with: What's New waits for the next version.
	if a.d.Paths.DataDir != "" {
		if err := update.MarkWhatsNewSeen(a.d.Paths.StateFile()); err != nil {
			log.Printf("whats new: %v", err)
		}
	}
	a.showWindow(WinOnboarding, false)
	if a.d.Shell != nil {
		a.d.Shell.ShowHub()
	}
	a.invalidate()
}

// OnboardingView is the view of the onboarding window.
func (a *App) OnboardingView(c *mu.Context) {
	a.secondaryFrame(c, func(p theme.Colors) {
		eyebrow(c, p, i18n.T("onboarding.step", "step", a.onb.step+1, "total", OnboardingSteps))
		mu.Column(c).Grow(1).Gap(14).Children(func() {
			switch a.onb.step {
			case 0:
				a.onbWelcome(c, p)
			case 1:
				a.permissionsBody(c, p, true)
			case 2:
				a.onbModel(c, p)
			case 3:
				a.onbHotkeys(c, p)
			case 4:
				a.onbAI(c, p)
			case 5:
				a.onbPractice(c, p)
			default:
				a.onbDone(c, p)
			}
		})
		mu.Text(c, i18n.T("onboarding.footer.hint")).FontSize(12).TextColor(p.Pencil)
		mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
			if a.onb.step > 0 && a.btn(c, p, "onboarding.back", i18n.T("onboarding.footer.back"), btnGhost) {
				a.setOnboardingStep(a.onb.step - 1)
			}
			mu.Spacer(c)
			last := a.onb.step == OnboardingSteps-1
			if !last && a.btn(c, p, "onboarding.skip", i18n.T("onboarding.footer.skip"), btnGhost) {
				a.setOnboardingStep(a.onb.step + 1)
			}
			next := i18n.T("onboarding.footer.continue")
			switch {
			case a.onb.step == 0:
				next = i18n.T("onboarding.footer.get_started")
			case last:
				next = i18n.T("onboarding.footer.open_sayso")
			}
			if a.btn(c, p, "onboarding.next", next, btnPrimary) {
				if last {
					a.FinishOnboarding()
				} else {
					a.setOnboardingStep(a.onb.step + 1)
				}
			}
		})
	})
}

func (a *App) onbWelcome(c *mu.Context, p theme.Colors) {
	heading(c, p, i18n.T("onboarding.welcome.title"), 34)
	mu.Text(c, i18n.TOS("onboarding.welcome.lead", nil)).FontSize(14.5).TextColor(p.Graphite).LineHeight(1.45)
	for _, f := range []string{i18n.TOS("onboarding.welcome.fact_engine", nil), i18n.T("onboarding.welcome.fact_every_app"), i18n.T("onboarding.welcome.fact_setup_time")} {
		mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
			mu.Icon(c, icon("check")).FontSize(14).TextColor(p.Success)
			mu.Text(c, f).FontSize(13.5).TextColor(p.Ink)
		})
	}
	a.row(c, p, i18n.T("onboarding.welcome.interface_language"), "", func() { a.languageSelect(c, "onboarding.language") })
}

func (a *App) onbModel(c *mu.Context, p theme.Colors) {
	heading(c, p, i18n.T("onboarding.model.title"), 30)
	mu.Text(c, i18n.T("onboarding.model.lead")).FontSize(14).TextColor(p.Graphite)
	if a.d.Engine == nil {
		mu.Text(c, i18n.T("onboarding.model.no_engine")).FontSize(13).TextColor(p.Danger)
	}
	id := a.FinalModel()
	m, _ := models.Find(id)
	card(c, p).Children(func() {
		mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
			mu.Text(c, m.Name).Font(theme.Display).FontSize(20).FontWeight(600).TextColor(p.Ink).Grow(1)
			slip(c, p, i18n.T("onboarding.model.recommended"), p.Success)
		})
		if m.DescriptionKey != "" {
			mu.Text(c, i18n.T(m.DescriptionKey)).FontSize(13).TextColor(p.Graphite)
		}
		st := a.modelStatus(id)
		mu.Text(c, a.statusText(id)).FontSize(12.5).TextColor(p.Pencil)
		if st.State == "downloading" {
			mu.Progress(c, float64(st.Fraction)).Height(4)
		}
		if !st.IsOnDisk() && st.State != "downloading" && a.btn(c, p, "onboarding.download", i18n.T("onboarding.model.download_size", "size", models.FormatSize(m.SizeBytes)), btnPrimary) {
			a.downloadModel(id)
		}
	})
	if a.btn(c, p, "onboarding.all_models", i18n.T("onboarding.model.different_model"), btnGhost) {
		a.SetPage(PageModels)
		if a.d.Shell != nil {
			a.d.Shell.ShowHub()
		}
	}
}

func (a *App) onbHotkeys(c *mu.Context, p theme.Colors) {
	heading(c, p, i18n.T("onboarding.hotkeys.title"), 30)
	mu.Text(c, i18n.T("onboarding.hotkeys.lead")).FontSize(14).TextColor(p.Graphite)
	h := &a.cfg.Hotkeys
	a.row(c, p, i18n.T("onboarding.hotkeys.start_stop"), i18n.T("settings.dictation.toggle_detail"), func() {
		a.recorder(c, p, "onboarding.hotkey.toggle", i18n.T("onboarding.hotkeys.start_stop"), h.Toggle, func(n *config.Hotkey) {
			h.Toggle = n
			a.hotkeysChanged()
		})
	})
	a.row(c, p, i18n.T("onboarding.hotkeys.push_to_talk"), i18n.T("onboarding.hotkeys.works_everywhere"), func() {
		a.recorder(c, p, "onboarding.hotkey.push_to_talk", i18n.T("onboarding.hotkeys.push_to_talk"), h.PushToTalk, func(n *config.Hotkey) {
			h.PushToTalk = n
			a.hotkeysChanged()
		})
	})
}

func (a *App) onbAI(c *mu.Context, p theme.Colors) {
	heading(c, p, i18n.T("onboarding.ai.title"), 30)
	mu.Text(c, i18n.T("onboarding.ai.lead")).FontSize(14).TextColor(p.Graphite)
	a.addProviderForm(c, p)
}

func (a *App) onbPractice(c *mu.Context, p theme.Colors) {
	heading(c, p, i18n.T("onboarding.practice.title"), 30)
	lead := i18n.T("onboarding.practice.lead_push")
	if h := a.cfg.Hotkeys.Toggle; h != nil {
		lead = i18n.T("onboarding.practice.lead_toggle", "keys", h.CompactLabel())
	}
	mu.Text(c, lead).FontSize(14).TextColor(p.Graphite)
	card(c, p).Children(func() {
		eyebrow(c, p, i18n.T("onboarding.practice.read_this"))
		mu.Text(c, i18n.T("styles.featured.written")).Font(theme.Body).FontSize(15).TextColor(p.Ink)
	})
	mu.TextArea(a.k(c, "onboarding.practice"), &a.onbPracticeText).Label(i18n.T("onboarding.practice.field_label")).
		Placeholder(i18n.T("onboarding.practice.placeholder")).Height(90)
	if !a.engineRunning() {
		mu.Text(c, i18n.T("onboarding.practice.not_ready_engine")).FontSize(12.5).TextColor(p.Danger)
	}
}

func (a *App) onbDone(c *mu.Context, p theme.Colors) {
	heading(c, p, i18n.T("onboarding.done.title"), 32)
	lead := i18n.T("onboarding.done.lead_none")
	if h := a.cfg.Hotkeys.Toggle; h != nil {
		lead = i18n.T("onboarding.done.lead_toggle", "keys", h.CompactLabel())
	}
	mu.Text(c, lead).FontSize(14).TextColor(p.Graphite)
	for _, t := range [][2]string{
		{i18n.T("onboarding.done.pill_title"), i18n.T("onboarding.done.pill_body")},
		{i18n.TOS("onboarding.done.menu_title", nil), i18n.T("onboarding.done.menu_body")},
		{i18n.T("onboarding.done.paste_title"), i18n.T("onboarding.done.paste_body")},
	} {
		mu.Column(c).Gap(2).Children(func() {
			mu.Text(c, t[0]).FontSize(14).FontWeight(600).TextColor(p.Ink)
			mu.Text(c, t[1]).FontSize(13).TextColor(p.Graphite)
		})
	}
}

// ---- Permissions guide ----

// PermissionsView is the view of the permissions guide window.
func (a *App) PermissionsView(c *mu.Context) {
	a.secondaryFrame(c, func(p theme.Colors) {
		a.permissionsBody(c, p, false)
		mu.Spacer(c)
		mu.Row(c).Justify(mu.End).Children(func() {
			if a.btn(c, p, "permissions.close", i18n.T("whats_new.done"), btnPrimary) {
				a.showWindow(WinPermissions, false)
			}
		})
	})
}

func (a *App) permissionsBody(c *mu.Context, p theme.Colors, onboarding bool) {
	title := i18n.T("settings.permissions.title")
	if onboarding {
		title = i18n.T("onboarding.permissions.title")
	}
	heading(c, p, title, 30)
	mu.Text(c, i18n.T("onboarding.permissions.lead")).FontSize(14).TextColor(p.Graphite)
	type perm struct {
		key, name, detail, kind string
		ok                      bool
	}
	var st struct{ mic, ax, im, needIM bool }
	pm := a.platformPermissions()
	if pm != nil {
		s := pm.Status()
		st.mic, st.ax, st.im, st.needIM = s.Microphone, s.Accessibility, s.InputMonitoring, s.NeedsInputPerms
	}
	list := []perm{
		{"permissions.mic", i18n.T("os.permission.microphone"), i18n.T("settings.permissions.microphone_detail"), "microphone", st.mic},
		{"permissions.ax", i18n.TOS("os.permission.accessibility", nil), i18n.T("settings.permissions.accessibility_detail"), "accessibility", st.ax},
	}
	if st.needIM {
		list = append(list, perm{"permissions.im", i18n.TOS("os.permission.input_monitoring", nil), i18n.T("settings.permissions.input_monitoring_detail"), "input_monitoring", st.im})
	}
	a.group(c, p, "", func() {
		for _, it := range list {
			a.row(c, p, it.name, it.detail, func() {
				if it.ok {
					slip(c, p, i18n.T("onboarding.permissions.allowed"), p.Success)
					return
				}
				if a.btn(c, p, it.key, i18n.T("settings.permissions.request"), btnSecondary) && pm != nil {
					kind := it.kind
					go func() {
						var err error
						switch kind {
						case "microphone":
							err = pm.RequestMicrophone()
						case "accessibility":
							err = pm.RequestAccessibility()
						default:
							err = pm.RequestInputMonitoring()
						}
						if err != nil {
							log.Printf("permission %s: %v", kind, err)
						}
						a.post(a.invalidate)
					}()
				}
			})
		}
	})
	mu.Text(c, i18n.TOS("settings.permissions.note", nil)).FontSize(12.5).TextColor(p.Pencil)
}

// ---- What's New ----

type whatsNewState struct {
	release update.NotesRelease
	has     bool
}

func (s *whatsNewState) load(notes, version string) {
	if notes == "" {
		return
	}
	releases, err := update.ParseReleaseNotes(notes)
	if err != nil {
		log.Printf("release notes: %v", err)
		return
	}
	v, err := update.ParseVersion(version)
	if err != nil {
		return
	}
	s.release, s.has = update.ReleaseIn(releases, v, update.ThisOS())
}

// HasWhatsNew is true when the release notes have this version.
func (a *App) HasWhatsNew() bool { return a.news.has }

// OpenWhatsNew shows the What's New window.
func (a *App) OpenWhatsNew() { a.showWindow(WinWhatsNew, true) }

// WhatsNewView is the view of the What's New window.
func (a *App) WhatsNewView(c *mu.Context) {
	a.secondaryFrame(c, func(p theme.Colors) {
		eyebrow(c, p, i18n.T("whats_new.heading", "version", a.d.Version))
		r := a.news.release
		title := r.Title
		if title == "" {
			title = i18n.T("whats_new.window_title")
		}
		heading(c, p, title, 28)
		mu.Scroll(c.Key("whats_new.list")).Grow(1).Gap(14).Role(mu.RoleList).Label(i18n.T("whats_new.list_label")).Children(func() {
			for _, it := range r.Item {
				mu.Row(c).Gap(12).AlignItems(mu.Start).Children(func() {
					ic := it.Icon
					if ic == "" {
						ic = "sparkle"
					}
					mu.Icon(c, icon(ic)).FontSize(18).TextColor(p.Accent)
					mu.Column(c).Gap(3).Grow(1).Basis(0).Children(func() {
						mu.Text(c, it.Title).FontSize(14.5).FontWeight(600).TextColor(p.Ink)
						mu.Text(c, it.Text).FontSize(13).TextColor(p.Graphite).LineHeight(1.4)
					})
				})
			}
			if r.Note != nil {
				card(c, p).Children(func() {
					mu.Text(c, r.Note.Title).FontSize(14).FontWeight(600).TextColor(p.Ink)
					mu.Text(c, r.Note.Text).FontSize(13).TextColor(p.Graphite)
				})
			}
		})
		mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
			if a.btn(c, p, "whats_new.all", i18n.T("whats_new.all_changes"), btnGhost) {
				a.openURL("https://github.com/watzon/sayso/releases")
			}
			mu.Spacer(c)
			if a.btn(c, p, "whats_new.close", i18n.T("whats_new.done"), btnPrimary) {
				if a.d.Paths.DataDir != "" {
					if err := update.MarkWhatsNewSeen(a.d.Paths.StateFile()); err != nil {
						log.Printf("whats new: %v", err)
					}
				}
				a.showWindow(WinWhatsNew, false)
			}
		})
	})
}

// ---- Updates ----

// Updater is the background updater; *update.Updater implements it.
type Updater interface {
	CheckNow()
	Install()
	SetAutoCheck(on bool)
	SetAutomatic(on bool)
}

type updateState struct {
	status update.Status
}

// UpdateStatus implements the updater's status callback.
func (a *App) UpdateStatus(s update.Status) {
	a.post(func() {
		a.upd.status = s
		a.invalidate()
	})
}

// updateOffered is true when a new version is on offer.
func (a *App) updateOffered() bool {
	switch a.upd.status.Kind {
	case update.StatusAvailable, update.StatusDownloading, update.StatusPreparing, update.StatusReady:
		return true
	}
	return false
}

// CheckForUpdates asks the updater to check now.
func (a *App) CheckForUpdates() {
	if a.d.Updater != nil {
		a.upd.status.Kind = update.StatusChecking
		a.d.Updater.CheckNow()
	}
	a.invalidate()
}

func (a *App) openUpdateWindow() { a.showWindow(WinUpdate, true) }

func (a *App) updateHeadline() string {
	s := a.upd.status
	switch s.Kind {
	case update.StatusChecking:
		return i18n.T("update.checking")
	case update.StatusAvailable:
		return i18n.T("update.available", "version", s.Version.String())
	case update.StatusDownloading:
		return i18n.T("update.downloading", "version", s.Version.String())
	case update.StatusPreparing:
		return i18n.T("update.preparing", "version", s.Version.String())
	case update.StatusReady:
		return i18n.T("update.ready", "version", s.Version.String())
	case update.StatusUnreadable:
		return i18n.T("update.newer_available")
	case update.StatusFailed:
		return i18n.T("update.failed")
	}
	return i18n.T("update.up_to_date")
}

func (a *App) updateDetail() string {
	s := a.upd.status
	switch s.Kind {
	case update.StatusDownloading:
		return i18n.T("update.downloaded", "received", models.FormatSize(s.Received), "total", models.FormatSize(s.Total))
	case update.StatusReady:
		return i18n.T("update.ready_detail")
	case update.StatusFailed:
		return s.Failure.Error()
	case update.StatusIdle:
		if s.LastCheck != nil {
			t := time.Unix(int64(*s.LastCheck), 0)
			return i18n.T("update.last_check_date", "version", a.d.Version, "date", i18n.DayMonth(t), "time", t.Format("15:04"))
		}
		return i18n.T("update.never_checked", "version", a.d.Version)
	}
	return i18n.T("update.you_have", "version", a.d.Version)
}

// UpdateView is the view of the update window.
func (a *App) UpdateView(c *mu.Context) {
	a.secondaryFrame(c, func(p theme.Colors) {
		heading(c, p, a.updateHeadline(), 24)
		mu.Text(c, a.updateDetail()).FontSize(13.5).TextColor(p.Graphite)
		if s := a.upd.status; s.Kind == update.StatusDownloading && s.Total > 0 {
			mu.Progress(c, float64(s.Received)/float64(s.Total)).Height(4)
		}
		mu.Spacer(c)
		mu.Row(c).Gap(8).Children(func() {
			if a.upd.status.NotesURL != "" && a.btn(c, p, "update.notes", i18n.T("update.whats_new"), btnGhost) {
				a.openURL(a.upd.status.NotesURL)
			}
			mu.Spacer(c)
			if a.btn(c, p, "update.dismiss", i18n.T("update.dismiss"), btnSecondary) {
				a.showWindow(WinUpdate, false)
			}
			label := i18n.T("update.action.update_now")
			if a.upd.status.Kind == update.StatusReady {
				label = i18n.T("update.action.restart")
			}
			if a.btn(c, p, "update.install", label, btnPrimary) && a.d.Updater != nil {
				a.d.Updater.Install()
			}
		})
	})
}

// ---- App approval ----

type approvalAsk struct {
	q        bridge.ApprovalQuestion
	download *bridge.DownloadQuestion
	answer   chan bool
}

// AskApproval asks the user about an app and waits for the answer. It
// runs on a Host goroutine.
func (a *App) AskApproval(q bridge.ApprovalQuestion, withdrawn func() bool) (allow, answered bool) {
	return a.ask(&approvalAsk{q: q, answer: make(chan bool, 1)}, withdrawn)
}

// AskDownload asks the user whether an app may download a model.
func (a *App) AskDownload(q bridge.DownloadQuestion, withdrawn func() bool) bool {
	allow, _ := a.ask(&approvalAsk{download: &q, answer: make(chan bool, 1)}, withdrawn)
	return allow
}

func (a *App) ask(ask *approvalAsk, withdrawn func() bool) (bool, bool) {
	a.post(func() {
		a.asks = append(a.asks, ask)
		a.showWindow(WinApproval, true)
		a.invalidate()
	})
	t := time.NewTicker(250 * time.Millisecond)
	defer t.Stop()
	for {
		select {
		case v := <-ask.answer:
			return v, true
		case <-t.C:
			if withdrawn != nil && withdrawn() {
				a.post(func() { a.dropAsk(ask) })
				return false, false
			}
		}
	}
}

func (a *App) dropAsk(ask *approvalAsk) {
	out := a.asks[:0]
	for _, x := range a.asks {
		if x != ask {
			out = append(out, x)
		}
	}
	a.asks = out
	if len(a.asks) == 0 {
		a.showWindow(WinApproval, false)
	}
	a.invalidate()
}

func (a *App) answer(ask *approvalAsk, allow bool) {
	ask.answer <- allow
	a.dropAsk(ask)
}

// ApprovalView is the view of the approval window: the first question.
func (a *App) ApprovalView(c *mu.Context) {
	a.secondaryFrame(c, func(p theme.Colors) {
		if len(a.asks) == 0 {
			mu.Text(c, i18n.T("apps.settings.empty")).FontSize(13).TextColor(p.Graphite)
			return
		}
		ask := a.asks[0]
		if d := ask.download; d != nil {
			heading(c, p, i18n.T("apps.download_ask.title", "app", d.AppName), 22)
			mu.Text(c, i18n.T("apps.download_ask.body", "model", d.ModelName, "size", models.FormatSize(d.SizeBytes))).FontSize(13.5).TextColor(p.Graphite)
			if d.Reason != "" {
				eyebrow(c, p, i18n.T("apps.download_ask.reason_label", "app", d.AppName))
				mu.Text(c, d.Reason).FontSize(13).TextColor(p.Ink)
			}
		} else {
			q := ask.q
			title := i18n.T("apps.approval.title", "app", q.Name)
			body := i18n.T("apps.approval.body")
			if q.Approved {
				title, body = i18n.T("apps.approval.title_more", "app", q.Name), i18n.T("apps.approval.body_more")
			}
			heading(c, p, title, 22)
			signer := i18n.T("apps.approval.not_signed")
			switch {
			case q.Signer != "":
				signer = i18n.T("apps.approval.signed_by", "org", q.Signer)
			case q.Identity.Kind == bridge.IdentitySigned && q.Identity.TeamID != "":
				signer = i18n.T("apps.approval.signed")
			}
			mu.Text(c, signer).FontSize(12.5).TextColor(p.Pencil)
			if id := identityText(q.Identity); id != "" {
				mu.Text(c, id).Font(theme.Mono).FontSize(12).TextColor(p.Graphite)
			}
			if q.ClaimedName != "" && q.ClaimedName != q.Name {
				mu.Text(c, i18n.T("apps.approval.claimed", "name", q.ClaimedName)).FontSize(12.5).TextColor(p.Pencil)
			}
			mu.Text(c, body).FontSize(13.5).TextColor(p.Graphite)
			if q.Interpreter {
				mu.Text(c, i18n.T("apps.approval.interpreter")).FontSize(13).TextColor(p.Danger)
			}
			eyebrow(c, p, i18n.T("apps.approval.asks_to"))
			shown := q.Capabilities
			if len(shown) == 0 {
				// An app that asks for nothing still gets the list of the models.
				shown = []bridge.Capability{bridge.CapModelsRead}
			}
			for _, cp := range shown {
				mu.Row(c).Gap(8).Children(func() {
					mu.Icon(c, icon("check")).FontSize(13).TextColor(p.Graphite)
					mu.Text(c, capabilityLabel(cp)).FontSize(13).TextColor(p.Ink)
				})
			}
		}
		mu.Spacer(c)
		if n := len(a.asks); n > 1 {
			mu.Text(c, fmt.Sprintf("+%d", n-1)).FontSize(12).TextColor(p.Pencil)
		}
		mu.Row(c).Gap(8).Justify(mu.End).Children(func() {
			deny, allow := i18n.T("apps.approval.dont_allow"), i18n.T("apps.approval.allow")
			if ask.download != nil {
				deny, allow = i18n.T("apps.download_ask.cancel"), i18n.T("apps.download_ask.download")
			}
			if a.btn(c, p, "approval.deny", deny, btnSecondary) {
				a.answer(ask, false)
			}
			if a.btn(c, p, "approval.allow", allow, btnPrimary) {
				a.answer(ask, true)
			}
		})
	})
}

// identityText is the team and identifier, the executable path, or the
// Flatpak id of an app, as the approval question shows it.
func identityText(id bridge.Identity) string {
	switch id.Kind {
	case bridge.IdentitySigned:
		if id.TeamID == "" {
			return ""
		}
		return id.Identifier + " · " + id.TeamID
	case bridge.IdentityPath:
		if id.Path == "" {
			return ""
		}
		return bridge.VisiblePath(id.Path)
	case bridge.IdentityFlatpak:
		return id.AppID + " · Flatpak"
	}
	return ""
}
