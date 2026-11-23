package ui

import (
	"fmt"
	"log"
	"runtime"
	"strings"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
)

// History filters.
const (
	filterAll = iota
	filterEnhanced
	filterAudio
)

type historyState struct {
	loaded    bool
	query     string
	lastQuery string
	filter    int
	app       string // bundle id, "" for all apps
	appsOpen  bool
	appQuery  string // the filter field of the app picker
	apps      []store.AppCount
	entries   []store.HistoryEntry
	total     int
	selected  int64
	menuOpen  bool
	confirm   bool // delete confirmation of the selected entry
	busy      string
	showRaw   bool

	player    Player
	playerFor int64
	playing   bool
	position  float64 // seconds
}

// Player plays one recording; the app uses mygo/audio.
type Player interface {
	Play()
	Pause()
	Seek(t time.Duration)
	Position() time.Duration
	Duration() time.Duration
	Playing() bool
	Close()
}

func (a *App) loadHistory() {
	h := &a.hist
	if h.loaded && h.query == h.lastQuery {
		return
	}
	h.loaded, h.lastQuery = true, h.query
	h.entries, h.total, h.apps = nil, 0, nil
	if a.d.Store == nil {
		return
	}
	q := store.HistoryQuery{Text: strings.TrimSpace(h.query), EnhancedOnly: h.filter == filterEnhanced, HasAudio: h.filter == filterAudio, AppBundleID: h.app, Limit: 500}
	var err error
	if h.entries, err = a.d.Store.List(q); err != nil {
		log.Printf("history: %v", err)
	}
	h.total, _ = a.d.Store.Count(store.HistoryQuery{})
	h.apps, _ = a.d.Store.AppList()
	// The selection follows the list: a filter that hides the selected entry
	// selects the first one that shows.
	if _, ok := a.selectedEntry(); !ok {
		var first int64
		if len(h.entries) > 0 {
			first = h.entries[0].ID
		}
		if first != h.selected {
			h.selected, h.menuOpen, h.confirm, h.showRaw = first, false, false, false
			a.stopPlayer()
		}
	}
}

func (a *App) selectedEntry() (store.HistoryEntry, bool) {
	for _, e := range a.hist.entries {
		if e.ID == a.hist.selected {
			return e, true
		}
	}
	return store.HistoryEntry{}, false
}

func (a *App) setHistoryFilter(f int) {
	a.hist.filter, a.hist.loaded = f, false
	a.invalidate()
}

func (a *App) historyPage(c *mu.Context, p theme.Colors) {
	a.loadHistory()
	if !a.cfg.History.Enabled && len(a.hist.entries) == 0 {
		mu.Column(c).Fill().Padding(44, 52).Gap(20).Children(func() {
			pageHeader(c, p, i18n.T("history.off.title"), i18n.T("history.off.body"))
			mu.Row(c).Children(func() {
				if a.btn(c, p, "history.turn_on", i18n.T("history.off.turn_on"), btnPrimary) {
					a.cfg.History.Enabled = true
					a.saveConfig()
				}
			})
		})
		return
	}
	w, _ := c.Size()
	listW := float32(392)
	if w < 1100 {
		listW = 300
	}
	mu.Row(c).Fill().AlignItems(mu.Stretch).Children(func() {
		mu.Column(c).Width(listW).Shrink(0).BorderEndWidth(1).BorderColor(p.Rule).Children(func() {
			a.historyToolbar(c, p)
			mu.Scroll(c.Key("history.list")).Grow(1).Padding(0, 20, 20, 28).Gap(4).Role(mu.RoleList).
				Label(i18n.T("history.a11y.list")).Children(func() { a.historyList(c, p) })
		})
		mu.Scroll(c.Key("history.detail")).Grow(1).Basis(0).Children(func() {
			// With no entry the list says why; the detail stays blank.
			if e, ok := a.selectedEntry(); ok {
				a.historyDetail(c, p, e)
			}
		})
	})
}

