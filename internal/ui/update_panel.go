package ui

import (
	"fmt"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/theme"
	"github.com/watzon/sayso/internal/update"
)

// The update controls that Settings › Updates, the sidebar, and the update
// window share: one status (a.upd), one set of actions.

type updateAction int

const (
	updNone         updateAction = iota
	updDownload                  // download and prepare the offered version
	updDownloadPage              // this install updates by hand
	updCancel                    // stop the download
	updRestart                   // install the prepared version
	updRetry                     // after a failure
)

// updateActionNow is what the user can do in the current status.
func (a *App) updateActionNow() updateAction {
	s := a.upd.status
	switch s.Kind {
	case update.StatusAvailable:
		switch {
		case s.Manual == update.ManualNone:
			return updDownload
		case s.Manual == update.ManualPackaged && (s.Package == update.PackageAUR || s.Package == update.PackageNix):
			return updNone // the package manager updates it; the detail says how
		}
		return updDownloadPage
	case update.StatusUnreadable:
		return updDownloadPage
	case update.StatusDownloading:
		return updCancel
	case update.StatusReady:
		return updRestart
	case update.StatusFailed:
		return updRetry
	}
	return updNone
}

// runUpdateAction does an action.
func (a *App) runUpdateAction(act updateAction) {
	u := a.d.Updater
	switch act {
	case updDownloadPage:
		a.openURL(update.DownloadPage)
	case updDownload:
		if u != nil {
			u.Install()
		}
	case updCancel:
		if u != nil {
			u.Cancel()
		}
	case updRestart:
		if u != nil {
			u.Restart()
		}
	case updRetry:
		// A failed download or swap keeps the offer: download again. A failed
		// check, or a failure found at start, checks again.
		if a.upd.offered && u != nil {
			u.Install()
		} else {
			a.CheckForUpdates()
		}
	}
	a.invalidate()
}

func updateActionLabel(act updateAction) string {
	switch act {
	case updDownload:
		return i18n.T("update.action.download")
	case updDownloadPage:
		return i18n.T("update.action.open_download_page")
	case updCancel:
		return i18n.T("update.action.cancel")
	case updRestart:
		return i18n.T("update.action.restart")
	case updRetry:
		return i18n.T("update.action.try_again")
	}
	return ""
}

func updateActionKey(act updateAction) string {
	return [...]string{"", "download", "download_page", "cancel", "restart", "retry"}[act]
}

// updateFraction is the part of the download received, 0 to 1.
func (a *App) updateFraction() float32 {
	s := a.upd.status
	if s.Kind != update.StatusDownloading || s.Total == 0 {
		return 0
	}
	return min(float32(float64(s.Received)/float64(s.Total)), 1)
}

// updateProgressText is "12.3 MB of 98.1 MB · 13%".
func (a *App) updateProgressText() string {
	s := a.upd.status
	text := i18n.T("update.downloaded", "received", models.FormatSize(s.Received), "total", models.FormatSize(s.Total))
	if s.Total > 0 {
		text += fmt.Sprintf(" · %d%%", int(a.updateFraction()*100))
	}
	return text
}

// updateFailureText is the failure in the interface language.
func updateFailureText(f update.Failure) string {
	key := "update.failure." + f.Kind
	text := i18n.T(key, "error", f.Detail, "back", f.Back, "version", f.Detail)
	if text == key {
		return f.Error()
	}
	return text
}

// updateManualText says how this install gets the new version by hand.
func (a *App) updateManualText(s update.Status) string {
	v := a.d.Version
	switch s.Manual {
	case update.ManualWrongFolder:
		return i18n.T("update.wrong_folder")
	case update.ManualNoPermission:
		return i18n.T("update.no_permission", "version", v)
	case update.ManualNoExchange:
		return i18n.T("update.no_exchange", "version", v)
	case update.ManualPackaged:
		switch s.Package {
		case update.PackageAppImage:
			return i18n.T("update.packaged_appimage", "version", v)
		case update.PackageFlatpak:
			return i18n.T("update.packaged_flatpak", "version", v)
		case update.PackageAUR:
			return i18n.T("update.packaged_aur", "version", v)
		case update.PackageNix:
			return i18n.T("update.packaged_nix", "version", v)
		}
	}
	return i18n.T("update.unsupported", "version", v)
}

// updateButtons are the action of the status and, with check, Check for
// updates where there is nothing else to do. Keys: <prefix>.download,
// .download_page, .cancel, .restart, .retry, and updates.check_now.
func (a *App) updateButtons(c *mu.Context, p theme.Colors, prefix string, check bool) {
	act := a.updateActionNow()
	mu.Row(c).Gap(8).AlignItems(mu.Center).Shrink(0).Children(func() {
		if act != updNone {
			kind := btnPrimary
			if act == updCancel || act == updDownloadPage {
				kind = btnSecondary
			}
			if a.btn(c, p, prefix+"."+updateActionKey(act), updateActionLabel(act), kind) {
				a.runUpdateAction(act)
			}
		}
		switch a.upd.status.Kind {
		case update.StatusIdle, update.StatusChecking, update.StatusUnreadable:
			if check && a.button(c, p, "updates.check_now", i18n.T("update.action.check"),
				bopt{kind: btnSecondary, disabled: a.upd.status.Kind == update.StatusChecking}) {
				a.CheckForUpdates()
			}
		}
	})
}

