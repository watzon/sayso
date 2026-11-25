package ui

import (
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/update"
)

// Restart to update installs the prepared version, and Sayso quits once the
// updater says the new version or Setup waits for it.
func TestRestartToUpdateInstallsAndQuits(t *testing.T) {
	e := newEnv(t)
	u := &fakeUpdater{}
	e.a.SetUpdater(u)
	e.a.UpdateStatus(update.Status{Kind: update.StatusReady, Version: update.MustVersion("0.17.1")})
	tt := mu.NewTester(e.a.UpdateView, UpdateW, UpdateH)
	press(t, tt, i18n.T("update.action.restart"))
	if u.restarts != 1 || u.installs != 0 {
		t.Fatalf("restarts %d, installs %d", u.restarts, u.installs)
	}
	if e.shell.quit {
		t.Fatal("quit before the update was in place")
	}
	e.a.UpdateStatus(update.Status{Kind: update.StatusRestarting, Version: update.MustVersion("0.17.1")})
	if !e.shell.quit {
		t.Error("Sayso did not quit for the update")
	}
}
