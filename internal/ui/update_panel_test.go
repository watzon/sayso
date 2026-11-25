package ui

import (
	"image/color"
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/update"
)

func named(tt *mu.Tester, name string) int {
	n := 0
	for _, a := range tt.AccessAll() {
		if a.Name == name && a.Role != mu.RoleText && a.Role != mu.RoleNone {
			n++
		}
	}
	return n
}

// built renders a frame and reports which of the keys it built.
func built(e *testEnv, tt *mu.Tester, keys ...string) map[string]bool {
	e.a.keys = nil
	tt.Frame()
	out := map[string]bool{}
	for _, k := range keys {
		out[k] = e.a.keys[k]
	}
	return out
}

var offer = update.Status{Kind: update.StatusAvailable, Version: update.MustVersion("0.17.1"), NotesURL: update.URLPrefix + "tag/v0.17.1"}

// Settings › Updates downloads, shows the progress, cancels, and restarts
// the update itself: no trip back to the sidebar.
func TestSettingsDownloadsAndInstallsTheUpdate(t *testing.T) {
	e := newEnv(t)
	u := &fakeUpdater{}
	e.a.SetUpdater(u)
	e.a.OpenSettings(SecUpdates)
	tt := e.hub(t)
	e.a.UpdateStatus(offer)
	if k := built(e, tt, "updates.download"); !k["updates.download"] {
		t.Fatalf("no download in Settings; texts %q", tt.Texts())
	}
	press(t, tt, i18n.T("update.action.download"))
	if u.installs != 1 {
		t.Fatalf("installs %d", u.installs)
	}

	e.a.UpdateStatus(update.Status{Kind: update.StatusDownloading, Version: offer.Version, Received: 42 << 20, Total: 100 << 20})
	k := built(e, tt, "updates.progress", "updates.cancel", "updates.download")
	if !k["updates.progress"] || !k["updates.cancel"] || k["updates.download"] {
		t.Fatalf("downloading: %v", k)
	}
	if !tt.HasText(e.a.updateProgressText()) || !strings.HasSuffix(e.a.updateProgressText(), "42%") {
		t.Errorf("progress text %q; texts %q", e.a.updateProgressText(), tt.Texts())
	}
	press(t, tt, i18n.T("update.action.cancel"))
	if u.cancels != 1 {
		t.Fatalf("cancels %d", u.cancels)
	}

	e.a.UpdateStatus(update.Status{Kind: update.StatusReady, Version: offer.Version})
	if k := built(e, tt, "updates.restart"); !k["updates.restart"] {
		t.Fatal("no restart in Settings")
	}
	press(t, tt, i18n.T("update.action.restart"))
	if u.restarts != 1 {
		t.Fatalf("restarts %d", u.restarts)
	}
}

// Try again downloads again after a failed download, and checks again after
// a failed check.
func TestSettingsTriesAgainAfterAFailure(t *testing.T) {
	e := newEnv(t)
	u := &fakeUpdater{}
	e.a.SetUpdater(u)
	e.a.OpenSettings(SecUpdates)
	tt := e.hub(t)
	e.a.UpdateStatus(offer)
	e.a.UpdateStatus(update.Status{Kind: update.StatusFailed, Failure: update.Failure{Kind: "download", Detail: "EOF"}})
	if k := built(e, tt, "updates.retry"); !k["updates.retry"] {
		t.Fatal("no retry")
	}
	if want := i18n.T("update.failure.download", "error", "EOF"); !tt.HasText(want) {
		t.Errorf("failure text %q not shown; texts %q", want, tt.Texts())
	}
	press(t, tt, i18n.T("update.action.try_again"))
	if u.installs != 1 || u.checks != 0 {
		t.Fatalf("after a failed download: installs %d, checks %d", u.installs, u.checks)
	}

	e.a.UpdateStatus(update.Status{Kind: update.StatusIdle})
	e.a.UpdateStatus(update.Status{Kind: update.StatusFailed, Failure: update.Failure{Kind: "unreachable", Detail: "offline"}})
	tt.Frame()
	press(t, tt, i18n.T("update.action.try_again"))
	if u.installs != 1 || u.checks != 1 {
		t.Fatalf("after a failed check: installs %d, checks %d", u.installs, u.checks)
	}
}

