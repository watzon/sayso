package ui

import (
	"testing"

	mu "github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/golden"

	"github.com/watzon/sayso/internal/config"
)

// The paper kit as Rust draws it (spec §7.6): buttons, the switch, chips,
// a badge, keycaps, a style tag and a settings row, light and dark. Text
// differs between systems, so the goldens are for macOS only. Run with
// MYGO_UPDATE_GOLDEN=1 to write them again after a deliberate change.
func TestGoldenPaperKit(t *testing.T) {
	for _, dark := range []bool{false, true} {
		e := newEnv(t)
		a := e.a
		if dark {
			a.cfg.Appearance.Theme = config.ThemeDark
		}
		on, off := true, false
		view := func(c *mu.Context) {
			p := a.colors(c)
			mu.Column(c).Padding(20).Gap(16).Background(p.Sheet).Grow(1).Children(func() {
				mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
					a.button(c, p, "g.primary", "New style", bopt{kind: btnPrimary, icon: "plus", size: sizeLarge})
					a.button(c, p, "g.secondary", "Base prompt", bopt{icon: "file"})
					a.button(c, p, "g.small", "Change style", bopt{size: sizeSmall})
					a.button(c, p, "g.danger", "Remove", bopt{kind: btnDanger})
				})
				mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
					a.toggle(c, "g.on", "On", &on, nil)
					a.toggle(c, "g.off", "Off", &off, nil)
					a.chip(c, p, "g.chip.on", "All", true)
					a.chip(c, p, "g.chip.off", "Enhanced", false)
					badge(c, p, "Cloud", toneAccent, "cloud")
					badge(c, p, "Default", toneInk, "")
					styleTag(c, p, "Rambling", "#6B4A6E")
					keycaps(c, p, []string{"⌥", "Space"}, false)
				})
				a.row(c, p, "Launch at login", "Start Sayso in the menu bar when you log in.", func() {
					a.toggle(c, "g.row", "Launch at login", &on, nil)
				})
			})
		}
		tt := mu.NewTester(view, 720, 220)
		tt.SetDark(dark)
		name := "testdata/golden-paper-kit-light.png"
		if dark {
			name = "testdata/golden-paper-kit-dark.png"
		}
		golden.Match(t, tt, name, golden.Tolerance{Channel: 8, PerMille: 5})
	}
}
