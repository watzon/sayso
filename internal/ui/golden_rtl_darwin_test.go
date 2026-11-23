package ui

import (
	"testing"

	mu "github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/golden"

	"github.com/watzon/sayso/internal/config"
)

// The hub, onboarding and the overlay laid out right to left, with the
// English text, as an Arabic interface lays them out: the sidebar on the
// right, the sheet clear of the traffic lights, mirrored drawings and
// arrows, hotkeys left to right. Run with MYGO_UPDATE_GOLDEN=1 to write
// them again after a deliberate change.
func TestGoldenRightToLeft(t *testing.T) {
	tol := golden.Tolerance{Channel: 8, PerMille: 5}
	t.Run("hub", func(t *testing.T) {
		forceDirection(t, "rtl")
		e := newEnv(t)
		e.addEntry(t, "Let's move the standup to Thursday.", "Slack", false)
		tt := e.hub(t)
		tt.SetTitleBar(macBar)
		golden.Match(t, tt, "testdata/golden-rtl-hub-home-light.png", tol)
	})
	t.Run("onboarding", func(t *testing.T) {
		forceDirection(t, "rtl")
		e := newEnv(t)
		e.a.onb.step = stepHotkeys
		tt := mu.NewTesterWith(e.a.OnboardingView, OnboardingW, OnboardingH, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
		tt.SetTitleBar(macBar)
		tt.Frame()
		tt.Frame()
		golden.Match(t, tt, "testdata/golden-rtl-onboarding-05-hotkeys-light.png", tol)
	})
	t.Run("overlay", func(t *testing.T) {
		forceDirection(t, "rtl")
		a := recordingWithPreview(t, config.OverlayLarge, config.OverlayPill)
		a.cfg.Appearance.Theme = config.ThemeDark
		for i := range 48 {
			a.Level(0.3 + 0.5*float32(i%7)/7)
		}
		w, h := OverlayWindowSize(a.cfg.Overlay)
		tt := mu.NewTesterWith(a.OverlayView, w, h, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
		tt.SetDark(true)
		tt.Frame()
		tt.Frame()
		golden.Match(t, tt, "testdata/golden-rtl-overlay-recording-large-dark.png", tol)
	})
}
