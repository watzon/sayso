package ui

import (
	"fmt"
	"log"
	"math"
	"strings"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/platform"
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

// dataChanged makes every page read the store again, and redraws every
// window, so a hub or popover that shows the data reads it now rather
// than at the next input.
func (a *App) dataChanged() {
	a.home.loaded = false
	a.hist.loaded = false
	a.dict.loaded = false
	a.sty.names = nil
	a.publish()
	a.invalidate()
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
	narrow := w-sidebarWidth < 820
	greetSize := float32(44)
	if narrow {
		greetSize = 36
	}
	mu.Row(c).AlignItems(mu.End).Justify(mu.SpaceBetween).Gap(24).Children(func() {
		mu.Column(c).Gap(10).Grow(1).Basis(0).MinWidth(0).Children(func() {
			uiText(c, strings.ToUpper(i18n.WeekdayDayMonth(now)), 13, 500, p.Graphite).SingleLine()
			display(c, p, greeting(now, a.cfg.General.DisplayName()), greetSize).Role(mu.RoleHeading).SingleLine().Ellipsis("…")
		})
		mu.Box(c).Padding(0, 0, 6, 0).Shrink(0).Children(func() { a.hotkeyHint(c, p) })
	})
	if a.cfg.History.Enabled {
		a.statsStrip(c, p, narrow)
	} else {
		a.historyOffCard(c, p)
	}
	body := mu.Row(c).Gap(40).AlignItems(mu.Start)
	if narrow {
		body.Column()
	}
	body.Children(func() {
		mu.Column(c).Grow(1).Basis(0).MinWidth(0).Children(func() { a.recentList(c, p) })
		side := mu.Column(c).Gap(28)
		if narrow {
			side.Grow(1)
		} else {
			side.Width(300).Shrink(0)
		}
		side.Children(func() { a.homeSide(c, p) })
	})
}

// hotkeyHint is "Press ⌥ Space anywhere to dictate"; it is also the button
// that starts and stops a dictation from the hub.
func (a *App) hotkeyHint(c *mu.Context, p theme.Colors) {
	h := a.cfg.Hotkeys.Toggle
	if h == nil {
		mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
			uiText(c, i18n.T("home.hint.no_hotkey"), 14, 400, p.Graphite)
			if a.link(c, p, "home.set_hotkey", i18n.T("home.hint.set_hotkey"), 14, 500, "") {
				a.OpenSettings(SecDictation)
			}
		})
		return
	}
	label := i18n.T("popover.start_dictation")
	if a.Busy() {
		label = i18n.T("popover.stop_dictation")
	}
	b := mu.ButtonBase(a.k(c, "home.dictate")).Label(label).Gap(8).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	b.Children(func() {
		uiText(c, i18n.T("home.hint.press"), 14, 400, p.Graphite)
		keycaps(c, p, h.Keycaps(), false)
		uiText(c, i18n.T("home.hint.anywhere"), 14, 400, p.Graphite)
	})
	if clicked(c, b) {
		a.Toggle()
	}
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

