package ui

import (
	"fmt"
	"log"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
)

type homeState struct {
	loaded bool
	stats  Stats
	recent []store.HistoryEntry
	words  int
	rules  int
}

// dataChanged makes every page read the store again.
func (a *App) dataChanged() {
	a.home.loaded = false
	a.hist.loaded = false
	a.dict.loaded = false
	a.publish()
}

func (a *App) loadHome() {
	if a.home.loaded {
		return
	}
	a.home = homeState{loaded: true}
	s := a.d.Store
	if s == nil {
		return
	}
	now := a.now()
	if entries, err := s.StatsSince(now.AddDate(0, 0, -8)); err == nil {
		a.home.stats = ComputeStats(entries, now)
	} else {
		log.Printf("stats: %v", err)
	}
	if recent, err := s.List(store.HistoryQuery{Limit: 6}); err == nil {
		a.home.recent = recent
	}
	if w, err := s.ListWords(); err == nil {
		a.home.words = len(w)
	}
	if r, err := s.ListReplacements(); err == nil {
		a.home.rules = len(r)
	}
}

// greeting is the Home title for the time of day.
func greeting(now time.Time, name string) string {
	part := "evening"
	switch h := now.Hour(); {
	case h < 12:
		part = "morning"
	case h < 18:
		part = "afternoon"
	}
	if name != "" {
		return i18n.T("home.greeting."+part+"_name", "name", name)
	}
	return i18n.T("home.greeting." + part)
}

func (a *App) homePage(c *mu.Context, p theme.Colors) {
	a.loadHome()
	now := a.now()
	w, _ := c.Size()
	wide := w >= 1100
	mu.Row(c).Gap(32).AlignItems(mu.Start).Children(func() {
		mu.Column(c).Gap(24).Grow(1).Basis(0).Children(func() {
			mu.Row(c).AlignItems(mu.End).Gap(16).Wrap().Children(func() {
				mu.Column(c).Gap(6).Grow(1).Children(func() {
					eyebrow(c, p, i18n.WeekdayDayMonth(now))
					heading(c, p, greeting(now, a.cfg.General.DisplayName()), 40).Role(mu.RoleHeading)
				})
				a.hotkeyHint(c, p)
			})
			a.dictateStrip(c, p)
			rule(c, p)
			if a.cfg.History.Enabled {
				a.statsStrip(c, p)
			} else {
				a.historyOffCard(c, p)
			}
			a.recentList(c, p)
			if !wide {
				a.homeSide(c, p)
			}
		})
		if wide {
			mu.Column(c).Width(300).Gap(20).Children(func() { a.homeSide(c, p) })
		}
	})
}

func (a *App) hotkeyHint(c *mu.Context, p theme.Colors) {
	mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
		h := a.cfg.Hotkeys.Toggle
		if h == nil {
			mu.Text(c, i18n.T("home.hint.no_hotkey")).FontSize(14).TextColor(p.Graphite)
			if a.btn(c, p, "home.set_hotkey", i18n.T("home.hint.set_hotkey"), btnGhost) {
				a.OpenSettings(SecDictation)
			}
			return
		}
		mu.Text(c, i18n.T("home.hint.press")).FontSize(14).TextColor(p.Graphite)
		for _, k := range h.Keycaps() {
			keycap(c, p, k)
		}
		mu.Text(c, i18n.T("home.hint.anywhere")).FontSize(14).TextColor(p.Graphite)
	})
}

// dictateStrip starts and stops a dictation from the hub, with the live
// state beside the button.
func (a *App) dictateStrip(c *mu.Context, p theme.Colors) {
	mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
		label := i18n.T("popover.start_dictation")
		if a.Busy() {
			label = i18n.T("popover.stop_dictation")
		}
		if a.btn(c, p, "home.dictate", label, btnPrimary) {
			a.Toggle()
		}
		if a.Busy() {
			if a.btn(c, p, "home.cancel", i18n.T("styles.cancel"), btnSecondary) {
				a.CancelDictation()
			}
		}
		switch a.state.Kind {
		case dictation.StateRecording:
			waveform(c, a.levelsInOrder()[24:], p.Ink).Width(140).Height(24)
			mu.Text(c, a.elapsedLabel()).Font(theme.Mono).FontSize(13).TextColor(p.Graphite)
		case dictation.StateProcessing:
			mu.Spinner(c)
			mu.Text(c, a.stageLabel()).FontSize(13).TextColor(p.Graphite)
		}
		if a.incognito {
			slip(c, p, i18n.T("overlay.incognito"), p.Accent)
		}
	})
}

