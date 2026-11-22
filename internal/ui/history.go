package ui

import (
	"fmt"
	"log"
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
	if h.selected == 0 && len(h.entries) > 0 {
		h.selected = h.entries[0].ID
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
		mu.Column(c).Fill().Padding(40, 48).Gap(14).Children(func() {
			pageHeader(c, p, i18n.T("history.off.title"), i18n.T("history.off.body"))
			if a.btn(c, p, "history.turn_on", i18n.T("history.off.turn_on"), btnPrimary) {
				a.cfg.History.Enabled = true
				a.saveConfig()
			}
		})
		return
	}
	w, _ := c.Size()
	listW := float32(392)
	if w < 1100 {
		listW = 300
	}
	mu.Row(c).Fill().AlignItems(mu.Stretch).Children(func() {
		mu.Column(c).Width(listW).BorderEndWidth(1).BorderColor(p.Rule).Children(func() {
			a.historyToolbar(c, p)
			mu.Scroll(c.Key("history.list")).Grow(1).Padding(0, 10, 16, 10).Role(mu.RoleList).
				Label(i18n.T("history.a11y.list")).Children(func() { a.historyList(c, p) })
		})
		mu.Scroll(c.Key("history.detail")).Grow(1).Basis(0).Padding(32, 36).Gap(20).Children(func() {
			if e, ok := a.selectedEntry(); ok {
				a.historyDetail(c, p, e)
			} else {
				a.historyEmpty(c, p)
			}
		})
	})
}

func (a *App) historyToolbar(c *mu.Context, p theme.Colors) {
	bar := c.TitleBar()
	mu.Column(c).Padding(max(bar.Bottom, 0)+24, 16, 12, 16).Gap(10).Children(func() {
		mu.Row(c).AlignItems(mu.Baseline).Gap(8).Children(func() {
			heading(c, p, i18n.T("history.title"), 28).Role(mu.RoleHeading)
			count := i18n.TP("history.count", a.hist.total, map[string]any{"formatted": formatCount(a.hist.total)})
			if len(a.hist.entries) != a.hist.total {
				count = i18n.T("history.count_filtered", "shown", len(a.hist.entries), "total", a.hist.total)
			}
			mu.Text(c, count).FontSize(12.5).TextColor(p.Graphite)
		})
		mu.SearchField(a.k(c, "history.search"), &a.hist.query).Label(i18n.T("history.search_placeholder")).
			Placeholder(i18n.T("history.search_placeholder"))
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
			if a.chip(c, p, "history.filter.app", appLabel, a.hist.app != "") {
				a.hist.appsOpen = !a.hist.appsOpen
			}
		})
		if a.hist.appsOpen {
			a.appPicker(c, p)
		}
	})
}

// chip is a filter chip; it reports a click.
func (a *App) chip(c *mu.Context, p theme.Colors, key, label string, on bool) bool {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Role(mu.RoleToggleButton).Checked(on).
		Padding(4, 11).Radius(99).Border(1, p.Rule).Cursor(mu.CursorPointer)
	fg := p.Graphite
	if on {
		fg = p.OnInk
		b.Background(p.InkFill).Border(1, p.InkFill)
	} else if b.Hovered() {
		b.Background(p.Deboss)
	}
	b.Children(func() { mu.Text(c, label).FontSize(12.5).FontWeight(500).TextColor(fg).SingleLine() })
	return clicked(c, b)
}

func (a *App) appPicker(c *mu.Context, p theme.Colors) {
	mu.Column(c).Padding(6).Radius(10).Background(p.SheetRaised).Border(1, p.Rule).Role(mu.RoleList).
		Label(i18n.T("history.a11y.app_list")).Children(func() {
		pick := func(key, label, bundle string) {
			b := mu.ButtonBase(a.k(c, key)).Label(label).Padding(6, 8).Radius(6).Cursor(mu.CursorPointer)
			if b.Hovered() || a.hist.app == bundle {
				b.Background(p.Deboss)
			}
			b.Children(func() { mu.Text(c, label).FontSize(13).TextColor(p.Ink) })
			if clicked(c, b) {
				a.hist.app, a.hist.appsOpen, a.hist.loaded = bundle, false, false
			}
		}
		pick("history.app.all", i18n.T("history.all_apps"), "")
		if len(a.hist.apps) == 0 {
			mu.Text(c, i18n.T("history.app_none")).FontSize(12.5).TextColor(p.Graphite).Padding(6, 8)
		}
		for i, ac := range a.hist.apps {
			pick(fmt.Sprintf("history.app.%d", i), i18n.T("history.a11y.app_option", "app", ac.Name, "count", ac.Count), ac.BundleID)
		}
	})
}