// statsStrip is the four figures between two rules.
func (a *App) statsStrip(c *mu.Context, p theme.Colors, narrow bool) {
	s := a.home.stats
	size := float32(40)
	if narrow {
		size = 34
	}
	cell := func(label string, grow float32, first bool, value func()) {
		col := mu.Column(c).Gap(6).Grow(grow).Basis(0).MinWidth(0)
		if !first {
			col.PaddingStart(24).BorderStartWidth(1).BorderColor(p.Rule)
		}
		col.Children(func() {
			uiText(c, label, 13, 500, p.Graphite).LineHeight(16.0 / 13).SingleLine()
			value()
		})
	}
	figure := func(v string) func() {
		return func() {
			if v == "" {
				heading(c, p, i18n.T("home.stats.none_yet"), 32).Opacity(0.45).SingleLine()
				return
			}
			display(c, p, v, size).SingleLine()
		}
	}
	pace := ""
	if s.PaceWPM > 0 {
		pace = i18n.T("home.stats.wpm", "wpm", s.PaceWPM)
	}
	mu.Row(c).PaddingY(24).BorderWidth(1, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		cell(i18n.T("home.stats.words_today"), 1, true, figure(formatCount(s.WordsToday)))
		cell(i18n.T("home.stats.time_saved"), 1, false, figure(i18n.T("home.stats.minutes", "minutes", s.MinutesSavedToday)))
		cell(i18n.T("home.stats.pace"), 1, false, figure(pace))
		cell(i18n.T("home.stats.this_week"), 1.3, false, func() {
			mu.Row(c).AlignItems(mu.End).Gap(12).Children(func() {
				mu.Box(c).Grow(1).MinWidth(0).Children(figure(formatCount(s.WordsThisWeek)))
				a.weekBars(c, p, s)
			})
		})
	})
}

// weekBars are the words of each day this week: today in ink, past days
// in shade, the days to come a stub.
func (a *App) weekBars(c *mu.Context, p theme.Colors, s Stats) {
	most := 1
	for _, n := range s.WeekByDay {
		most = max(most, n)
	}
	mu.Box(c).Size(7*8+6*6, 44).Shrink(0).Label(formatCount(s.WordsThisWeek)).Draw(func(pt *mu.Painter, r mu.Rect) {
		for i, n := range s.WeekByDay {
			h, col := float32(6), p.Rule
			if n > 0 && i <= s.TodayIndex {
				h = float32(math.Round(float64(6 + 34*float32(n)/float32(most))))
				col = p.DebossShade
				if i == s.TodayIndex {
					col = p.Ink
				}
			}
			pt.Fill(mu.Rect{X: r.X + float32(i)*14, Y: r.Y + r.H - h, W: 8, H: h}, col, 3)
		}
	})
}