func (a *App) historyToolbar(c *mu.Context, p theme.Colors) {
	mu.Column(c).Padding(40, 20, 18, 28).Gap(18).Children(func() {
		mu.Row(c).PaddingX(4).AlignItems(mu.End).Gap(8).Children(func() {
			heading(c, p, i18n.T("history.title"), 32).LineHeight(40.0 / 32).Role(mu.RoleHeading).Grow(1)
			count := i18n.TP("history.count", a.hist.total, map[string]any{"formatted": formatCount(a.hist.total)})
			uiText(c, count, 13, 400, p.Graphite).Padding(0, 0, 6, 0).SingleLine()
		})
		mini := "⌘F"
		if runtime.GOOS != "darwin" {
			mini = "Ctrl F"
		}
		a.searchWell(c, p, "history.search", i18n.T("history.search_placeholder"), &a.hist.query, 38, 0, mini)
		mu.Row(c).Gap(6).Wrap().Role(mu.RoleToolbar).Label(i18n.T("history.a11y.filters")).Children(func() {
			chips := []struct {
				key, label string
				f          int
			}{
				{"history.filter.all", i18n.T("history.filter_all"), filterAll},
				{"history.filter.enhanced", i18n.T("history.filter_enhanced"), filterEnhanced},
				{"history.filter.audio", i18n.T("history.filter_audio"), filterAudio},
			}
			for _, ch := range chips {
				if a.chip(c, p, ch.key, ch.label, a.hist.filter == ch.f) {
					a.setHistoryFilter(ch.f)
				}
			}
			appLabel := i18n.T("history.filter_app")
			for _, ac := range a.hist.apps {
				if ac.BundleID == a.hist.app && a.hist.app != "" {
					appLabel = ac.Name
				}
			}
			chip := a.chipBase(c, p, "history.filter.app", appLabel, a.hist.app != "", "chevron-down", mu.RoleButton).
				HasPopup(mu.PopupListBox).Expanded(a.hist.appsOpen)
			if clicked(c, chip) {
				a.hist.appsOpen = !a.hist.appsOpen
				a.hist.appQuery = ""
			}
			// Floating under the chip (Rust history.rs app_picker), so
			// the list below stays where it is.
			mu.PopoverBase(c, chip, &a.hist.appsOpen, func(panel mu.Element) { a.appPicker(c, p, panel) })
		})
	})
}

// appRows are the lines of the app picker: "All apps" (nil), then the
// apps. With a query, only the apps whose name has it (Rust app_rows).
func appRows(apps []store.AppCount, query string) []*store.AppCount {
	query = strings.ToLower(strings.TrimSpace(query))
	var rows []*store.AppCount
	if query == "" {
		rows = append(rows, nil)
	}
	for i := range apps {
		if query == "" || strings.Contains(strings.ToLower(apps[i].Name), query) {
			rows = append(rows, &apps[i])
		}
	}
	return rows
}

// appPicker is the sheet of the App filter: a filter field, then "All
// apps" and each app with its icon and its number of dictations, in a
// list that scrolls.
func (a *App) appPicker(c *mu.Context, p theme.Colors, panel mu.Element) {
	mu.Column(c).Children(func() {
		mu.Column(c).Padding(6).Children(func() {
			a.searchWell(c, p, "history.app_filter", i18n.T("history.app_filter_placeholder"), &a.hist.appQuery, 32, 0, "").AutoFocus()
		})
		list := a.floatingList(c, p, panel.Margin(6, 0, 0, 0).Width(272).Radius(12), "history.apps", 298)
		list.Padding(0, 5, 5, 5).Gap(1).Role(mu.RoleListBox).Label(i18n.T("history.a11y.app_list")).Children(func() {
			rows := appRows(a.hist.apps, a.hist.appQuery)
			if len(rows) == 0 {
				uiText(c, i18n.T("history.app_none"), 13, 400, p.Graphite).Padding(8, 9)
			}
			for _, r := range rows {
				a.appRow(c, p, r)
			}
		})
	})
}