// updateTrack is the download progress in the accent.
func (a *App) updateTrack(c *mu.Context, p theme.Colors, key string, height float32) mu.Element {
	return trackIn(a.k(c, key), p, "", a.updateFraction(), height, i18n.T("update.downloading", "version", a.upd.status.Version.String()), p.Accent)

}

// accentButton is the call to action of the sidebar slip: accent filled,
// the one spot of color in the sidebar.
func (a *App) accentButton(c *mu.Context, p theme.Colors, key, label, iconName string) bool {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Height(30).PaddingX(12).Gap(6).Radius(8).Grow(1).
		Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer).Background(p.Accent)
	switch {
	case b.Pressed():
		b.InsetShadow(0, 2, 4, 0, black.Alpha(0.35))
	default:
		b.InsetShadow(0, 1, 0, 0, white.Alpha(0.18)).InsetShadow(0, -2, 3, 0, black.Alpha(0.25)).Shadow(0, 1, 2, 0, p.ShadowA(0.25))
		if b.Hovered() {
			b.Background(p.Accent.Alpha(0.9))
		}
	}
	b.Children(func() {
		if iconName != "" {
			mu.Icon(c, dirIcon(c, iconName)).FontSize(14).TextColor(p.OnInk)
		}
		uiText(c, label, 13, 600, p.OnInk).SingleLine()
	})
	return clicked(c, b)
}

// updateSlipShown is true while the sidebar shows the update: a version on
// offer, or a failure after one.
func (a *App) updateSlipShown() bool {
	return a.updateOffered() || (a.upd.status.Kind == update.StatusFailed && a.upd.offered)
}

// updateSlip is the update in the sidebar: an accent card with the version,
// the progress, and the next step. The rail shows an accent button that
// opens Settings › Updates.
func (a *App) updateSlip(c *mu.Context, p theme.Colors, rail bool) {
	if !a.updateSlipShown() {
		return
	}
	s := a.upd.status
	version := s.Version.String()
	if s.Kind == update.StatusFailed {
		version = a.upd.offer.String()
	}
	if rail {
		label := i18n.T("update.slip_version", "version", version)
		b := mu.ButtonBase(a.k(c, "update.slip")).Label(label).Tooltip(label).Size(30, 30).Shrink(0).Radius(8).
			Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer).Background(p.Accent).Shadow(0, 1, 2, 0, p.ShadowA(0.25))
		b.Children(func() { mu.Icon(c, icon("download")).FontSize(16).TextColor(p.OnInk) })
		if clicked(c, b) {
			a.OpenSettings(SecUpdates)
		}
		return
	}
	title, sub, ic, mark := i18n.T("update.slip_version", "version", version), i18n.T("update.slip_title"), "download", p.Accent
	switch s.Kind {
	case update.StatusDownloading:
		title, sub = i18n.T("update.downloading_short", "version", version), a.updateProgressText()
	case update.StatusPreparing:
		title, sub = i18n.T("update.preparing_short", "version", version), ""
	case update.StatusReady:
		title, sub, ic = i18n.T("update.ready_short", "version", version), "", "check"
	case update.StatusFailed:
		title, sub, ic, mark = i18n.T("update.failed"), updateFailureText(s.Failure), "warning", p.Danger
	case update.StatusAvailable:
		if act := a.updateActionNow(); act == updNone || act == updDownloadPage {
			sub = a.updateManualText(s)
		}
	}
	card := mu.Column(a.k(c, "update.slip.card")).Gap(10).Padding(12, 12, 12, 12).Radius(10).
		Background(p.AccentWash).BorderWidth(1, 1, 1, 1).BorderColor(p.Accent.Alpha(0.45)).
		InsetShadow(0, 1, 0, 0, p.Highlight(0.6)).Shadow(0, 1, 2, 0, p.ShadowA(0.10)).
		Role(mu.RoleGroup).Label(title)
	card.Children(func() {
		mu.Row(c).Gap(10).AlignItems(mu.Start).Children(func() {
			mu.Box(c).Size(24, 24).Shrink(0).Radius(99).Background(mark).Justify(mu.Center).AlignItems(mu.Center).
				Children(func() { mu.Icon(c, icon(ic)).FontSize(13).TextColor(p.OnInk) })
			mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
				uiText(c, title, 13, 600, p.Ink)
				if sub != "" {
					col := p.Graphite
					if s.Kind == update.StatusFailed {
						col = p.Danger
					}
					uiText(c, sub, 12, 400, col).Tooltip(sub)
				}
			})
		})
		if s.Kind == update.StatusDownloading {
			a.updateTrack(c, p, "update.slip.progress", 6)
		}
		act := a.updateActionNow()
		switch act {
		case updNone:
		case updCancel:
			if a.button(c, p, "update.slip.cancel", updateActionLabel(act), bopt{kind: btnGhost, size: sizeSmall, grow: true}) {
				a.runUpdateAction(act)
			}
		default:
			iconName := map[updateAction]string{updDownload: "download", updRestart: "sparkle", updRetry: "undo", updDownloadPage: "globe"}[act]
			if a.accentButton(c, p, "update.slip", updateActionLabel(act), iconName) {
				a.runUpdateAction(act)
			}
		}
		if a.upd.notesURL != "" && a.link(c, p, "update.slip.notes", i18n.T("update.whats_new"), 12, 500, "") {
			a.openURL(a.upd.notesURL)
		}
	})
}
