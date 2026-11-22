package ui

import (
	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The tray popover: a themed panel under the menu bar or tray icon with
// the status, Start/Stop, the style switcher, Incognito, Open and Quit.

// Popover window size, in DIPs.
const (
	PopoverW = 300
	PopoverH = 480
)

type popoverState struct {
	stylesOpen bool
}

// PopoverView is the view of the tray popover.
func (a *App) PopoverView(c *mu.Context) {
	p := a.look(c)
	c.Root().Background(p.SheetRaised)
	mu.Column(c).Fill().Padding(14).Gap(10).Children(func() {
		mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
			mu.Icon(c, icon("wordmark")).FontSize(18).TextColor(p.Ink)
			mu.Text(c, i18n.T("popover.title")).Font(theme.Display).FontSize(19).FontWeight(600).TextColor(p.Ink).Grow(1)
			status, _, dot := a.engineSummary(p)
			mu.Box(c).Size(8, 8).Radius(99).Background(dot)
			mu.Text(c, status).FontSize(12).TextColor(p.Graphite)
		})
		a.loadHome()
		mu.Text(c, i18n.TP("popover.words_today", a.home.stats.WordsToday, map[string]any{"number": formatCount(a.home.stats.WordsToday)})).
			FontSize(12.5).TextColor(p.Graphite)
		label := i18n.T("popover.start_dictation")
		if a.Busy() {
			label = i18n.T("popover.stop_dictation")
		}
		b := mu.ButtonBase(a.k(c, "popover.toggle")).Label(label).Padding(10, 14).Radius(10).Justify(mu.Center).
			Background(p.InkFill).Cursor(mu.CursorPointer)
		b.Children(func() { mu.Text(c, label).FontSize(14).FontWeight(600).TextColor(p.OnInk) })
		if clicked(c, b) {
			a.Toggle()
		}
		rule(c, p)
		eyebrow(c, p, i18n.T("popover.style"))
		active := a.ActiveStyle()
		mu.Scroll(c.Key("popover.styles")).Grow(1).Basis(0).Gap(2).Role(mu.RoleRadioGroup).Label(i18n.T("popover.style")).Children(func() {
			for _, st := range a.styles.Styles() {
				st := st
				name := styleName(&st)
				r := mu.ButtonBase(a.k(c, "popover.style."+st.ID)).Role(mu.RoleRadio).Checked(st.ID == active).Label(name).
					Padding(6, 8).Gap(8).Radius(7).AlignItems(mu.Center).Cursor(mu.CursorPointer)
				if st.ID == active {
					r.Background(p.Deboss)
				} else if r.Hovered() {
					r.Background(p.Sheet)
				}
				r.Children(func() {
					seal(c, st.Ink, p)
					mu.Text(c, name).FontSize(13).TextColor(p.Ink).Grow(1)
				})
				if clicked(c, r) {
					a.SetActiveStyle(st.ID)
				}
			}
		})
		rule(c, p)
		mu.Row(c).AlignItems(mu.Center).Children(func() {
			mu.Column(c).Grow(1).Children(func() {
				mu.Text(c, i18n.T("popover.incognito")).FontSize(13.5).TextColor(p.Ink)
				if a.incognito {
					mu.Text(c, i18n.T("popover.not_saved")).FontSize(11.5).TextColor(p.Graphite)
				}
			})
			on := a.incognito
			mu.Switch(a.k(c, "popover.incognito"), &on).Label(i18n.T("popover.incognito")).OnChange(func() { a.SetIncognito(on) })
		})
		mu.Row(c).Gap(8).Children(func() {
			if a.btn(c, p, "popover.open", i18n.T("popover.open_sayso"), btnSecondary) {
				if a.d.Shell != nil {
					a.d.Shell.ShowWindow(WinPopover, false)
					a.d.Shell.ShowHub()
				}
			}
			mu.Spacer(c)
			if a.btn(c, p, "popover.quit", i18n.T("popover.quit"), btnGhost) && a.d.Shell != nil {
				a.d.Shell.Quit()
			}
		})
	})
}