// appRow is one line of the app picker; r is nil for "All apps".
func (a *App) appRow(c *mu.Context, p theme.Colors, r *store.AppCount) {
	key, name, bundle, count := "history.app.all", i18n.T("history.all_apps"), "", a.hist.total
	if r != nil {
		key, name, bundle, count = "history.app."+r.BundleID, r.Name, r.BundleID, r.Count
	}
	sel := a.hist.app == bundle
	b := menuLine(mu.ButtonBase(a.k(c, key)).Label(i18n.T("history.a11y.app_option", "app", name, "count", formatCount(count))).
		Role(mu.RoleOption).Checked(sel).Height(32).PaddingX(9).Gap(9).Radius(8))
	if b.Hovered() || sel {
		b.Background(p.Deboss)
	}
	b.Children(func() {
		if r != nil {
			a.appBadge(c, name, bundle, 18)
		} else {
			mu.Box(c).Size(18, 18).Shrink(0).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
				mu.Icon(c, icon("history")).FontSize(14).TextColor(p.Graphite)
			})
		}
		weight := 400
		if sel {
			weight = 600
		}
		uiText(c, name, 13, weight, p.Ink).SingleLine().Ellipsis("…").Grow(1).Basis(0).MinWidth(0)
		if sel {
			mu.Icon(c, icon("check")).FontSize(12).TextColor(p.Ink)
		}
		mono(c, formatCount(count), 12, p.Graphite).Shrink(0)
	})
	if clicked(c, b) {
		a.hist.app, a.hist.appsOpen, a.hist.loaded = bundle, false, false
	}
}

func (a *App) historyList(c *mu.Context, p theme.Colors) {
	if len(a.hist.entries) == 0 {
		title, body := i18n.T("history.empty.title"), i18n.T("history.empty.body", "hotkey", a.hotkeyLabel())
		if a.hist.query != "" || a.hist.filter != filterAll || a.hist.app != "" {
			title, body = i18n.T("history.no_match_title"), i18n.T("history.no_match_body")
		}
		mu.Column(c).Padding(24, 10).Gap(6).Children(func() {
			uiText(c, title, 14, 600, p.Ink)
			uiText(c, body, 13, 400, p.Graphite)
		})
		return
	}
	now := a.now()
	day := ""
	for _, e := range a.hist.entries {
		if d := dayLabel(e.CreatedAt, now); d != day {
			top := float32(14)
			if day == "" {
				top = 6
			}
			day = d
			caps(c, p, d).Padding(top, 10, 6, 10)
		}
		a.historyRow(c, p, e)
	}
}

// dayLabel is "Today", "Yesterday", or the date.
func dayLabel(t, now time.Time) string {
	t = t.In(now.Location())
	y1, m1, d1 := t.Date()
	y2, m2, d2 := now.Date()
	if y1 == y2 && m1 == m2 && d1 == d2 {
		return i18n.Today()
	}
	if y, m, d := now.AddDate(0, 0, -1).Date(); y1 == y && m1 == m && d1 == d {
		return i18n.Yesterday()
	}
	return i18n.WeekdayDayMonth(t)
}

func entryApp(e store.HistoryEntry) string {
	if e.App != nil && e.App.Name != "" {
		return e.App.Name
	}
	return i18n.T("history.unknown_app")
}

func (a *App) historyRow(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	sel := e.ID == a.hist.selected
	tm := a.clockOf(e.CreatedAt)
	bundle := ""
	if e.App != nil {
		bundle = e.App.BundleID
	}
	r := mu.ButtonBase(a.k(c, fmt.Sprintf("history.row.%d", e.ID))).Role(mu.RoleListItem).Selected(sel).
		Label(i18n.T("history.a11y.entry", "app", entryApp(e), "time", tm)).Padding(12).Gap(12).Radius(12).
		AlignItems(mu.Start).Justify(mu.Start).Cursor(mu.CursorPointer)
	if sel {
		r.Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).Shadow(0, 1, 2, 0, p.ShadowA(0.14)).Shadow(0, 6, 16, 0, p.ShadowA(0.10))
	} else if r.Hovered() {
		r.Background(p.Deboss.Alpha(0.4))
	}
	r.Children(func() {
		a.appBadge(c, entryApp(e), bundle, 28)
		mu.Column(c).Gap(4).Grow(1).Basis(0).MinWidth(0).AlignItems(mu.Stretch).Children(func() {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
				uiText(c, entryApp(e), 13, 600, p.Ink).SingleLine().Grow(1)
				mono(c, tm, 12, p.Graphite)
			})
			fg := p.Graphite
			if sel {
				fg = p.Ink
			}
			serif(c, e.FinalText, 14, fg).LineHeight(20.0 / 14).MaxLines(3).Ellipsis("…")
		})
	})
	if clicked(c, r) && !sel {
		a.selectEntry(e.ID)
	}
	// A right-click opens the actions of this entry, whichever is
	// selected; each row keeps whether its menu shows.
	open := mu.Local(r, rowMenuKey{}, func() bool { return false })
	m := mu.ContextMenuBase(c, r, open)
	m.Popup(func(panel mu.Element) {
		list := a.floatingList(c, p, panel.MinWidth(184).Padding(5).Radius(12).Label(entryApp(e)), "history.row_menu", 312)
		list.Gap(1).Children(func() {
			a.historyActionItems(c, p, a.historyActions(c, e), func(act historyAction) mu.Element {
				return m.Item().Label(act.label)
			}, func() {})
		})
	})
}