func (a *App) historyOffCard(c *mu.Context, p theme.Colors) {
	card(c, p).Children(func() {
		uiText(c, i18n.T("home.history_off.title"), 15, 600, p.Ink)
		uiText(c, i18n.T("home.history_off.block_detail"), 13, 400, p.Graphite)
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
	mu.Row(c).AlignItems(mu.Center).Padding(0, 0, 8, 0).Children(func() {
		heading(c, p, i18n.T("home.recent.title"), 22).Role(mu.RoleHeading).Grow(1)
		if len(a.home.recent) > 0 && a.link(c, p, "home.open_history", i18n.T("home.recent.open_history"), 14, 500, "") {
			a.SetPage(PageHistory)
		}
	})
	if len(a.home.recent) == 0 {
		hk := i18n.T("home.recent.your_hotkey")
		if h := a.cfg.Hotkeys.Toggle; h != nil {
			hk = h.CompactLabel()
		}
		mu.Column(c).Gap(4).PaddingY(16).BorderWidth(1, 0, 1, 0).BorderColor(p.Rule).Children(func() {
			uiText(c, i18n.T("home.recent.empty_title"), 15, 600, p.Ink)
			uiText(c, i18n.T("home.recent.empty_body", "hotkey", hk), 13, 400, p.Graphite)
		})
		return
	}
	mu.Column(c).Role(mu.RoleList).Children(func() {
		for i, e := range a.home.recent {
			a.recentRow(c, p, i, e)
		}
	})
}

// clockOf is an entry's time as kit::clock writes it: "9:41", "13:05".
func (a *App) clockOf(t time.Time) string {
	return strings.TrimPrefix(t.In(a.now().Location()).Format("15:04"), "0")
}

// clockOrDay is the time of an entry from today, or its short day.
func (a *App) clockOrDay(t time.Time) string {
	now := a.now()
	l := t.In(now.Location())
	if y1, m1, d1 := l.Date(); func() bool { y2, m2, d2 := now.Date(); return y1 == y2 && m1 == m2 && d1 == d2 }() {
		return a.clockOf(t)
	}
	return i18n.DayMonthShort(l)
}

// durationClock is a length as "0:07" or "1:32".
func durationClock(ms uint64) string {
	s := (ms + 500) / 1000
	return fmt.Sprintf("%d:%02d", s/60, s%60)
}

// entryStyle is the name and ink of the style an entry ran.
func (a *App) entryStyle(e store.HistoryEntry) (string, string) {
	id := e.StyleID
	if id == "" {
		id = "raw"
	}
	if st, ok := a.styles.Get(id); ok {
		return styleName(st), st.Ink
	}
	return id, ""
}

func (a *App) recentRow(c *mu.Context, p theme.Colors, i int, e store.HistoryEntry) {
	app := i18n.T("home.recent.unknown_app")
	bundle := ""
	if e.App != nil && e.App.Name != "" {
		app, bundle = e.App.Name, e.App.BundleID
	}
	r := mu.ButtonBase(a.k(c, fmt.Sprintf("home.recent.%d", i))).Label(app+", "+e.FinalText).Role(mu.RoleListItem).
		Height(62).PaddingX(4).Gap(14).AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Cursor(mu.CursorPointer)
	if r.Hovered() {
		r.Background(p.Deboss.Alpha(0.35))
	}
	r.Children(func() {
		mono(c, a.clockOrDay(e.CreatedAt), 12, p.Graphite).Width(56).Shrink(0)
		a.appBadge(c, app, bundle, 28)
		serif(c, oneLine(e.FinalText), 15, p.Ink).SingleLine().Ellipsis("…").Grow(1).Basis(0).MinWidth(0)
		name, ink := a.entryStyle(e)
		styleTag(c, p, name, ink)
		mono(c, durationClock(e.DurationMs), 12, p.Graphite).Width(36).Shrink(0).TextAlign(mu.End)
	})
	if clicked(c, r) {
		a.hist.selected = e.ID
		a.SetPage(PageHistory)
	}
}

// oneLine joins the lines of a text for a one-line row.
func oneLine(s string) string { return strings.Join(strings.Fields(s), " ") }

// homeSide is the Default Style card and the checklist.
func (a *App) homeSide(c *mu.Context, p theme.Colors) {
	a.defaultStyleCard(c, p)
	items := a.checklist()
	all := true
	for _, it := range items {
		all = all && it.ok
	}
	head := i18n.T("home.checklist.needs_attention")
	if all {
		head = i18n.T("home.checklist.all_set")
	}
	mu.Column(c).PaddingX(4).Gap(12).Children(func() {
		caps(c, p, head)
		mu.Column(c).Gap(10).Children(func() {
			for _, it := range items {
				a.checkRow(c, p, it)
			}
		})
	})
}

// defaultStyleCard is the default style on its stack of cards.
func (a *App) defaultStyleCard(c *mu.Context, p theme.Colors) {
	st, _ := a.styles.Get(a.ActiveStyle())
	mu.Column(c).Padding(10, 0, 0, 0).Children(func() {
		under := theme.Over(p.Sheet, p.Deboss, 0.5)
		mu.Box(c).Absolute().Top(0).Left(14).Right(14).Height(40).Radius(12).Background(p.Deboss).Shadow(0, 1, 2, 0, p.ShadowA(0.10))
		mu.Box(c).Absolute().Top(5).Left(7).Right(7).Height(40).Radius(12).Background(under).Shadow(0, 1, 2, 0, p.ShadowA(0.10))
		mu.Column(c).ZIndex(1).Padding(18, 20).Gap(12).Radius(12).Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).
			Shadow(0, 1, 2, 0, p.ShadowA(0.14)).Shadow(0, 10, 26, 0, p.ShadowA(0.12)).Children(func() {
			mu.Row(c).Height(22).AlignItems(mu.Center).Children(func() {
				caps(c, p, i18n.T("home.style.default")).Grow(1)
				if st != nil && st.UsesAI() && a.cfg.AI.Enabled {
					if prov, ok := a.cfg.AI.StyleProvider(st.Provider); ok {
						if prov.IsCloud() {
							badge(c, p, i18n.T("hub.place.cloud"), toneAccent, "cloud")
						} else {
							badge(c, p, i18n.T("hub.place.local"), toneMuted, "")
						}
					}
				}
			})
			if st != nil {
				display(c, p, styleName(st), 28).LineHeight(30.0 / 28)
				uiText(c, a.styleDetail(st), 14, 400, p.Graphite).LineHeight(20.0 / 14)
			}
			mu.Row(c).Gap(6).Padding(4, 0, 0, 0).AlignItems(mu.Center).Children(func() {
				if a.button(c, p, "home.change_style", i18n.T("home.style.change"), bopt{size: sizeSmall}) {
					a.SetPage(PageStyles)
				}
				if h := a.cfg.Hotkeys.CycleStyle; h != nil {
					mu.Box(c).Label(i18n.T("home.style.cycle_hotkey")).Children(func() {
						keycapStack(mu.Box(c).Height(22).PaddingX(6).Radius(5).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
							mono(c, h.CompactLabel(), 11, p.Ink)
						})
					})
				}
			})
		})
	})
}