func (a *App) elapsedLabel() string {
	d := a.now().Sub(a.stateAt)
	if d < 0 {
		d = 0
	}
	return fmt.Sprintf("%d:%02d", int(d.Minutes()), int(d.Seconds())%60)
}

func (a *App) stageLabel() string {
	switch a.state.Stage {
	case dictation.StageEnhancing:
		return i18n.T("overlay.stage.applying_style")
	case dictation.StageInserting:
		return i18n.T("overlay.stage.inserting")
	}
	return i18n.T("overlay.stage.transcribing")
}

func (a *App) statsStrip(c *mu.Context, p theme.Colors) {
	s := a.home.stats
	stat := func(label, value string, first bool) {
		col := mu.Column(c).Gap(6).Grow(1).Basis(0)
		if !first {
			col.PaddingStart(20).BorderStartWidth(1).BorderColor(p.Rule)
		}
		col.Children(func() {
			mu.Text(c, label).FontSize(12.5).TextColor(p.Graphite)
			if value == "" {
				mu.Text(c, i18n.T("home.stats.none_yet")).Font(theme.Display).FontSize(22).TextColor(p.Pencil)
				return
			}
			mu.Text(c, value).Font(theme.Display).FontSize(28).FontWeight(600).TextColor(p.Ink).SingleLine()
		})
	}
	mu.Row(c).Gap(20).Children(func() {
		stat(i18n.T("home.stats.words_today"), formatCount(s.WordsToday), true)
		stat(i18n.T("home.stats.time_saved"), i18n.T("home.stats.minutes", "minutes", s.MinutesSavedToday), false)
		pace := ""
		if s.PaceWPM > 0 {
			pace = i18n.T("home.stats.wpm", "wpm", s.PaceWPM)
		}
		stat(i18n.T("home.stats.pace"), pace, false)
		mu.Column(c).Gap(6).Grow(1).Basis(0).PaddingStart(20).BorderStartWidth(1).BorderColor(p.Rule).Children(func() {
			mu.Text(c, i18n.T("home.stats.this_week")).FontSize(12.5).TextColor(p.Graphite)
			a.weekBars(c, p, s)
		})
	})
}

func (a *App) weekBars(c *mu.Context, p theme.Colors, s Stats) {
	most := 1
	for _, n := range s.WeekByDay {
		most = max(most, n)
	}
	mu.Box(c).Height(36).Label(formatCount(s.WordsThisWeek)).Draw(func(pt *mu.Painter, r mu.Rect) {
		w := (r.W - 6*4) / 7
		for i, n := range s.WeekByDay {
			h := max(r.H*float32(n)/float32(most), 2)
			col := p.Pencil
			if i == s.TodayIndex {
				col = p.Ink
			}
			pt.Fill(mu.Rect{X: r.X + float32(i)*(w+4), Y: r.Y + r.H - h, W: w, H: h}, col, 2)
		}
	})
}

func (a *App) historyOffCard(c *mu.Context, p theme.Colors) {
	card(c, p).Children(func() {
		mu.Text(c, i18n.T("home.history_off.title")).FontSize(15).FontWeight(600).TextColor(p.Ink)
		mu.Text(c, i18n.T("home.history_off.block_detail")).FontSize(13).TextColor(p.Graphite)
		mu.Row(c).Gap(8).Children(func() {
			if a.btn(c, p, "home.history_on", i18n.T("home.history_off.turn_on"), btnPrimary) {
				a.cfg.History.Enabled = true
				a.saveConfig()
			}
			if a.btn(c, p, "home.history_settings", i18n.T("home.history_off.open_settings"), btnGhost) {
				a.OpenSettings(SecHistory)
			}
		})
	})
}