type rowMenuKey struct{}

func (a *App) selectEntry(id int64) {
	h := &a.hist
	h.selected, h.menuOpen, h.confirm, h.showRaw = id, false, false, false
	a.stopPlayer()
	a.invalidate()
}

func (a *App) hotkeyLabel() string {
	if h := a.cfg.Hotkeys.Toggle; h != nil {
		return h.CompactLabel()
	}
	return i18n.T("history.empty.your_hotkey")
}

func (a *App) historyDetail(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	bundle := ""
	if e.App != nil {
		bundle = e.App.BundleID
	}
	mu.Row(c).Padding(40, 48, 20, 48).AlignItems(mu.Center).Gap(10).Children(func() {
		a.appBadge(c, entryApp(e), bundle, 24)
		uiText(c, entryApp(e), 14, 600, p.Ink).SingleLine()
		uiText(c, a.clockOrDay(e.CreatedAt)+" · "+durationLabel(e.DurationMs), 14, 400, p.Graphite).SingleLine().Grow(1)
		if a.hist.busy != "" {
			uiText(c, a.hist.busy, 13, 400, p.Graphite)
		}
		label := i18n.T("history.a11y.more_actions")
		more := mu.ButtonBase(a.k(c, "history.more")).Label(label).Tooltip(label).Size(30, 30).Radius(8).
			Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer).HasPopup(mu.PopupMenu)
		if more.Hovered() || a.hist.menuOpen {
			more.Background(p.Deboss.Alpha(0.55))
		}
		more.Children(func() { mu.Icon(c, icon("more")).FontSize(18).TextColor(p.Graphite) })
		if clicked(c, more) {
			a.hist.menuOpen, a.hist.confirm = !a.hist.menuOpen, false
		}
		mu.PopoverBase(c, more, &a.hist.menuOpen, func(panel mu.Element) { a.historyMenu(c, p, panel, e) })
	})
	mu.Column(c).Padding(8, 48, 0, 48).Gap(28).Children(func() {
		if a.hist.confirm {
			a.deleteConfirm(c, p, e)
		}
		long := len(e.FinalText) > 140
		text := mu.Scroll(c.Key(fmt.Sprintf("history.text.%d", e.ID))).MinHeight(66).MaxHeight(154)
		if long {
			text.DrawOver(func(pt *mu.Painter, r mu.Rect) {
				f := mu.Rect{X: r.X, Y: r.Y + r.H - 28, W: r.W, H: 28}
				pt.FillGradient(f, mu.LinearGradient{From: p.Sheet.Alpha(0), To: p.Sheet, Angle: 180}, 0)
			})
		}
		text.Children(func() {
			mu.Text(c, e.FinalText).Font(serifFont(theme.Display)).FontSize(33).FontWeight(500).LineHeight(44.0 / 33).TextColor(p.Ink).
				Selectable().Label(i18n.T("history.a11y.final_text"))
		})
		a.audioPlayer(c, p, e)
	})
	mu.Column(c).Margin(28, 48, 0, 48).Padding(20, 4, 32, 4).BorderWidth(1, 0, 0, 0).BorderColor(p.Rule).Children(func() {
		a.trail(c, p, e)
	})
}

func durationLabel(ms uint64) string {
	s := int((ms + 500) / 1000)
	if s < 60 {
		return i18n.T("hub.duration.seconds", "seconds", max(s, 1))
	}
	return i18n.T("hub.duration.minutes_seconds", "minutes", s/60, "seconds", s%60)
}

// historyAction is one line of the actions on an entry.
type historyAction struct {
	key, icon, label string
	danger           bool // in red, below a rule
	run              func()
}