// styleDetail is a style's description and where it runs, as one paragraph.
func (a *App) styleDetail(st *enhance.Style) string {
	desc := styleDescription(st)
	var where string
	switch {
	case !st.UsesAI():
		where = i18n.T("home.style.detail_no_ai", "computer", computer())
	case !a.cfg.AI.Enabled:
		where = i18n.T("home.style.detail_needs_provider")
	default:
		prov, ok := a.cfg.AI.StyleProvider(st.Provider)
		switch {
		case !ok:
			where = i18n.T("home.style.detail_needs_provider")
		case prov.NeedsModel():
			where = i18n.T("home.style.detail_needs_model", "provider", prov.Name)
		case prov.IsCloud():
			where = i18n.T("home.style.detail_cloud", "provider", prov.Name, "model", prov.ModelName())
		default:
			where = i18n.T("home.style.detail_local", "computer", computer(), "provider", prov.Name, "model", prov.ModelName())
		}
	}
	if desc == "" {
		return where
	}
	return desc + " " + where
}

// micDetail is the microphone in use, or detail when it is not known.
func (a *App) micDetail(detail string) string {
	if detail == "" {
		return ""
	}
	if a.cfg.Audio.InputDevice != nil {
		for _, d := range a.audioDevices() {
			if d.ID == platform.NormalizeDeviceID(*a.cfg.Audio.InputDevice) {
				return d.Name
			}
		}
	}
	return detail
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
	return []checkItem{
		{"home.check.mic", i18n.T("home.checklist.microphone"), a.micDetail(allowed(perm.mic)), perm.mic, i18n.T("home.checklist.allow"), request("microphone")},
		{"home.check.ax", i18n.TOS("os.permission.accessibility", nil), allowed(perm.ax), perm.ax, i18n.T("home.checklist.allow"), request("accessibility")},
		{"home.check.model", i18n.T("home.checklist.model"), modelDetail, ms.IsOnDisk(), i18n.T("home.checklist.open_models"), func() { a.SetPage(PageModels) }},
		{"home.check.dictionary", i18n.T("home.checklist.dictionary"), i18n.T("home.checklist.dictionary_summary",
			"words", i18n.TP("home.checklist.words", a.home.words, nil), "rules", i18n.TP("home.checklist.rules", a.home.rules, nil)),
			a.home.words > 0, i18n.T("home.checklist.add_words"), func() { a.SetPage(PageDictionary) }},
	}
}