// An install that cannot update itself offers the download page.
func TestAManualInstallOpensTheDownloadPage(t *testing.T) {
	e := newEnv(t)
	e.a.SetUpdater(&fakeUpdater{})
	e.a.OpenSettings(SecUpdates)
	tt := e.hub(t)
	s := offer
	s.Manual = update.ManualNoPermission
	e.a.UpdateStatus(s)
	if k := built(e, tt, "updates.download_page", "updates.download"); !k["updates.download_page"] || k["updates.download"] {
		t.Fatalf("manual: %v", k)
	}
	press(t, tt, i18n.T("update.action.open_download_page"))
	if len(e.shell.opened) == 0 || e.shell.opened[len(e.shell.opened)-1] != update.DownloadPage {
		t.Errorf("opened %v", e.shell.opened)
	}
}

// The sidebar shows the same update as Settings, with the same actions.
func TestTheSidebarSlipFollowsTheUpdate(t *testing.T) {
	e := newEnv(t)
	u := &fakeUpdater{}
	e.a.SetUpdater(u)
	tt := e.hub(t)
	if k := built(e, tt, "update.slip.card"); k["update.slip.card"] {
		t.Fatal("a slip with no update")
	}
	e.a.UpdateStatus(offer)
	if k := built(e, tt, "update.slip.card", "update.slip", "update.slip.notes"); !k["update.slip.card"] || !k["update.slip"] || !k["update.slip.notes"] {
		t.Fatalf("available: %v", k)
	}
	if n := named(tt, i18n.T("update.action.download")); n != 1 {
		t.Fatalf("%d Download controls", n)
	}
	press(t, tt, i18n.T("update.action.download"))
	if u.installs != 1 {
		t.Fatalf("installs %d", u.installs)
	}

	e.a.UpdateStatus(update.Status{Kind: update.StatusDownloading, Version: offer.Version, Received: 1 << 20, Total: 4 << 20})
	if k := built(e, tt, "update.slip.progress", "update.slip.cancel"); !k["update.slip.progress"] || !k["update.slip.cancel"] {
		t.Fatalf("downloading: %v", k)
	}
	if !tt.HasText(e.a.updateProgressText()) {
		t.Errorf("no progress text; texts %q", tt.Texts())
	}

	e.a.UpdateStatus(update.Status{Kind: update.StatusReady, Version: offer.Version})
	tt.Frame()
	press(t, tt, i18n.T("update.action.restart"))
	if u.restarts != 1 {
		t.Fatalf("restarts %d", u.restarts)
	}

	// The settings page and the sidebar read one status.
	e.a.OpenSettings(SecUpdates)
	if k := built(e, tt, "updates.restart"); !k["updates.restart"] {
		t.Fatal("Settings does not show the update the sidebar showed")
	}

	e.a.CloseSettings()
	e.a.UpdateStatus(update.Status{Kind: update.StatusIdle})
	if k := built(e, tt, "update.slip.card"); k["update.slip.card"] {
		t.Error("the slip stayed after a check found nothing newer")
	}
}

// In the rail the update is an accent button that opens Settings › Updates.
func TestTheRailSlipOpensTheUpdateSettings(t *testing.T) {
	e := newEnv(t)
	e.a.SetUpdater(&fakeUpdater{})
	e.a.cfg.Appearance.SidebarCollapsed = true
	tt := e.hub(t)
	e.a.UpdateStatus(offer)
	tt.Frame()
	press(t, tt, i18n.T("update.slip_version", "version", "0.17.1"))
	if !e.a.inSettings || e.a.section != SecUpdates {
		t.Errorf("in settings %v, section %v", e.a.inSettings, e.a.section)
	}
}

// The slip stands out from the sidebar: its button is filled with the
// accent, in light and dark.
func TestTheSidebarSlipIsAccentFilled(t *testing.T) {
	for _, dark := range []bool{false, true} {
		e := newEnv(t)
		e.a.cfg.Appearance.PaperTexture = false
		e.a.SetUpdater(&fakeUpdater{})
		tt := e.hub(t)
		tt.SetDark(dark)
		e.a.UpdateStatus(offer)
		tt.Frame()
		r, ok := controlBounds(tt, i18n.T("update.action.download"))
		if !ok {
			t.Fatal("no download button")
		}
		got := tt.Image().RGBAAt(int(r.X+6), int(r.Y+r.H/2))
		a := e.a.themes.get(e.a.cfg.Appearance.Ink, dark).Colors.Accent
		want := color.RGBA{a.R, a.G, a.B, 255}
		if !near(got, want) {
			t.Errorf("dark %v: the button is %v, the accent %v", dark, got, want)
		}
	}
}