// historyActions are the actions on e, for its ⋯ menu and its row's
// context menu. Delete selects e and asks first.
func (a *App) historyActions(c *mu.Context, e store.HistoryEntry) []historyAction {
	enhanceLabel := i18n.T("history.menu.enhance")
	if e.Enhance.Result == store.EnhanceApplied {
		enhanceLabel = i18n.T("history.menu.re_enhance")
	}
	acts := []historyAction{
		{"history.copy", "copy", i18n.T("history.menu.copy"), false, func() {
			c.WriteClipboard(e.FinalText)
			a.hist.busy = i18n.T("history.status_copied")
		}},
		{"history.copy_meta", "code", i18n.T("history.menu.copy_metadata"), false, func() { c.WriteClipboard(e.MetadataJSON()) }},
		{"history.enhance", "sparkle", enhanceLabel, false, func() { a.reEnhance(e) }},
	}
	if e.AudioFile != "" {
		acts = append(acts, historyAction{"history.transcribe", "undo", i18n.T("history.menu.transcribe_again"), false, func() { a.transcribeAgain(e) }})
	}
	return append(acts, historyAction{"history.delete", "trash", i18n.T("history.delete"), true, func() {
		if a.hist.selected != e.ID {
			a.selectEntry(e.ID)
		}
		a.hist.confirm = true
	}})
}

// historyActionItems draws acts as the lines of a menu (Rust kit.rs
// action_menu): an icon and a label from the leading edge. item makes the
// line of an action, a button of the ⋯ menu or an item of a context menu;
// done runs before the chosen action.
func (a *App) historyActionItems(c *mu.Context, p theme.Colors, acts []historyAction, item func(historyAction) mu.Element, done func()) {
	for i, act := range acts {
		if act.danger && i > 0 {
			mu.Box(c).Height(1).Shrink(0).Margin(4, 6).Background(p.Rule)
		}
		b := menuLine(item(act).Height(32).Padding(0, 14, 0, 10).Gap(10).Radius(8))
		fg, iconFg, hover := p.Ink, p.Graphite, p.Deboss
		if act.danger {
			fg, iconFg, hover = p.Danger, p.Danger, p.DangerWash
		}
		if b.Hovered() || b.Highlighted() {
			b.Background(hover)
		}
		b.Children(func() {
			mu.Icon(c, icon(act.icon)).FontSize(14).TextColor(iconFg)
			uiText(c, act.label, 13, 500, fg).SingleLine()
		})
		if clicked(c, b) {
			done()
			act.run()
		}
	}
}

// historyMenu is the 3-dot menu, drawn under its button.
func (a *App) historyMenu(c *mu.Context, p theme.Colors, panel mu.Element, e store.HistoryEntry) {
	list := a.floatingList(c, p, panel.Margin(6, 0, 0, 0).MinWidth(184).Padding(5).Radius(12).Role(mu.RoleMenu), "history.menu", 312)
	list.Gap(1).Children(func() {
		a.historyActionItems(c, p, a.historyActions(c, e), func(act historyAction) mu.Element {
			return mu.ButtonBase(a.k(c, act.key)).Label(act.label).Role(mu.RoleMenuItem)
		}, func() { a.hist.menuOpen = false })
	})
}

func (a *App) deleteConfirm(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	mu.Row(c).Gap(10).Padding(10, 14).Radius(12).Background(p.DangerWash).AlignItems(mu.Center).Role(mu.RoleAlert).Children(func() {
		uiText(c, i18n.T("history.confirm_delete"), 13, 500, p.Ink).Grow(1)
		if a.button(c, p, "history.delete_cancel", i18n.T("history.cancel"), bopt{kind: btnGhost, size: sizeSmall}) {
			a.hist.confirm = false
		}
		if a.button(c, p, "history.delete_confirm", i18n.T("history.delete"), bopt{kind: btnDanger, size: sizeSmall}) {
			a.deleteEntry(e.ID)
		}
	})
}

func (a *App) deleteEntry(id int64) {
	if a.d.Store != nil {
		if _, err := a.d.Store.Delete(id); err != nil {
			a.flash = err.Error()
		}
	}
	a.stopPlayer()
	a.hist.confirm, a.hist.selected = false, 0
	a.dataChanged()
	a.invalidate()
}