func (a *App) checkRow(c *mu.Context, p theme.Colors, it checkItem) {
	mu.Row(c).Gap(10).AlignItems(mu.Center).Label(it.title).Children(func() {
		checkDot(c, p, it.ok)
		uiText(c, it.title, 14, 400, p.Ink).LineHeight(18.0 / 14).Grow(1).Basis(0).SingleLine()
		if !it.ok && it.do != nil {
			if a.link(c, p, it.key, it.action, 13, 500, "") {
				it.do()
			}
			return
		}
		uiText(c, it.detail, 13, 400, p.Graphite).SingleLine().Ellipsis("…").MaxWidth(170).TextAlign(mu.End)
	})
}

// routeKind is where a style's text goes (Rust ext.rs StyleRoute).
type routeKind int

const (
	routeNoAI             routeKind = iota // Raw, or a style with no prompt
	routeNeedsProvider                     // AI is off, or the style's provider is not set up
	routeNeedsModel                        // the provider has no model chosen yet
	routeLocal                             // on this computer
	routeCloud                             // off this computer
	routeCleanupCannotRun                  // a cleanup model that cannot run the style cleans up with its defaults
)

// route is where a style runs: the provider's name (or the cleanup
// model's) and the model's, "" for none.
type route struct {
	kind            routeKind
	provider, model string
}

// line is the route as one line: "Cloud · Claude Code · sonnet".
func (r route) line() string {
	switch r.kind {
	case routeNeedsProvider:
		return i18n.T("styles.route.needs_provider")
	case routeNeedsModel:
		return i18n.T("styles.route.needs_model", "provider", r.provider)
	case routeLocal, routeCloud:
		key := "styles.route.local"
		if r.kind == routeCloud {
			key = "styles.route.cloud"
		}
		if r.model == "" {
			return i18n.T(key+"_no_model", "provider", r.provider)
		}
		return i18n.T(key, "provider", r.provider, "model", r.model)
	case routeCleanupCannotRun:
		return i18n.T("styles.route.cleanup_cannot_run", "model", r.model)
	}
	return i18n.T("styles.route.no_ai", "computer", computer())
}

// styleRoute is where st runs (Rust ext.rs style_route): on its own
// provider, or on the default one, with its own model or the provider's.
// A provider that is gone does not fall back to the default.
func (a *App) styleRoute(st *enhance.Style) route {
	if st.IsRaw() {
		return route{kind: routeNoAI}
	}
	if !a.cfg.AI.Enabled {
		return route{kind: routeNeedsProvider}
	}
	prov, ok := a.cfg.AI.StyleProvider(st.Provider)
	if !ok {
		return route{kind: routeNeedsProvider}
	}
	// A cleanup model has no prompt and no model to choose; it cleans up
	// on this computer, also a style that it cannot run.
	if prov.Kind == config.KindCleanupModel {
		info, known := enhance.FindCleanupModel(prov.ModelName())
		switch plan := st.RunPlanOn(prov); {
		case plan.Kind == enhance.PlanCleanup:
			return route{kind: routeLocal, provider: info.Name}
		case plan.Kind == enhance.PlanCleanupOnly:
			return route{kind: routeCleanupCannotRun, model: info.Name}
		case known:
			return route{kind: routeNoAI}
		}
	}
	if !st.UsesAI() {
		return route{kind: routeNoAI}
	}
	model := ""
	switch {
	case st.Model != nil && *st.Model != "":
		model = a.modelLabel(prov.ID, *st.Model)
	case prov.ModelName() != "":
		model = a.modelLabel(prov.ID, prov.ModelName())
	case prov.NeedsModel():
		return route{kind: routeNeedsModel, provider: prov.Name}
	}
	kind := routeLocal
	if prov.IsCloud() {
		kind = routeCloud
	}
	return route{kind: kind, provider: prov.Name, model: model}
}

// defaultProvider is the provider of a style that names none, by the rule
// dictation uses (config.AI.StyleProvider).
func (a *App) defaultProvider() (config.Provider, bool) {
	return a.cfg.AI.StyleProvider(nil)
}