func (a *App) recentList(c *mu.Context, p theme.Colors) {
	mu.Column(c).Gap(8).Children(func() {
		mu.Row(c).AlignItems(mu.Center).Children(func() {
			eyebrow(c, p, i18n.T("home.recent.title"))
			mu.Spacer(c)
			if len(a.home.recent) > 0 && a.btn(c, p, "home.open_history", i18n.T("home.recent.open_history"), btnGhost) {
				a.SetPage(PageHistory)
			}
		})
		if len(a.home.recent) == 0 {
			hk := i18n.T("home.recent.your_hotkey")
			if h := a.cfg.Hotkeys.Toggle; h != nil {
				hk = h.CompactLabel()
			}
			card(c, p).Children(func() {
				mu.Text(c, i18n.T("home.recent.empty_title")).FontSize(15).FontWeight(600).TextColor(p.Ink)
				mu.Text(c, i18n.T("home.recent.empty_body", "hotkey", hk)).FontSize(13).TextColor(p.Graphite)
			})
			return
		}
		mu.Column(c).Radius(12).Background(p.SheetRaised).Border(1, p.Rule).Dividers(1, p.Rule).Clip().Children(func() {
			for i, e := range a.home.recent {
				a.recentRow(c, p, i, e)
			}
		})
	})
}

func (a *App) recentRow(c *mu.Context, p theme.Colors, i int, e store.HistoryEntry) {
	app := i18n.T("home.recent.unknown_app")
	if e.App != nil && e.App.Name != "" {
		app = e.App.Name
	}
	r := mu.ButtonBase(a.k(c, fmt.Sprintf("home.recent.%d", i))).Label(app+", "+e.FinalText).
		Padding(10, 14).Gap(12).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	if r.Hovered() {
		r.Background(p.Sheet)
	}
	r.Children(func() {
		mu.Text(c, e.CreatedAt.In(a.now().Location()).Format("15:04")).Font(theme.Mono).FontSize(12).TextColor(p.Pencil).Width(44)
		mu.Text(c, e.FinalText).Font(theme.Body).FontSize(14).TextColor(p.Ink).SingleLine().Ellipsis("…").Grow(1).Basis(0)
		mu.Text(c, app).FontSize(12).TextColor(p.Graphite).SingleLine()
	})
	if clicked(c, r) {
		a.hist.selected = e.ID
		a.SetPage(PageHistory)
	}
}

// homeSide is the Default Style card and the checklist.
func (a *App) homeSide(c *mu.Context, p theme.Colors) {
	st, _ := a.styles.Get(a.ActiveStyle())
	card(c, p).Children(func() {
		eyebrow(c, p, i18n.T("home.style.default"))
		if st != nil {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
				seal(c, st.Ink, p)
				mu.Text(c, styleName(st)).Font(theme.Display).FontSize(20).FontWeight(600).TextColor(p.Ink)
			})
			mu.Text(c, styleDescription(st)).FontSize(13).TextColor(p.Graphite)
			mu.Text(c, a.styleRoute(st)).FontSize(12).TextColor(p.Pencil)
		}
		if a.btn(c, p, "home.change_style", i18n.T("home.style.change"), btnSecondary) {
			a.SetPage(PageStyles)
		}
	})
	card(c, p).Gap(4).Children(func() {
		items := a.checklist()
		all := true
		for _, it := range items {
			all = all && it.ok
		}
		head := i18n.T("home.checklist.needs_attention")
		if all {
			head = i18n.T("home.checklist.all_set")
		}
		eyebrow(c, p, head)
		for _, it := range items {
			a.checkRow(c, p, it)
		}
	})
}

type checkItem struct {
	key, title, detail string
	ok                 bool
	action             string
	do                 func()
}