// transcribeAgain runs the final pass on the saved audio again.
func (a *App) transcribeAgain(e store.HistoryEntry) {
	if a.d.Store == nil || a.d.Engine == nil || e.AudioFile == "" {
		return
	}
	a.hist.busy = i18n.T("history.status_transcribing")
	st, eng, model, lang := a.d.Store, a.d.Engine, a.FinalModel(), a.cfg.Dictation.Language
	go func() {
		samples, err := st.LoadAudio(e.AudioFile)
		var tr dictation.Transcript
		if err == nil {
			tr, err = eng.Transcribe(samples, dictation.SessionOptions{FinalModel: model, Language: lang})
		}
		a.post(func() {
			a.hist.busy = ""
			if err != nil {
				a.flash = err.Error()
			} else {
				e.Transcript, e.FinalText, e.Model, e.TranscribeMs = tr.Text, tr.Text, string(tr.Model), tr.ElapsedMs
				if err := st.UpdateEntry(e); err != nil {
					a.flash = err.Error()
				}
			}
			a.dataChanged()
			a.invalidate()
		})
	}()
}

// reEnhance runs the default style on the transcript again.
func (a *App) reEnhance(e store.HistoryEntry) {
	if a.d.Store == nil || a.d.Styler == nil {
		return
	}
	st, ok := a.styles.Get(a.ActiveStyle())
	if !ok {
		return
	}
	prov, _ := a.defaultProvider()
	a.hist.busy = i18n.T("history.status_enhancing")
	job := dictation.StyleJob{Style: st, Provider: prov, Text: e.Transcript, Language: a.cfg.Dictation.Language,
		SpeechModel: models.ModelID(e.Model), BasePrompt: a.styles.BasePrompt()}
	if e.App != nil {
		job.App, job.Site = e.App.BundleID, e.App.Site
	}
	styler, db := a.d.Styler, a.d.Store
	go func() {
		out := styler.Run(job)
		a.post(func() {
			a.hist.busy = ""
			if out.EnhanceError != nil {
				a.flash = out.EnhanceError.Error()
			} else {
				e.FinalText, e.Enhance, e.StyleID = out.Text, out.Enhance, st.ID
				if err := db.UpdateEntry(e); err != nil {
					a.flash = err.Error()
				}
			}
			a.dataChanged()
			a.invalidate()
		})
	}()
}

// audioPlayer is the debossed tray: the ink play button, the waveform
// (which seeks), and the time.
func (a *App) audioPlayer(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	h := &a.hist
	if e.AudioFile == "" {
		note := i18n.T("history.audio.not_saved")
		if len(e.Waveform) > 0 {
			note = i18n.T("history.audio.expired")
		}
		uiText(c, note, 13, 400, p.Graphite)
		return
	}
	total := float64(e.DurationMs) / 1000
	if h.player != nil && h.playerFor == e.ID {
		h.position = h.player.Position().Seconds()
		h.playing = h.player.Playing()
		if h.playing {
			c.Invalidate()
		}
	}
	mu.Column(c).Gap(10).Children(func() {
		tray := mu.Row(c).Padding(14, 18, 14, 14).Gap(16).Radius(16).AlignItems(mu.Center).Background(p.Deboss).
			InsetShadow(0, 2, 4, 0, p.ShadowA(0.18)).InsetShadow(0, -1, 0, 0, p.Highlight(0.75))
		tray.Children(func() {
			label, ic := i18n.T("history.a11y.play"), "play"
			if h.playing {
				label, ic = i18n.T("history.a11y.pause"), "pause"
			}
			b := mu.ButtonBase(a.k(c, "history.play")).Label(label).Size(44, 44).Shrink(0).Radius(99).Background(p.InkFill).
				InsetShadow(0, 1, 0, 0, white.Alpha(0.14)).InsetShadow(0, -2, 4, 0, black.Alpha(0.45)).Shadow(0, 2, 4, 0, p.ShadowA(0.28)).
				Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer).Disabled(a.d.NewPlayer == nil)
			if a.d.NewPlayer == nil {
				b.Opacity(0.35)
			}
			b.Children(func() { mu.Icon(c, icon(ic)).FontSize(16).TextColor(p.OnInk) })
			if clicked(c, b) {
				a.togglePlay(e)
			}
			pos := h.position
			played := float32(0)
			if total > 0 {
				played = float32(pos / total)
			}
			wave := e.Waveform
			mu.SliderBase(a.k(c, "history.position"), &pos, 0, max(total, 0.1)).Label(i18n.T("history.a11y.position")).
				Grow(1).Basis(0).Height(44).Cursor(mu.CursorPointer).OnChange(func() { a.seek(e, pos) }).
				Draw(func(pt *mu.Painter, r mu.Rect) { paintWave(pt, r, wave, played, p.Ink, p.Ink.Alpha(0.35)) })
			mono(c, clock(h.position)+" / "+clock(total), 12, p.Graphite).Shrink(0)
		})
		kept := i18n.T("history.audio.kept")
		if d := a.cfg.History.KeepAudioDays; d != nil {
			kept = i18n.T("history.audio.kept_until", "date", i18n.DayMonth(e.CreatedAt.AddDate(0, 0, int(*d))))
		}
		uiText(c, kept, 13, 400, p.Graphite).PaddingX(4)
	})
}