func (a *App) historyList(c *mu.Context, p theme.Colors) {
	if len(a.hist.entries) == 0 {
		title, body := i18n.T("history.empty.title"), i18n.T("history.empty.body", "hotkey", a.hotkeyLabel())
		if a.hist.query != "" || a.hist.filter != filterAll || a.hist.app != "" {
			title, body = i18n.T("history.no_match_title"), i18n.T("history.no_match_body")
		}
		mu.Column(c).Padding(24, 8).Gap(6).Children(func() {
			mu.Text(c, title).FontSize(14).FontWeight(600).TextColor(p.Ink)
			mu.Text(c, body).FontSize(12.5).TextColor(p.Graphite)
		})
		return
	}
	now := a.now()
	day := ""
	for _, e := range a.hist.entries {
		if d := dayLabel(e.CreatedAt, now); d != day {
			day = d
			mu.Text(c, strings.ToUpper(d)).FontSize(11).FontWeight(600).LetterSpacing(1).TextColor(p.Pencil).Padding(14, 8, 6, 8)
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
	tm := e.CreatedAt.In(a.now().Location()).Format("15:04")
	r := mu.ButtonBase(a.k(c, fmt.Sprintf("history.row.%d", e.ID))).Role(mu.RoleListItem).Selected(sel).
		Label(i18n.T("history.a11y.entry", "app", entryApp(e), "time", tm)).Padding(10, 10).Gap(4).Radius(8).Column().Cursor(mu.CursorPointer)
	if sel {
		r.Background(p.SheetRaised).Border(1, p.Rule)
	} else if r.Hovered() {
		r.Background(p.Deboss.Alpha(0.6))
	}
	r.Children(func() {
		mu.Row(c).Gap(6).AlignItems(mu.Center).Children(func() {
			mu.Text(c, entryApp(e)).FontSize(12).FontWeight(500).TextColor(p.Graphite).SingleLine().Grow(1)
			if e.Enhance.Result == store.EnhanceApplied {
				mu.Icon(c, icon("sparkle")).FontSize(12).TextColor(p.Accent)
			}
			if e.AudioFile != "" {
				mu.Icon(c, icon("audio")).FontSize(12).TextColor(p.Pencil)
			}
			mu.Text(c, tm).Font(theme.Mono).FontSize(11.5).TextColor(p.Pencil)
		})
		mu.Text(c, e.FinalText).Font(theme.Body).FontSize(13.5).TextColor(p.Ink).MaxLines(2).Ellipsis("…")
	})
	if clicked(c, r) && !sel {
		a.selectEntry(e.ID)
	}
}

func (a *App) selectEntry(id int64) {
	h := &a.hist
	h.selected, h.menuOpen, h.confirm, h.showRaw = id, false, false, false
	a.stopPlayer()
	a.invalidate()
}

func (a *App) historyEmpty(c *mu.Context, p theme.Colors) {
	mu.Column(c).Gap(8).Padding(40, 0).Children(func() {
		mu.Text(c, i18n.T("history.empty.title")).Font(theme.Display).FontSize(24).TextColor(p.Ink)
		mu.Text(c, i18n.T("history.empty.body", "hotkey", a.hotkeyLabel())).FontSize(14).TextColor(p.Graphite)
	})
}

func (a *App) hotkeyLabel() string {
	if h := a.cfg.Hotkeys.Toggle; h != nil {
		return h.CompactLabel()
	}
	return i18n.T("history.empty.your_hotkey")
}

func (a *App) historyDetail(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	when := e.CreatedAt.In(a.now().Location())
	mu.Row(c).AlignItems(mu.Center).Gap(10).Children(func() {
		mu.Column(c).Gap(2).Grow(1).Children(func() {
			mu.Text(c, entryApp(e)).FontSize(13).FontWeight(500).TextColor(p.Graphite)
			mu.Text(c, i18n.T("history.time_and_duration", "time", dayLabel(when, a.now())+" "+when.Format("15:04"),
				"duration", durationLabel(e.DurationMs))).FontSize(12.5).TextColor(p.Pencil)
		})
		if a.hist.busy != "" {
			mu.Text(c, a.hist.busy).FontSize(12.5).TextColor(p.Graphite)
		}
		label := i18n.T("history.a11y.more_actions")
		more := mu.ButtonBase(a.k(c, "history.more")).Label(label).Tooltip(label).Size(30, 30).Radius(8).
			Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer).HasPopup(mu.PopupMenu)
		if more.Hovered() || a.hist.menuOpen {
			more.Background(p.Deboss)
		}
		more.Children(func() { mu.Icon(c, icon("more")).FontSize(16).TextColor(p.Graphite) })
		if clicked(c, more) {
			a.hist.menuOpen, a.hist.confirm = !a.hist.menuOpen, false
		}
		mu.PopoverBase(c, more, &a.hist.menuOpen, func(panel mu.Element) { a.historyMenu(c, p, panel, e) })
	})
	if a.hist.confirm {
		a.deleteConfirm(c, p, e)
	}
	mu.Text(c, e.FinalText).Font(theme.Body).FontSize(18).LineHeight(1.5).TextColor(p.Ink).Selectable().
		Label(i18n.T("history.a11y.final_text"))
	a.audioPlayer(c, p, e)
	a.trail(c, p, e)
}

func durationLabel(ms uint64) string {
	s := int(ms / 1000)
	if s < 60 {
		return i18n.T("hub.duration.seconds", "seconds", s)
	}
	return i18n.T("hub.duration.minutes_seconds", "minutes", s/60, "seconds", s%60)
}

// historyMenu is the 3-dot menu, drawn under its button.
func (a *App) historyMenu(c *mu.Context, p theme.Colors, panel mu.Element, e store.HistoryEntry) {
	item := func(key, label string, danger bool, fn func()) {
		b := mu.ButtonBase(a.k(c, key)).Label(label).Role(mu.RoleMenuItem).Padding(7, 12).Radius(6).Cursor(mu.CursorPointer)
		if b.Hovered() {
			b.Background(p.Deboss)
		}
		fg := p.Ink
		if danger {
			fg = p.Danger
		}
		b.Children(func() { mu.Text(c, label).FontSize(13).TextColor(fg).SingleLine() })
		if clicked(c, b) {
			a.hist.menuOpen = false
			fn()
		}
	}
	enhanceLabel := i18n.T("history.menu.enhance")
	if e.Enhance.Result == store.EnhanceApplied {
		enhanceLabel = i18n.T("history.menu.re_enhance")
	}
	panel.Margin(6, 0, 0, 0).Width(220).Padding(5).Radius(10).Role(mu.RoleMenu).
		Background(p.SheetRaised).Border(1, p.Rule).Shadow(0, 8, 24, 0, p.Shadow.Alpha(0.18))
	mu.Column(c).Children(func() {
		item("history.copy", i18n.T("history.menu.copy"), false, func() { c.WriteClipboard(e.FinalText); a.hist.busy = i18n.T("history.status_copied") })
		item("history.copy_meta", i18n.T("history.menu.copy_metadata"), false, func() { c.WriteClipboard(e.MetadataJSON()) })
		item("history.enhance", enhanceLabel, false, func() { a.reEnhance(e) })
		if e.AudioFile != "" {
			item("history.transcribe", i18n.T("history.menu.transcribe_again"), false, func() { a.transcribeAgain(e) })
		}
		item("history.delete", i18n.T("history.delete"), true, func() { a.hist.confirm = true })
	})
}

func (a *App) deleteConfirm(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	mu.Row(c).Gap(10).Padding(10, 14).Radius(10).Background(p.DangerWash).AlignItems(mu.Center).Role(mu.RoleAlert).Children(func() {
		mu.Text(c, i18n.T("history.confirm_delete")).FontSize(13).TextColor(p.Ink).Grow(1)
		if a.btn(c, p, "history.delete_cancel", i18n.T("history.cancel"), btnGhost) {
			a.hist.confirm = false
		}
		if a.btn(c, p, "history.delete_confirm", i18n.T("history.delete"), btnDanger) {
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

// audioPlayer: the waveform, play/pause and the position slider.
func (a *App) audioPlayer(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	h := &a.hist
	if e.AudioFile == "" {
		note := i18n.T("history.audio.not_saved")
		if len(e.Waveform) > 0 {
			note = i18n.T("history.audio.expired")
		}
		mu.Text(c, note).FontSize(12.5).TextColor(p.Pencil)
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
	card(c, p).Gap(10).Children(func() {
		mu.Row(c).Gap(12).AlignItems(mu.Center).Children(func() {
			label, ic := i18n.T("history.a11y.play"), "play"
			if h.playing {
				label, ic = i18n.T("history.a11y.pause"), "pause"
			}
			if a.iconBtn(c, p, "history.play", ic, label) {
				a.togglePlay(e)
			}
			levels := make([]float32, len(e.Waveform))
			for i, b := range e.Waveform {
				levels[i] = float32(b) / 255 / 6
			}
			waveform(c, levels, p.Ink).Grow(1).Height(36)
			mu.Text(c, i18n.T("history.player.position", "pos", clock(h.position), "total", clock(total))).
				Font(theme.Mono).FontSize(12).TextColor(p.Graphite)
		})
		pos := h.position
		mu.StepSlider(a.k(c, "history.position"), &pos, 0, max(total, 0.1), 5).
			Label(i18n.T("history.a11y.position")).OnChange(func() { a.seek(e, pos) })
		kept := i18n.T("history.audio.kept")
		if d := a.cfg.History.KeepAudioDays; d != nil {
			kept = i18n.T("history.audio.kept_until", "date", i18n.DayMonthYear(e.CreatedAt.AddDate(0, 0, int(*d))))
		}
		mu.Text(c, kept).FontSize(12).TextColor(p.Pencil)
	})
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

// trail is "How it got here": transcript, dictionary, style, insertion.
func (a *App) trail(c *mu.Context, p theme.Colors, e store.HistoryEntry) {
	mu.Column(c).Gap(10).Children(func() {
		eyebrow(c, p, i18n.T("history.trail.title"))
		step := func(iconName, text, detail string) {
			mu.Row(c).Gap(10).AlignItems(mu.Start).Children(func() {
				mu.Icon(c, icon(iconName)).FontSize(15).TextColor(p.Graphite)
				mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
					mu.Text(c, text).FontSize(13.5).TextColor(p.Ink)
					if detail != "" {
						mu.Text(c, detail).FontSize(12.5).TextColor(p.Graphite)
					}
				})
			})
		}
		model := a.modelName(models.ModelID(e.Model))
		if e.TranscribeMs > 0 {
			step("mic", i18n.T("history.trail.transcript_ms", "model", model, "ms", e.TranscribeMs), "")
		} else {
			step("mic", i18n.T("history.trail.transcript", "model", model), "")
		}
		if e.Transcript != e.FinalText {
			if a.btn(c, p, "history.show_transcript", i18n.T("history.trail.show_transcript"), btnGhost) {
				a.hist.showRaw = !a.hist.showRaw
			}
			if a.hist.showRaw {
				mu.Text(c, e.Transcript).Font(theme.Body).FontSize(14).TextColor(p.Graphite).Padding(8, 12).Radius(8).Background(p.Deboss).Selectable()
			}
		}
		n := 0
		for _, r := range e.Replacements {
			n += r.Count
		}
		if n == 0 {
			step("dictionary", i18n.T("history.trail.dictionary_none"), "")
		} else {
			step("dictionary", i18n.TP("history.trail.dictionary", n, nil), "")
		}
		styleLabel := e.StyleID
		if st, ok := a.styles.Get(e.StyleID); ok {
			styleLabel = styleName(st)
		}
		switch e.Enhance.Result {
		case store.EnhanceApplied:
			step("sparkle", i18n.T("history.trail.style_applied", "style", styleLabel, "provider", e.Enhance.ProviderID,
				"model", e.Enhance.Model, "seconds", i18n.Decimal(float64(e.Enhance.ElapsedMs)/1000, 1)), "")
		case store.EnhanceFailed:
			step("warning", i18n.T("history.trail.style_failed", "style", styleLabel, "provider", e.Enhance.ProviderID),
				i18n.T("history.trail.style_failed_detail", "reason", e.Enhance.Reason))
		default:
			if e.StyleID == "" || e.StyleID == "raw" {
				step("sparkle", i18n.T("history.trail.raw_no_ai"), "")
			} else {
				step("sparkle", i18n.T("history.trail.style_unused", "style", styleLabel), i18n.T("history.trail.ai_not_run"))
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
			step("arrow-right", i18n.T(k, "app", app), "")
		case store.InsertTyped:
			step("keyboard", i18n.T("history.trail.typed", "app", app), "")
		case store.InsertNotInserted:
			step("close", i18n.T("history.trail.cancelled"), "")
		case store.InsertFailed:
			step("warning", i18n.T("history.trail.not_inserted", "reason", e.Insert.Reason), "")
		}
	})
}
