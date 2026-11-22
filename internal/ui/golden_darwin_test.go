package ui

import (
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"
	"github.com/egoist/mygo/ui/golden"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/store"
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

// The recording overlay as the Paper page "Overlay" draws it: the ink
// waveform in its pill, and the live preview on its slip, short at Large
// and long at Small (the latest words at the bottom, the oldest fading out
// of the top), light and dark. Reduce motion stills the ink and the pen.
func TestGoldenOverlayRecording(t *testing.T) {
	for _, dark := range []bool{false, true} {
		for _, size := range []config.OverlaySize{config.OverlayLarge, config.OverlaySmall} {
			a := recordingWithPreview(t, size, config.OverlayPill)
			if size == config.OverlayLarge {
				a.Preview("Let's move the standup to Thursday and invite the design", "team to jo")
			}
			if dark {
				a.cfg.Appearance.Theme = config.ThemeDark
			}
			for i := range 48 {
				a.Level(0.3 + 0.5*float32(i%7)/7)
			}
			w, h := OverlayWindowSize(a.cfg.Overlay)
			tt := mu.NewTesterWith(a.OverlayView, w, h, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
			tt.SetDark(dark)
			tt.Frame()
			tt.Frame()
			mode := "light"
			if dark {
				mode = "dark"
			}
			golden.Match(t, tt, "testdata/golden-overlay-recording-"+string(size)+"-"+mode+".png", golden.Tolerance{Channel: 8, PerMille: 5})
		}
	}
}

// "How it got here" of a long dictation: the italic transcript in two
// lines, and "Show full transcript" under it at its left edge.
func TestGoldenHistoryTrail(t *testing.T) {
	e := newEnv(t)
	a := e.a
	long := strings.Repeat("so the plan is to move the standup to Thursday and invite the design team ", 3)
	en := store.HistoryEntry{Transcript: long, FinalText: long, StyleID: "raw", Model: "parakeet-tdt-v3"}
	en.Insert.Result = store.InsertPasted
	view := func(c *mu.Context) {
		p := a.look(c)
		mu.Column(c).Padding(20).Background(p.Sheet).Grow(1).Children(func() { a.trail(c, p, en) })
	}
	tt := mu.NewTester(view, 560, 360)
	golden.Match(t, tt, "testdata/golden-history-trail-light.png", golden.Tolerance{Channel: 8, PerMille: 5})
}