// paintWave draws the waveform as mirrored bars, the played part in ink.
func paintWave(pt *mu.Painter, r mu.Rect, peaks []byte, played float32, on, off mu.Color) {
	if len(peaks) == 0 {
		pt.Fill(mu.Rect{X: r.X, Y: r.Y + r.H/2 - 0.5, W: r.W, H: 1}, off, 0)
		return
	}
	const bar, gap = 2, 2
	n := int((r.W + gap) / (bar + gap))
	for i := range n {
		v := float32(peaks[i*len(peaks)/max(n, 1)]) / 255
		h := max(2, r.H*v)
		x := r.X + float32(i)*(bar+gap)
		col := off
		if (x-r.X)/r.W < played {
			col = on
		}
		pt.Fill(mu.Rect{X: x, Y: r.Y + (r.H-h)/2, W: bar, H: h}, col, 1)
	}
}

func clock(s float64) string {
	n := int(s)
	return fmt.Sprintf("%d:%02d", n/60, n%60)
}

func (a *App) togglePlay(e store.HistoryEntry) {
	h := &a.hist
	if h.player == nil || h.playerFor != e.ID {
		a.stopPlayer()
		if a.d.Store == nil || a.d.NewPlayer == nil {
			return
		}
		samples, err := a.d.Store.LoadAudio(e.AudioFile)
		if err != nil {
			a.flash = i18n.T("history.notice.audio_missing")
			return
		}
		h.player, h.playerFor = a.d.NewPlayer(samples), e.ID
	}
	if h.player.Playing() {
		h.player.Pause()
		h.playing = false
	} else {
		h.player.Play()
		h.playing = true
	}
	a.invalidate()
}

func (a *App) seek(e store.HistoryEntry, s float64) {
	a.hist.position = s
	if a.hist.player != nil && a.hist.playerFor == e.ID {
		a.hist.player.Seek(time.Duration(s * float64(time.Second)))
	}
}

func (a *App) stopPlayer() {
	if a.hist.player != nil {
		a.hist.player.Close()
	}
	a.hist.player, a.hist.playerFor, a.hist.playing, a.hist.position = nil, 0, false, 0
}

// trailStep is one step of "How it got here".
type trailStep struct {
	title string
	body  func()
	last  bool
}

