package ui

import (
	"fmt"
	"log"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/bridge"
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
	WhatsNewW, WhatsNewH = 520, 600
	UpdateW, UpdateH     = 460, 300
	ApprovalW, ApprovalH = 460, 420
	PermissionsW         = 520
	PermissionsH         = 440
)

func (a *App) showWindow(w Window, show bool) {
	if a.d.Shell != nil {
		a.d.Shell.ShowWindow(w, show)
	}
}

// secondaryFrame is the paper sheet every secondary window draws on. The
// windows have no title bar, so the whole sheet moves the window: a press
// on a control inside it still goes to the control.
func (a *App) secondaryFrame(c *mu.Context, fn func(p theme.Colors)) {
	p := a.look(c)
	c.Root().Background(p.Sheet)
	bar := c.TitleBar()
	sheet := mu.Column(c.Key("secondary.sheet")).Fill().Padding(max(bar.Bottom, 0)+24, 28, 24, 28).Gap(14).DragWindow()
	// None of these windows maximizes, so a double click on the sheet does
	// nothing, rather than the zoom or maximize of a title bar.
	sheet.WindowDoubleClicked()
	sheet.Children(func() { fn(p) })
}

// finishSecondary ends onboarding: What's New waits for the next version (a
// new user has nothing to compare with), the window closes and the hub opens.
func (a *App) finishSecondary() {
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
					mu.Icon(c, dirIcon(c, ic)).FontSize(18).TextColor(p.Accent)
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
	// Install downloads and prepares the offered version.
	Install()
	// Cancel stops a download.
	Cancel()
	// Restart installs the prepared version; StatusRestarting follows.
	Restart()
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
		if s.Kind == update.StatusRestarting && a.d.Shell != nil {
			// The new version (macOS) or Setup (Windows) waits for Sayso to quit.
			a.d.Shell.Quit()
		}
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
				if a.upd.status.Kind == update.StatusReady {
					a.d.Updater.Restart()
				} else {
					a.d.Updater.Install()
				}
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
				ltr(mu.Text(c, id)).Font(theme.Mono).FontSize(12).TextColor(p.Graphite)
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
