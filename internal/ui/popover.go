package ui

import (
	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/theme"
)

// The tray popover (crates/sayso-app/src/popover.rs): a 340 px sheet under
// the menu bar or tray icon with the status, the ink Start button, the last
// dictation, the style chips, Model, Microphone and Incognito, and Open and
// Quit.

// Popover window size, in DIPs.
const (
	PopoverW = 340
	PopoverH = 500
)

type popoverState struct {
	// open is the row whose choices show: "model", "mic" or "".
	open string
}

// PopoverView is the view of the tray popover.
func (a *App) PopoverView(c *mu.Context) {
	p := a.look(c)
	c.Root().Background(p.SheetRaised)
	a.loadHome()
	mu.Column(c).Fill().Padding(8).Gap(6).Draw(a.grain(p, "sheet", 16)).Children(func() {
		status, _, dot := a.engineSummary(p)
		mu.Row(c).Padding(10, 10, 6, 10).Gap(10).AlignItems(mu.Center).Children(func() {
			statusDot(c, dot)
			uiText(c, status, 14, 600, p.Ink).Grow(1).SingleLine()
			uiText(c, i18n.TP("popover.words_today", a.home.stats.WordsToday, map[string]any{"number": formatCount(a.home.stats.WordsToday)}), 12, 400, p.Graphite).SingleLine()
		})
		a.popoverStart(c, p)
		a.popoverLast(c, p)
		caps(c, p, i18n.T("popover.style")).Padding(8, 10, 2, 10)
		active := a.ActiveStyle()
		mu.Row(c).Gap(6).Wrap().PaddingX(6).Role(mu.RoleRadioGroup).Label(i18n.T("popover.style")).Children(func() {
			for _, st := range a.styles.Styles() {
				a.styleChip(c, p, st.ID, styleName(&st), st.Ink, st.ID == active)
			}
		})
		mu.Column(c).Margin(8, 0, 0, 0).BorderWidth(1, 0, 0, 0).BorderColor(p.Rule).Children(func() {
			a.popoverRow(c, p, "popover.model", "models", i18n.T("popover.model"), a.modelName(a.FinalModel()), "model")
			if a.pop.open == "model" {
				a.popoverModels(c, p)
			}
			a.popoverRow(c, p, "popover.mic", "mic", i18n.T("popover.microphone"), a.micName(), "mic")
			if a.pop.open == "mic" {
				a.popoverMics(c, p)
			}
			mu.Row(c).Height(38).PaddingX(10).Gap(10).AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
				mu.Icon(c, icon("wordmark")).FontSize(16).TextColor(p.Graphite)
				mu.Column(c).Grow(1).Children(func() {
					uiText(c, i18n.T("popover.incognito"), 14, 400, p.Ink)
					if a.incognito {
						uiText(c, i18n.T("popover.not_saved"), 12, 400, p.Graphite)
					}
				})
				on := a.incognito
				a.toggle(c, "popover.incognito", i18n.T("popover.incognito"), &on, func() { a.SetIncognito(on) })
			})
		})
		mu.Spacer(c)
		mu.Row(c).Gap(8).Padding(4, 4, 4, 4).AlignItems(mu.Center).Children(func() {
			if a.button(c, p, "popover.open", i18n.T("popover.open_sayso"), bopt{grow: true}) {
				if a.d.Shell != nil {
					a.d.Shell.ShowWindow(WinPopover, false)
					a.d.Shell.ShowHub()
				}
			}
			if a.btn(c, p, "popover.quit", i18n.T("popover.quit"), btnGhost) && a.d.Shell != nil {
				a.d.Shell.Quit()
			}
		})
	})
}

// popoverStart is the big ink Start/Stop button with the hotkey.
func (a *App) popoverStart(c *mu.Context, p theme.Colors) {
	label := i18n.T("popover.start_dictation")
	if a.Busy() {
		label = i18n.T("popover.stop_dictation")
	}
	b := mu.ButtonBase(a.k(c, "popover.toggle")).Label(label).Height(52).Padding(0, 12, 0, 14).Gap(12).Radius(12).
		AlignItems(mu.Center).Cursor(mu.CursorPointer)
	inkButton(b, p, b.Pressed())
	b.Children(func() {
		// popover.rs: a 12 px status dot, light periwinkle on the ink of
		// light paper, the accent on dark paper.
		dot := mu.RGB(0x8D, 0x9B, 0xE6)
		if p.Dark {
			dot = p.Accent
		}
		mu.Box(c).Size(18, 18).Shrink(0).Radius(99).Background(dot.Alpha(0.16)).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
			mu.Box(c).Size(12, 12).Radius(99).Background(dot)
		})
		uiText(c, label, 15, 600, p.OnInk).Grow(1).SingleLine()
		if h := a.cfg.Hotkeys.Toggle; h != nil {
			mu.Row(c).Gap(3).Children(func() {
				for _, k := range h.Keycaps() {
					mu.Box(c).Height(22).MinWidth(22).PaddingX(6).Radius(5).Background(white.Alpha(0.12)).
						InsetShadow(0, -1, 0, 0, black.Alpha(0.4)).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
						mono(c, k, 11, p.OnInk)
					})
				}
			})
		}
	})
	if clicked(c, b) {
		a.Toggle()
	}
}

