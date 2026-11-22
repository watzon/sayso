package ui

import (
	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The hub: the window is the ground, the sidebar sits on it, and the page
// is a sheet of paper laid over the right side, with two sheets peeking
// out under it (the stacked-paper look of the design).

// Hub window geometry.
const (
	HubWidth      = 1280
	HubHeight     = 820
	HubMinWidth   = 760
	HubMinHeight  = 560
	sidebarWidth  = 232
	railWidth     = 88
	collapseBelow = 920
)

// collapsed is true when the sidebar shows as the narrow rail.
func (a *App) collapsed(c *mu.Context) bool {
	w, _ := c.Size()
	return a.cfg.Appearance.SidebarCollapsed || w < collapseBelow
}

// HubView is the view of the hub window.
func (a *App) HubView(c *mu.Context) {
	p := a.look(c)
	c.Root().Background(p.Ground)
	a.hubShortcuts(c)
	rail := a.collapsed(c)
	mu.Row(c).Fill().AlignItems(mu.Stretch).Children(func() {
		if a.inSettings {
			a.settingsSidebar(c, p, rail)
		} else {
			a.sidebar(c, p, rail)
		}
		a.sheet(c, p)
	})
	a.hubDialogs(c, p)
}

func (a *App) hubShortcuts(c *mu.Context) {
	for i, pg := range Pages {
		if c.Shortcut(mu.Cmd, mu.Key1+mu.Key(i)) {
			a.SetPage(pg)
		}
	}
	if c.Shortcut(mu.Cmd, mu.KeyComma) {
		a.OpenSettings(a.section)
	}
	if c.Shortcut(0, mu.KeyEscape) {
		switch {
		case a.Recording():
			a.CancelDictation()
		case a.sty.editing:
			a.sty.close()
		case a.inSettings:
			a.CloseSettings()
		}
	}
}

// sheet is the page on its stack of paper.
func (a *App) sheet(c *mu.Context, p theme.Colors) {
	// The two sheets under the page, offset down and in, are paint only:
	// elements there would take the page's clicks.
	under := func(pt *mu.Painter, r mu.Rect) {
		for i, inset := range []float32{14, 7} {
			dy := float32(2-i) * 3
			sh := mu.Rect{X: r.X + inset, Y: r.Y + dy*2, W: r.W - 2*inset, H: r.H}
			pt.Fill(sh, p.Rule.Alpha(0.6), 12)
			pt.Fill(mu.Rect{X: sh.X + 1, Y: sh.Y + 1, W: sh.W - 2, H: sh.H - 2}, p.Sheet, 11)
		}
	}
	mu.Box(c).Grow(1).Margin(10, 10, 10, 0).Draw(under).Children(func() {
		page := mu.Column(c).Fill().Radius(12).Background(p.Sheet).Border(1, p.Rule).Clip().
			Shadow(0, 1, 2, 0, p.Shadow.Alpha(0.08)).Shadow(0, 8, 24, 0, p.Shadow.Alpha(0.06))
		page.Children(func() {
			if a.page == PageHistory && !a.inSettings {
				a.historyPage(c, p) // two panes that scroll on their own
				return
			}
			mu.Scroll(c.Key(pageKey{a.page, a.section})).Grow(1).Padding(40, 48, 40, 48).Gap(26).Children(func() {
				if a.flash != "" {
					mu.Text(c, a.flash).FontSize(13).TextColor(p.Danger)
				}
				if a.inSettings {
					a.settingsPage(c, p)
					return
				}
				switch a.page {
				case PageDictionary:
					a.dictionaryPage(c, p)
				case PageStyles:
					a.stylesPage(c, p)
				case PageModels:
					a.modelsPage(c, p)
				default:
					a.homePage(c, p)
				}
			})
		})
		if a.page == PageStyles && !a.inSettings {
			a.styleDrawers(c, p)
		}
	})
}

type pageKey struct {
	page    Page
	section Section
}

// sidebar is the main navigation, or the rail when narrow.
func (a *App) sidebar(c *mu.Context, p theme.Colors, rail bool) {
	bar := c.TitleBar()
	w := float32(sidebarWidth)
	if rail {
		w = railWidth
	}
	nav := mu.Column(c).Width(w).Padding(bar.Bottom+16, 12, 14, 12).Gap(3).DragWindow().
		Role(mu.RoleNavigation).Label(i18n.T("hub.sidebar.main_nav"))
	nav.Children(func() {
		mu.Row(c).Gap(8).Padding(0, 10, 16, 10).Justify(alignIf(rail)).Children(func() {
			mu.Icon(c, icon("wordmark")).FontSize(22).TextColor(p.Ink)
			if !rail {
				mu.Text(c, "Sayso").Font(theme.Display).FontSize(25).FontWeight(500).TextColor(p.Ink)
			}
		})
		for _, pg := range Pages {
			a.navItem(c, p, pg.key(), pg.icon(), pg.title(), a.page == pg && !a.inSettings, rail, func() { a.SetPage(pg) })
		}
		mu.Spacer(c)
		a.sidebarFooter(c, p, rail)
	})
}

func alignIf(center bool) mu.Align {
	if center {
		return mu.Center
	}
	return mu.Start
}

// navItem is one row of a sidebar.
func (a *App) navItem(c *mu.Context, p theme.Colors, key, iconName, label string, current, rail bool, onClick func()) {
	row := mu.ButtonBase(a.k(c, key)).Label(label).Padding(8, 10).Gap(10).Radius(8).
		AlignItems(mu.Center).Justify(alignIf(rail)).Cursor(mu.CursorPointer)
	if rail {
		row.Tooltip(label)
	}
	fg := p.Graphite
	if current {
		fg = p.Ink
		row.Current(mu.CurrentPage).Background(p.SheetRaised).Border(1, p.Rule).Shadow(0, 1, 1, 0, p.Shadow.Alpha(0.06))
	} else if row.Hovered() {
		row.Background(p.Deboss.Alpha(0.7))
	}
	row.Children(func() {
		mu.Icon(c, icon(iconName)).FontSize(17).TextColor(fg)
		if !rail {
			mu.Text(c, label).FontSize(14.5).FontWeight(500).TextColor(fg).Grow(1).SingleLine()
		}
	})
	if clicked(c, row) {
		onClick()
	}
}

// sidebarFooter: the engine status, the update slip, the theme control and
// the collapse button.
func (a *App) sidebarFooter(c *mu.Context, p theme.Colors, rail bool) {
	status, detail, dot := a.engineSummary(p)
	mu.Row(c).Gap(8).Padding(6, 10).AlignItems(mu.Center).Justify(alignIf(rail)).
		Label(i18n.T("hub.sidebar.engine_status", "status", status, "detail", detail)).Tooltip(detail).Children(func() {
		mu.Box(c).Size(8, 8).Radius(99).Background(dot)
		if !rail {
			mu.Text(c, status).FontSize(12.5).FontWeight(500).TextColor(p.Graphite).SingleLine().Grow(1)
		}
	})
	if a.updateOffered() && !rail {
		label := i18n.T("update.slip_version", "version", a.upd.status.Version.String())
		if a.btn(c, p, "update.slip", label, btnSecondary) {
			a.openUpdateWindow()
		}
	}
	modes := []config.ThemeMode{config.ThemeSystem, config.ThemeLight, config.ThemeDark}
	keys := []string{"theme.auto", "theme.light", "theme.dark"}
	labels := []string{i18n.T("hub.sidebar.theme_auto"), i18n.T("hub.sidebar.theme_light"), i18n.T("hub.sidebar.theme_dark")}
	chosen := 0
	for i, m := range modes {
		if m == a.cfg.Appearance.Theme {
			chosen = i
		}
	}
	if rail {
		icons := []string{"display", "sun", "moon"}
		mu.Column(c).Gap(2).AlignItems(mu.Center).Children(func() {
			for i := range modes {
				if a.iconBtn(c, p, keys[i], icons[i], labels[i]) {
					a.SetThemeMode(modes[i])
				}
			}
		})
	} else if i := a.choice(c, p, keys, labels, chosen, true); i >= 0 {
		a.SetThemeMode(modes[i])
	}
	label := i18n.T("hub.sidebar.collapse")
	if a.cfg.Appearance.SidebarCollapsed {
		label = i18n.T("hub.sidebar.expand")
	}
	mu.Row(c).Justify(alignIf(rail)).Children(func() {
		if a.iconBtn(c, p, "nav.collapse", "sidebar", label) {
			a.ToggleSidebar()
		}
	})
}

// engineSummary is the status line of the engine and its dot color.
func (a *App) engineSummary(p theme.Colors) (status, detail string, dot mu.Color) {
	switch {
	case a.d.Engine == nil:
		return i18n.T("app.engine.stopped"), i18n.T("overlay.notice.no_engine"), p.Danger
	case !a.d.Engine.Running():
		return i18n.T("app.engine.restarting"), i18n.T("overlay.notice.engine_starting"), p.Pencil
	}
	if a.Recording() {
		return i18n.T("overlay.a11y.recording"), a.modelName(a.FinalModel()), p.Accent
	}
	return i18n.T("app.engine.ready"), a.modelName(a.FinalModel()), p.Success
}

// settingsSidebar replaces the main sidebar while Settings shows.
func (a *App) settingsSidebar(c *mu.Context, p theme.Colors, rail bool) {
	bar := c.TitleBar()
	w := float32(sidebarWidth)
	if rail {
		w = railWidth
	}
	nav := mu.Column(c).Width(w).Padding(bar.Bottom+16, 12, 14, 12).Gap(2).DragWindow().
		Role(mu.RoleNavigation).Label(i18n.T("hub.sidebar.settings_sections"))
	nav.Children(func() {
		back := "‹ " + i18n.T("hub.nav.settings")
		if rail {
			back = "‹"
		}
		b := mu.ButtonBase(a.k(c, "settings.back")).Label(i18n.T("hub.sidebar.back")).Padding(6, 10).Radius(8).Justify(alignIf(rail)).Cursor(mu.CursorPointer)
		if b.Hovered() {
			b.Background(p.Deboss)
		}
		b.Children(func() { mu.Text(c, back).Font(theme.Display).FontSize(20).FontWeight(500).TextColor(p.Ink) })
		if clicked(c, b) {
			a.CloseSettings()
		}
		mu.Box(c).Height(8)
		for _, s := range Sections {
			a.navItem(c, p, s.key(), s.icon(), s.title(), a.section == s, rail, func() { a.OpenSettings(s) })
		}
		mu.Spacer(c)
		label := i18n.T("hub.sidebar.open_config_file")
		if rail {
			if a.iconBtn(c, p, "settings.open_config", "file", label) {
				a.openPath(a.d.ConfigPath)
			}
		} else if a.btn(c, p, "settings.open_config", label, btnGhost) {
			a.openPath(a.d.ConfigPath)
		}
	})
}

func (a *App) openPath(path string) {
	if a.d.Shell != nil && path != "" {
		a.d.Shell.OpenPath(path)
	}
}

func (a *App) openURL(url string) {
	if a.d.Shell != nil && url != "" {
		a.d.Shell.OpenURL(url)
	}
}

// hubDialogs are the modals drawn in the hub.
func (a *App) hubDialogs(c *mu.Context, p theme.Colors) {
	if a.set.confirmClear {
		a.confirmClearDialog(c, p)
	}
}