func (a *App) checklist() []checkItem {
	var perm struct{ mic, ax bool }
	if a.d.Platform != nil && a.d.Platform.Permissions != nil {
		s := a.d.Platform.Permissions.Status()
		perm.mic, perm.ax = s.Microphone, s.Accessibility
	}
	request := func(kind string) func() {
		return func() {
			if a.d.Platform != nil && a.d.Platform.Permissions != nil {
				pm := a.d.Platform.Permissions
				go func() {
					if kind == "microphone" {
						pm.RequestMicrophone()
					} else {
						pm.RequestAccessibility()
					}
				}()
			}
		}
	}
	allowed := func(ok bool) string {
		if ok {
			return i18n.T("home.checklist.allowed")
		}
		return ""
	}
	model := a.FinalModel()
	ms := a.modelStatus(model)
	modelDetail := a.modelName(model)
	switch ms.State {
	case engine.StateDownloading:
		modelDetail = i18n.T("home.checklist.downloading", "percent", int(ms.Fraction*100))
	}
	engineDetail := i18n.T("app.engine.ready")
	if !a.engineRunning() {
		engineDetail = i18n.T("home.checklist.engine_stopped")
	}
	return []checkItem{
		{"home.check.mic", i18n.T("home.checklist.microphone"), allowed(perm.mic), perm.mic, i18n.T("home.checklist.allow"), request("microphone")},
		{"home.check.ax", i18n.TOS("os.permission.accessibility", nil), allowed(perm.ax), perm.ax, i18n.T("home.checklist.allow"), request("accessibility")},
		{"home.check.model", i18n.T("home.checklist.model"), modelDetail, ms.IsOnDisk(), i18n.T("home.checklist.open_models"), func() { a.SetPage(PageModels) }},
		{"home.check.dictionary", i18n.T("home.checklist.dictionary"), i18n.T("home.checklist.dictionary_summary",
			"words", i18n.TP("home.checklist.words", a.home.words, nil), "rules", i18n.TP("home.checklist.rules", a.home.rules, nil)),
			a.home.words > 0, i18n.T("home.checklist.add_words"), func() { a.SetPage(PageDictionary) }},
		{"home.check.engine", i18n.T("home.checklist.engine"), engineDetail, a.engineRunning(), "", nil},
	}
}

func (a *App) checkRow(c *mu.Context, p theme.Colors, it checkItem) {
	mu.Row(c).Gap(10).Padding(6, 0).AlignItems(mu.Center).Children(func() {
		mark, col := "warning", p.Danger
		if it.ok {
			mark, col = "check", p.Success
		}
		mu.Icon(c, icon(mark)).FontSize(15).TextColor(col)
		mu.Column(c).Gap(1).Grow(1).Basis(0).Children(func() {
			mu.Text(c, it.title).FontSize(13.5).FontWeight(500).TextColor(p.Ink)
			if it.detail != "" {
				mu.Text(c, it.detail).FontSize(12).TextColor(p.Graphite).SingleLine().Ellipsis("…")
			}
		})
		if !it.ok && it.do != nil && a.btn(c, p, it.key, it.action, btnGhost) {
			it.do()
		}
	})
}

// seal is a style's ink dot.
func seal(c *mu.Context, hex string, p theme.Colors) {
	col := p.Ink
	if rgb, ok := theme.ParseRgb(hex); ok {
		col = theme.UIColor(rgb)
	}
	mu.Box(c).Size(12, 12).Radius(99).Background(col)
}

// styleRoute is where a style runs, as one line.
func (a *App) styleRoute(st interface{ UsesAI() bool }) string {
	if !st.UsesAI() {
		return i18n.T("styles.route.no_ai", "computer", computer())
	}
	if !a.cfg.AI.Enabled {
		return i18n.T("styles.route.needs_provider")
	}
	prov, ok := a.defaultProvider()
	if !ok {
		return i18n.T("styles.route.needs_provider")
	}
	if prov.NeedsModel() {
		return i18n.T("styles.route.needs_model", "provider", prov.Name)
	}
	key := "styles.route.local"
	if prov.IsCloud() {
		key = "styles.route.cloud"
	}
	return i18n.T(key, "provider", prov.Name, "model", prov.ModelName())
}

func (a *App) defaultProvider() (config.Provider, bool) {
	if a.cfg.AI.DefaultProvider != nil {
		if pr, ok := a.cfg.AI.Provider(*a.cfg.AI.DefaultProvider); ok {
			return pr, true
		}
	}
	if len(a.cfg.AI.Providers) > 0 {
		return a.cfg.AI.Providers[0], true
	}
	return config.Provider{}, false
}