// popoverLast is the last dictation, with Copy.
func (a *App) popoverLast(c *mu.Context, p theme.Colors) {
	if len(a.home.recent) == 0 {
		return
	}
	e := a.home.recent[0]
	app := i18n.T("home.recent.unknown_app")
	if e.App != nil && e.App.Name != "" {
		app = e.App.Name
	}
	shallowWell(mu.Column(c).Padding(14).Gap(8).Radius(12), p).Children(func() {
		mu.Row(c).AlignItems(mu.Center).Children(func() {
			caps(c, p, i18n.T("popover.last", "app", app, "time", a.clockOf(e.CreatedAt))).Grow(1).Ellipsis("…")
			if a.link(c, p, "popover.copy", i18n.T("popover.copy"), 13, 600, "") {
				c.WriteClipboard(e.FinalText)
			}
		})
		mu.Text(c, e.FinalText).Font(serifFont(theme.Body)).FontSize(15).LineHeight(21.0 / 15).TextColor(p.Ink).MaxLines(3).Ellipsis("…")
	})
}

// styleChip is a style with its ink dot; the chosen one has an ink ring.
func (a *App) styleChip(c *mu.Context, p theme.Colors, id, name, hex string, on bool) {
	b := mu.ButtonBase(a.k(c, "popover.style."+id)).Role(mu.RoleRadio).Checked(on).Label(name).
		Height(28).PaddingX(11).Gap(6).Radius(99).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	fg, weight := p.Graphite, 500
	if on {
		fg, weight = p.Ink, 600
		b.Background(p.SheetRaised).Shadow(0, 0, 0, 1.5, p.Ink).Shadow(0, 1, 2, 0, p.ShadowA(0.16))
	} else {
		raisedSmall(b, p)
		if b.Hovered() {
			fg = p.Ink
		}
	}
	b.Children(func() {
		col, ok := inkColor(hex, p)
		d := mu.Box(c).Size(7, 7).Radius(99)
		if ok {
			d.Background(col)
		} else {
			d.Background(p.Pencil.Alpha(0.6))
		}
		uiText(c, name, 13, weight, fg).SingleLine()
	})
	if clicked(c, b) {
		a.SetActiveStyle(id)
	}
}

// popoverRow is a row with a value that opens its choices.
func (a *App) popoverRow(c *mu.Context, p theme.Colors, key, iconName, label, value, open string) {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Height(38).PaddingX(10).Gap(10).Radius(7).AlignItems(mu.Center).
		Expanded(a.pop.open == open).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Cursor(mu.CursorPointer)
	if b.Hovered() {
		b.Background(p.Deboss.Alpha(0.5))
	}
	b.Children(func() {
		mu.Icon(c, icon(iconName)).FontSize(16).TextColor(p.Graphite)
		uiText(c, label, 14, 400, p.Ink).Grow(1)
		uiText(c, value, 13, 400, p.Graphite).SingleLine().Ellipsis("…").MaxWidth(170)
		mu.Icon(c, icon("chevron-right")).FontSize(12).TextColor(p.Graphite)
	})
	if clicked(c, b) {
		if a.pop.open == open {
			a.pop.open = ""
		} else {
			a.pop.open = open
		}
	}
}

// popoverChoice is one 30 px choice under an open row.
func (a *App) popoverChoice(c *mu.Context, p theme.Colors, key, label string, on bool, pick func()) {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Role(mu.RoleMenuItemRadio).Checked(on).Height(30).PaddingX(10).Gap(8).Radius(7).
		AlignItems(mu.Center).Cursor(mu.CursorPointer)
	if b.Hovered() {
		b.Background(p.Deboss)
	}
	b.Children(func() {
		mu.Box(c).Width(14).Children(func() {
			if on {
				mu.Icon(c, icon("check")).FontSize(12).TextColor(p.Ink)
			}
		})
		uiText(c, label, 13, 500, p.Ink).SingleLine().Ellipsis("…")
	})
	if clicked(c, b) {
		pick()
		a.pop.open = ""
	}
}

func (a *App) popoverModels(c *mu.Context, p theme.Colors) {
	mu.Column(c).Padding(4, 6, 4, 32).Children(func() {
		n := 0
		for _, m := range models.Catalog() {
			if !m.FinalPass || m.IsRemote() || !a.modelStatus(m.ID).IsOnDisk() {
				continue
			}
			n++
			a.popoverChoice(c, p, "popover.model."+string(m.ID), m.Name, a.FinalModel() == m.ID, func() { a.UseModel(m.ID) })
		}
		if n == 0 {
			uiText(c, i18n.T("popover.no_model_downloaded"), 13, 400, p.Graphite).Padding(6, 10)
		}
		a.popoverChoice(c, p, "popover.manage_models", i18n.T("popover.manage_models"), false, func() {
			a.SetPage(PageModels)
			if a.d.Shell != nil {
				a.d.Shell.ShowWindow(WinPopover, false)
				a.d.Shell.ShowHub()
			}
		})
	})
}

func (a *App) popoverMics(c *mu.Context, p theme.Colors) {
	cur := ""
	if a.cfg.Audio.InputDevice != nil {
		cur = platform.NormalizeDeviceID(*a.cfg.Audio.InputDevice)
	}
	mu.Column(c).Padding(4, 6, 4, 32).Children(func() {
		a.popoverChoice(c, p, "popover.mic.default", i18n.T("popover.system_default"), cur == "", func() {
			a.cfg.Audio.InputDevice = nil
			a.saveConfig()
		})
		for _, d := range a.audioDevices() {
			a.popoverChoice(c, p, "popover.mic."+d.ID, d.Name, cur == d.ID, func() {
				a.cfg.Audio.InputDevice = ptr(d.ID)
				a.saveConfig()
			})
		}
	})
}

// micName is the microphone in use.
func (a *App) micName() string {
	if a.cfg.Audio.InputDevice != nil {
		id := platform.NormalizeDeviceID(*a.cfg.Audio.InputDevice)
		for _, d := range a.audioDevices() {
			if d.ID == id {
				return d.Name
			}
		}
	}
	return i18n.T("popover.system_default")
}