// trail is "How it got here": rings on a ruled rail, the last a filled dot.
func (a *App) trail(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	var steps []trailStep
	add := func(title string, body func()) { steps = append(steps, trailStep{title: title, body: body}) }
	model := a.modelName(models.ModelID(e.Model))
	title := i18n.T("history.trail.transcript", "model", model)
	if e.TranscribeMs > 0 {
		title = i18n.T("history.trail.transcript_ms", "model", model, "ms", e.TranscribeMs)
	}
	add(title, func() {
		t := mu.Text(c, e.Transcript).Font(serifFont(theme.Display)).Italic().FontSize(15).LineHeight(21.0 / 15).TextColor(p.Graphite).Selectable()
		if !a.hist.showRaw {
			t.MaxLines(2).Ellipsis("…")
		}
		if len(e.Transcript) > 160 {
			// In a row, the link keeps the width of its label and starts at
			// the transcript's edge; the step's column would stretch it and
			// center the label.
			mu.Row(c).Children(func() {
				label := i18n.T("history.trail.show_transcript")
				if a.link(c, p, "history.show_transcript", label, 13, 500, "") {
					a.hist.showRaw = !a.hist.showRaw
				}
			})
		}
	})
	n := 0
	for _, r := range e.Replacements {
		n += r.Count
	}
	if n == 0 {
		add(i18n.T("history.trail.dictionary_none"), nil)
	} else {
		add(i18n.TP("history.trail.dictionary", n, nil), func() {
			for _, r := range e.Replacements {
				uiText(c, "“"+r.From+"” → “"+r.To+"”", 14, 400, p.Graphite).LineHeight(18.0 / 14)
			}
		})
	}
	styleLabel := e.StyleID
	var styleDesc string
	if st, ok := a.styles.Get(e.StyleID); ok {
		styleLabel, styleDesc = styleName(st), styleDescription(st)
	}
	desc := func(s string) func() {
		if s == "" {
			return nil
		}
		return func() { uiText(c, s, 14, 400, p.Graphite).LineHeight(18.0 / 14) }
	}
	switch e.Enhance.Result {
	case store.EnhanceApplied:
		add(i18n.T("history.trail.style_applied", "style", styleLabel, "provider", e.Enhance.ProviderID,
			"model", e.Enhance.Model, "seconds", i18n.Decimal(float64(e.Enhance.ElapsedMs)/1000, 2)), desc(styleDesc))
	case store.EnhanceFailed:
		add(i18n.T("history.trail.style_failed", "style", styleLabel, "provider", e.Enhance.ProviderID),
			desc(i18n.T("history.trail.style_failed_detail", "reason", e.Enhance.Reason)))
	default:
		if e.StyleID == "" || e.StyleID == "raw" {
			add(i18n.T("history.trail.raw_no_ai"), nil)
		} else {
			add(i18n.T("history.trail.style_unused", "style", styleLabel), desc(i18n.T("history.trail.ai_not_run")))
		}
	}
	app := i18n.T("history.trail.the_app")
	if e.App != nil && e.App.Name != "" {
		app = e.App.Name
		if e.App.Site != "" {
			app = i18n.T("history.trail.site_in_app", "site", e.App.Site, "app", e.App.Name)
		}
	}
	switch e.Insert.Result {
	case store.InsertPasted:
		k := "history.trail.pasted_kept"
		if e.Insert.ClipboardRestored {
			k = "history.trail.pasted_restored"
		}
		add(i18n.T(k, "app", app), nil)
	case store.InsertTyped:
		add(i18n.T("history.trail.typed", "app", app), nil)
	case store.InsertNotInserted:
		add(i18n.T("history.trail.cancelled"), nil)
	case store.InsertFailed:
		add(i18n.T("history.trail.not_inserted", "reason", e.Insert.Reason), nil)
	}
	if len(steps) > 0 {
		steps[len(steps)-1].last = true
	}
	caps(c, p, i18n.T("history.trail.title")).Padding(0, 0, 14, 0)
	for _, s := range steps {
		mu.Row(c).Gap(16).AlignItems(mu.Stretch).Children(func() {
			last := s.last
			mu.Box(c).Width(12).Shrink(0).Draw(func(pt *mu.Painter, r mu.Rect) {
				cx, top := r.X+r.W/2, r.Y+3
				if last {
					pt.Fill(mu.Rect{X: cx - 8, Y: top - 3, W: 16, H: 16}, p.Accent.Alpha(0.18), 8)
					pt.Fill(mu.Rect{X: cx - 5, Y: top, W: 10, H: 10}, p.Accent, 5)
					return
				}
				pt.Stroke(mu.Rect{X: cx - 5, Y: top, W: 10, H: 10}, p.Ink, 5, 1)
				pt.Fill(mu.Rect{X: cx - 0.75, Y: top + 14, W: 1.5, H: r.Y + r.H - top - 16}, p.Rule, 0)
			})
			mu.Column(c).Gap(3).Grow(1).Basis(0).Padding(0, 0, 16, 0).Children(func() {
				uiText(c, s.title, 13, 600, p.Ink).LineHeight(16.0 / 13)
				if s.body != nil {
					s.body()
				}
			})
		})
	}
}
