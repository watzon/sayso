package ui

import (
	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The hub: the window is the board, the sidebar sits on it, and the page
// is a sheet of paper laid over the right side, with two sheets peeking
// out under its right and bottom edges (crates/sayso-app/src/hub/mod.rs).

// Hub window geometry.
const (
	HubWidth      = 1280
	HubHeight     = 820
	HubMinWidth   = 760
	HubMinHeight  = 560
	sidebarWidth  = 232
	railWidth     = 88
	collapseBelow = 920
	// sheetRadius is the corner of the page sheet.
	sheetRadius = 14
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
	mu.Row(c).Fill().AlignItems(mu.Stretch).Draw(a.grain(p, "board", 0)).Children(func() {
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
		case a.sty.editing || a.sty.baseOpen:
			a.sty.close()
		case a.inSettings:
			a.CloseSettings()
		}
	}
}

// underSheets paints the two sheets under the page; they are paint only,
// as elements there would take the page's clicks. r is the content column
// with its 10 px padding.
func underSheets(p theme.Colors) func(*mu.Painter, mu.Rect) {
	back := theme.Over(p.Sheet, p.Ground, 0.45)
	middle := theme.Over(p.Sheet, p.Ground, 0.25)
	return func(pt *mu.Painter, r mu.Rect) {
		top := r.Y + 10
		for _, s := range []struct {
			dy, right, bottom, left float32
			fill                    mu.Color
		}{{6, 6, 4, 6, back}, {3, 8, 6, 2, middle}} {
			sh := mu.Rect{X: r.X + s.left, Y: top + s.dy, W: r.W - s.left - s.right, H: r.Y + r.H - s.bottom - top - s.dy}
			pt.Shadow(sh, sheetRadius, 0, 1, 2, 0, p.ShadowA(0.12))
			pt.Fill(sh, s.fill, sheetRadius)
		}
	}
}

// sheet is the page on its stack of paper.
func (a *App) sheet(c *mu.Context, p theme.Colors) {
	top := float32(10)
	if bar := c.TitleBar(); bar.Bottom > 0 && bar.Left == 0 {
		top = max(top, bar.Bottom) // Windows' caption, a custom Linux title bar
	}
	mu.Box(c).Grow(1).Basis(0).Padding(top, 10, 10, 0).Draw(underSheets(p)).Children(func() {
		page := sheetStack(mu.Column(c).Fill().Radius(sheetRadius).Clip(), p).Draw(a.grain(p, "sheet", sheetRadius)).
			Role(mu.RoleMain).Label(a.routeName())
		page.Children(func() {
			if a.page == PageHistory && !a.inSettings {
				a.historyPage(c, p) // two panes that scroll on their own
				return
			}
			w, _ := c.Size()
			px := float32(52)
			if w-sidebarWidth < 820 {
				px = 32
			}
			top, bottom, gap := float32(44), float32(44), float32(40)
			switch {
			case a.inSettings:
				bottom, gap = 32, 28
			case a.page == PageDictionary, a.page == PageModels:
				gap = 32
			case a.page == PageStyles:
				gap = 26
			}
			mu.Scroll(c.Key(pageKey{a.page, a.section})).Grow(1).Padding(top, px, bottom, px).Gap(gap).Children(func() {
				if a.flash != "" {
					uiText(c, a.flash, 13, 400, p.Danger)
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

// routeName names the page for assistive technology.
func (a *App) routeName() string {
	if a.inSettings {
		return a.section.title()
	}
	return a.page.title()
}

type pageKey struct {
	page    Page
	section Section
}

// sidebarColumn is the column both sidebars share.
func (a *App) sidebarColumn(c *mu.Context, rail bool, label string) mu.Element {
	w := float32(sidebarWidth)
	if rail {
		w = railWidth
	}
	top := float32(52) // room for the traffic lights
	if bar := c.TitleBar(); bar.Left == 0 && bar.Bottom > 0 {
		top = bar.Bottom + 16
	}
	return mu.Column(c).Width(w).Shrink(0).Padding(top, 14, 16, 14).Gap(24).DragWindow().
		Role(mu.RoleNavigation).Label(label)
}

// sidebar is the main navigation, or the rail when narrow.
func (a *App) sidebar(c *mu.Context, p theme.Colors, rail bool) {
	a.sidebarColumn(c, rail, i18n.T("hub.sidebar.main_nav")).Children(func() {
		mu.Row(c).Height(30).PaddingX(8).Gap(10).AlignItems(mu.Center).Justify(alignIf(rail)).Children(func() {
			mu.Icon(c, icon("wordmark")).FontSize(22).TextColor(p.Ink)
			if !rail {
				mu.Text(c, "Sayso").Font(theme.Display).FontSize(24).FontWeight(600).TextColor(p.Ink)
			}
		})
		mu.Column(c).Gap(2).Children(func() {
			for _, pg := range Pages {
				a.navItem(c, p, pg.key(), pg.icon(), pg.title(), a.page == pg && !a.inSettings, rail, func() { a.SetPage(pg) })
			}
		})
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

// navItem is one row of a sidebar: pressed into the paper when current.
func (a *App) navItem(c *mu.Context, p theme.Colors, key, iconName, label string, current, rail bool, onClick func()) {
	row := mu.ButtonBase(a.k(c, key)).Label(label).Height(36).Radius(9).Gap(12).
		AlignItems(mu.Center).Justify(alignIf(rail)).Cursor(mu.CursorPointer)
	if rail {
		row.Width(36).AlignSelf(mu.Center).Tooltip(label)
	} else {
		row.PaddingX(10)
	}
	fg, weight := p.Graphite, 500
	if current {
		fg, weight = p.Ink, 600
		row.Current(mu.CurrentPage).Background(p.Deboss).InsetShadow(0, 1, 2.5, 0, p.ShadowA(0.22)).InsetShadow(0, -1, 0, 0, p.Highlight(0.7))
	} else if row.Hovered() {
		fg = p.Ink
		row.Background(p.Deboss.Alpha(0.55))
	}
	row.Children(func() {
		mu.Icon(c, icon(iconName)).FontSize(18).TextColor(fg)
		if !rail {
			uiText(c, label, 14, weight, fg).Grow(1).SingleLine()
		}
	})
	if clicked(c, row) {
		onClick()
	}
}

// collapseBtn folds the sidebar to the rail and back.
func (a *App) collapseBtn(c *mu.Context, p theme.Colors) {
	label := i18n.T("hub.sidebar.collapse")
	if a.cfg.Appearance.SidebarCollapsed {
		label = i18n.T("hub.sidebar.expand")
	}
	if a.iconBtn(c, p, "nav.collapse", "sidebar", label) {
		a.ToggleSidebar()
	}
}

// sidebarFooter: the engine status with the collapse button, the update
// slip and the theme control.
func (a *App) sidebarFooter(c *mu.Context, p theme.Colors, rail bool) {
	status, detail, dot := a.engineSummary(p)
	mu.Column(c).Gap(14).PaddingX(4).Children(func() {
		mu.Row(c).Gap(10).PaddingX(6).AlignItems(mu.Center).Justify(alignIf(rail)).Children(func() {
			if rail {
				mu.Column(c).Gap(14).AlignItems(mu.Center).Children(func() {
					mu.Box(c).Label(i18n.T("hub.sidebar.engine_status", "status", status, "detail", detail)).Tooltip(detail).
						Children(func() { statusDot(c, dot) })
					a.collapseBtn(c, p)
				})
				return
			}
			mu.Row(c).Gap(10).Grow(1).AlignItems(mu.Center).Label(i18n.T("hub.sidebar.engine_status", "status", status, "detail", detail)).
				Children(func() {
					statusDot(c, dot)
					mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
						uiText(c, status, 13, 600, p.Ink).SingleLine()
						uiText(c, detail, 12, 400, p.Graphite).SingleLine().Ellipsis("…")
					})
				})
			a.collapseBtn(c, p)
		})
		if a.updateOffered() && !rail {
			label := i18n.T("update.slip_version", "version", a.upd.status.Version.String())
			if a.button(c, p, "update.slip", label, bopt{size: sizeSmall, icon: "download"}) {
				a.openUpdateWindow()
			}
		}
		modes := []config.ThemeMode{config.ThemeSystem, config.ThemeLight, config.ThemeDark}
		keys := []string{"theme.auto", "theme.light", "theme.dark"}
		labels := []string{i18n.T("hub.sidebar.theme_auto"), i18n.T("hub.sidebar.theme_light"), i18n.T("hub.sidebar.theme_dark")}
		icons := []string{"display", "sun", "moon"}
		chosen := 0
		for i, m := range modes {
			if m == a.cfg.Appearance.Theme {
				chosen = i
			}
		}
		if rail {
			mu.Column(c).Gap(2).AlignItems(mu.Center).Children(func() {
				for i := range modes {
					if a.iconBtn(c, p, keys[i], icons[i], labels[i]) {
						a.SetThemeMode(modes[i])
					}
				}
			})
		} else if i := a.segmented(c, p, keys, labels, chosen, segOpt{fill: true, compact: true, icons: icons}); i >= 0 {
			a.SetThemeMode(modes[i])
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
	a.sidebarColumn(c, rail, i18n.T("hub.sidebar.settings_sections")).Children(func() {
		b := mu.ButtonBase(a.k(c, "settings.back")).Label(i18n.T("hub.sidebar.back")).Height(32).PaddingX(8).Gap(8).Radius(8).
			AlignItems(mu.Center).Justify(alignIf(rail)).Cursor(mu.CursorPointer)
		if b.Hovered() {
			b.Background(p.Deboss.Alpha(0.55))
		}
		b.Children(func() {
			mu.Icon(c, icon("chevron-left")).FontSize(16).TextColor(p.Ink)
			if !rail {
				mu.Text(c, i18n.T("hub.nav.settings")).Font(theme.Display).FontSize(22).FontWeight(600).TextColor(p.Ink)
			}
		})
		if clicked(c, b) {
			a.CloseSettings()
		}
		mu.Column(c).Gap(2).Children(func() {
			for _, s := range Sections {
				a.navItem(c, p, s.key(), s.icon(), s.title(), a.section == s, rail, func() { a.OpenSettings(s) })
			}
		})
		mu.Spacer(c)
		label := i18n.T("hub.sidebar.open_config_file")
		if rail {
			mu.Column(c).Gap(8).AlignItems(mu.Center).Children(func() {
				if a.iconBtn(c, p, "settings.open_config", "file", label) {
					a.openPath(a.d.ConfigPath)
				}
				a.collapseBtn(c, p)
			})
			return
		}
		mu.Row(c).Gap(8).AlignItems(mu.End).Children(func() {
			mu.Column(c).Gap(4).PaddingX(10).Grow(1).Basis(0).Children(func() {
				if a.link(c, p, "settings.open_config", label, 13, 600, "") {
					a.openPath(a.d.ConfigPath)
				}
				if a.d.ConfigPath != "" {
					mu.Text(c, tildePath(a.d.ConfigPath)).Font(theme.Mono).FontSize(11).TextColor(p.Graphite).LineHeight(1.45).Selectable()
				}
			})
			a.collapseBtn(c, p)
		})
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
