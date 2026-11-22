package ui

import (
	"math"
	"strings"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/theme"
)

// The model picker (crates/sayso-app/src/model_picker.rs, Paper page "Model
// picker · Thinking"): a trigger that shows the chosen model, and a sheet
// under it with the provider's models, a filter, Refresh, and for a
// provider the thinking level of the chosen model as a pen line.
//
// The Rust app opens the sheet in its own borderless window. MyGo's
// PopoverBase draws it in the window instead: it opens under the trigger,
// or above it where there is no room below, and Escape or a press outside
// closes it. A pick closes it too; the thinking level leaves it open.
//
// The model lists live in App.pick.lists, keyed by provider id (or a draft
// key), so every view of the same provider shares one fetch and an open
// sheet updates when a fetch ends. A fetch runs off the UI thread.

// ModelLister lists the models of a provider. enhance.ListProviderModels
// is the real one; tests give a fake.
type ModelLister func(p config.Provider, secrets providers.SecretStore, lm enhance.LanguageModel, timeout time.Duration) ([]enhance.ModelChoice, error)

const (
	// modelListTimeout bounds one fetch (model.rs load_models).
	modelListTimeout = 10 * time.Second
	pickerSheetW     = 340
	// pickerRowsH is the tallest the list of models gets.
	pickerRowsH = 280
	// thinkingPad is the room at the left and the right of the pen line.
	thinkingPad = 18
)

type listPhase int

const (
	listLoading listPhase = iota
	listReady
	listFailed
)

// modelList is a provider's models, as last fetched.
type modelList struct {
	phase  listPhase
	models []enhance.ModelChoice
	err    string
	// gen drops the answer of a fetch that a newer one replaced.
	gen uint64
}

type pickerState struct {
	lists map[string]*modelList
	gen   uint64
	// open is the key of the picker whose sheet shows; "" none.
	open   string
	filter string
	// hl is the highlighted row of the open sheet; reveal scrolls it into view.
	hl     int
	reveal bool
}

// pickReq is one model picker.
type pickReq struct {
	// key is the trigger's element key. One sheet per key.
	key string
	// listKey is the key in App.pick.lists; provider is what it lists.
	listKey  string
	provider config.Provider
	// secrets lists a provider that is not saved yet; nil uses Deps.Secrets.
	secrets providers.SecretStore
	// label is the trigger's text; chosen false shows it in the accent.
	label  string
	chosen bool
	// selected is the chosen model id, "" none.
	selected string
	// extra is a row above the models, as "Provider's model" in the style
	// editor. Picking it gives "". extraOn marks it chosen.
	extra   string
	extraOn bool
	// thinking shows the thinking level of the provider's model.
	thinking bool
	// field draws the trigger as a full-width raised field (the style
	// editor and onboarding) instead of the inline label.
	field bool
	// small is the field trigger of onboarding: 34 high, 13 px text.
	small bool
	// a11y names the trigger; "" is "Model: <label>".
	a11y string
	// load fetches the list as the sheet opens; nil fetches provider's.
	load func()
	// pick gets the chosen id, or "" for the extra row.
	pick func(id string)
}

// list is the cached list of key, or nil.
func (a *App) list(key string) *modelList {
	return a.pick.lists[key]
}

// loadModels fetches the models of pr into key. Without force a list that
// is loading or loaded stays as it is (model.rs load_models).
func (a *App) loadModels(key string, pr config.Provider, secrets providers.SecretStore, force bool) {
	if a.pick.lists == nil {
		a.pick.lists = map[string]*modelList{}
	}
	if pr.Kind == config.KindCleanupModel {
		// The catalog fixes its model: an empty list, never a fetch that
		// shows Loading forever.
		a.pick.lists[key] = &modelList{phase: listReady, models: []enhance.ModelChoice{}}
		return
	}
	if l := a.pick.lists[key]; l != nil && (l.phase == listLoading || (l.phase == listReady && !force)) {
		return
	}
	if secrets == nil {
		secrets = a.d.Secrets
	}
	a.pick.gen++
	gen := a.pick.gen
	a.pick.lists[key] = &modelList{phase: listLoading, gen: gen}
	lister := a.d.ListModels
	if lister == nil {
		lister = enhance.ListProviderModels
	}
	lm := a.engineLanguageModel()
	a.background(func() {
		models, err := lister(pr, secrets, lm, modelListTimeout)
		a.post(func() {
			l := a.pick.lists[key]
			if l == nil || l.gen != gen {
				return // dropped or replaced while it loaded
			}
			if err != nil {
				l.phase, l.err = listFailed, err.Error()
			} else {
				l.phase, l.models = listReady, models
			}
			a.invalidate()
		})
	})
	a.invalidate()
}

// loadProviderModels fetches the list of a saved provider.
func (a *App) loadProviderModels(id string, force bool) {
	if pr, ok := a.cfg.AI.Provider(id); ok {
		a.loadModels(id, pr, nil, force)
	}
}

// dropModels forgets a provider's list, after its key, login or folder changed.
func (a *App) dropModels(id string) { delete(a.pick.lists, id) }

// listedModel is the model id of key's ready list, if it has it.
func (a *App) listedModel(key, id string) (enhance.ModelChoice, bool) {
	if l := a.list(key); l != nil && l.phase == listReady {
		for _, m := range l.models {
			if m.ID == id {
				return m, true
			}
		}
	}
	return enhance.ModelChoice{}, false
}

// modelLabel is the provider's name of a model ("Haiku 4.5") when its list
// is loaded, else the last part of the id (model.rs model_label).
func (a *App) modelLabel(key, id string) string {
	if m, ok := a.listedModel(key, id); ok {
		return m.Name
	}
	return shortModel(id)
}

// shortModel is the last part of a model id: "openai/gpt-5" is "gpt-5".
func shortModel(id string) string {
	if i := strings.LastIndex(id, "/"); i >= 0 && i < len(id)-1 {
		return id[i+1:]
	}
	return id
}

// thinkingChoices are the levels to offer for the model of pr, from the
// least thinking to the most (model.rs thinking_choices). They come from
// the provider's list. Until it is here, Claude and ChatGPT offer the
// levels that all their models with levels take. A level from the config
// file that the list lacks still shows. Empty for a model without levels.
func (a *App) thinkingChoices(pr config.Provider) []string {
	var levels []string
	if m, ok := a.listedModel(pr.ID, pr.ModelName()); ok {
		levels = append(levels, m.Thinking...)
	} else if pr.Kind == config.KindClaudeCLI || pr.Kind == config.KindChatGPT {
		levels = []string{config.DefaultCLIThinking, "medium", "high"}
	}
	if t := pr.ThinkingChosen(); t != "" && !contains(levels, t) {
		levels = append(levels, t)
	}
	return levels
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// editProvider changes the saved provider id with fn, and saves.
func (a *App) editProvider(id string, fn func(p *config.Provider)) {
	for j := range a.cfg.AI.Providers {
		if a.cfg.AI.Providers[j].ID == id {
			fn(&a.cfg.AI.Providers[j])
		}
	}
	a.saveConfig()
}

// setProviderModel chooses the model a provider uses when a style names
// none. The new model can take other thinking levels than the old one
// (model.rs set_provider_model).
func (a *App) setProviderModel(id, model string) {
	m, listed := a.listedModel(id, model)
	a.editProvider(id, func(p *config.Provider) {
		p.SetModel(model)
		if listed {
			p.FitThinking(m.Thinking)
		}
	})
}

// providerModelLabel is the trigger text of a provider: "Haiku 4.5 · Low"
// (detail.rs model_picker).
func (a *App) providerModelLabel(pr config.Provider) string {
	label := i18n.T("settings.providers.default_model")
	switch {
	case pr.ModelName() != "":
		label = a.modelLabel(pr.ID, pr.ModelName())
	case pr.NeedsModel():
		label = i18n.T("settings.providers.choose_model")
	}
	if sent := pr.ThinkingSent(); sent != "" && contains(a.thinkingChoices(pr), sent) {
		label += " · " + config.ThinkingLabel(sent)
	}
	return label
}

// providerModelPicker is the model picker of a saved provider in Settings.
func (a *App) providerModelPicker(c *mu.Context, p theme.Colors, pr config.Provider) {
	id := pr.ID
	a.modelPicker(c, p, pickReq{
		key: "providers.model." + id, listKey: id, provider: pr,
		label: a.providerModelLabel(pr), chosen: !pr.NeedsModel(), selected: pr.ModelName(), thinking: true,
		pick: func(model string) {
			if model != "" {
				a.setProviderModel(id, model)
			}
		},
	})
}

// pickerRow is a row of the sheet: the extra row, a model, or the typed id.
type pickerRow struct {
	key, id, name, mono, note string
	on                        bool
}

// pickerRows are the rows the filter shows, in order, and the model count
// of the ready list (-1 while it is not ready).
func (a *App) pickerRows(r pickReq) (rows []pickerRow, count int) {
	count = -1
	if r.extra != "" {
		rows = append(rows, pickerRow{key: "model_picker.extra", name: r.extra, on: r.extraOn})
	}
	l := a.list(r.listKey)
	if l == nil || l.phase == listLoading {
		return rows, count
	}
	var models []enhance.ModelChoice
	if l.phase == listReady {
		models, count = l.models, len(l.models)
	}
	query := strings.ToLower(strings.TrimSpace(a.pick.filter))
	exact := false
	for _, m := range models {
		if query != "" && !strings.Contains(strings.ToLower(m.Name), query) && !strings.Contains(strings.ToLower(m.ID), query) {
			continue
		}
		exact = exact || strings.EqualFold(m.ID, query)
		row := pickerRow{key: "model_picker.row." + m.ID, id: m.ID, name: m.Name, on: m.ID == r.selected}
		// HTTP lists give only ids: the name is then the id, and it shows one time.
		if m.Name != m.ID {
			row.mono = m.ID
		}
		if m.Description != nil {
			row.note = *m.Description
		}
		rows = append(rows, row)
	}
	// A server that does not list a model can still run it: the typed text
	// is a choice of its own when no listed id matches it exactly, also when
	// the list failed or is empty.
	if typed := strings.TrimSpace(a.pick.filter); typed != "" && !exact && r.provider.Kind == config.KindOpenAICompatible {
		rows = append(rows, pickerRow{key: "model_picker.custom", id: typed, name: i18n.T("model_picker.use_typed", "model", typed)})
	}
	return rows, count
}

// openPicker opens the sheet of r, or closes it when it is open.
func (a *App) openPicker(r pickReq) {
	if a.pick.open == r.key {
		a.pick.open = ""
		return
	}
	a.pick.open, a.pick.filter, a.pick.reveal = r.key, "", true
	if r.load != nil {
		r.load()
	} else {
		a.loadModels(r.listKey, r.provider, r.secrets, false)
	}
	a.pick.hl = 0
	rows, _ := a.pickerRows(r)
	for i, row := range rows {
		if row.on {
			a.pick.hl = i
		}
	}
}

// modelPicker builds the trigger of r and, while it is open, its sheet.
func (a *App) modelPicker(c *mu.Context, p theme.Colors, r pickReq) {
	open := a.pick.open == r.key
	label := r.a11y
	if label == "" {
		label = i18n.T("model_picker.trigger_label", "model", r.label)
	}
	t := mu.ButtonBase(a.k(c, r.key)).Label(label).HasPopup(mu.PopupListBox).Expanded(open).
		AlignItems(mu.Center).Gap(4).Cursor(mu.CursorPointer)
	fg := p.Ink
	if !r.chosen {
		fg = p.Accent
	}
	if r.field {
		h, fs := float32(36), float32(14)
		if r.small {
			h, fs = 34, 13
		}
		t.Height(h).PaddingX(12).Gap(8).Radius(9).Justify(mu.SpaceBetween).Grow(1).Basis(0).MinWidth(0)
		raisedSmall(t, p)
		t.Children(func() {
			uiText(c, r.label, fs, 400, fg).SingleLine().Ellipsis("…").Grow(1).Basis(0).MinWidth(0)
			mu.Icon(c, icon("chevron-down")).FontSize(12).TextColor(p.Graphite)
		})
	} else {
		// The inline trigger: "Haiku 4.5 ⌄", its text where the row's text would be.
		t.Height(22).PaddingX(7).Margin(0, -7, 0, 0).Radius(6).Shrink(0).MaxWidth(260)
		if t.Hovered() || open {
			t.Background(p.SheetRaised)
		}
		t.Children(func() {
			uiText(c, r.label, 12, 600, fg).SingleLine().Ellipsis("…").Shrink(1).MinWidth(0)
			mu.Icon(c, icon("chevron-down")).FontSize(10).TextColor(fg)
		})
	}
	if clicked(c, t) {
		a.openPicker(r)
		open = a.pick.open == r.key
	}
	mu.PopoverBase(c, t, &open, func(panel mu.Element) {
		floating(panel.Margin(4, 0, 0, 0).Width(pickerSheetW).Radius(12).Clip().Role(mu.RoleDialog).
			Label(i18n.T("model_picker.title")), p)
		mu.Column(c).Children(func() { a.pickerSheet(c, p, panel, r) })
	})
	if !open && a.pick.open == r.key {
		a.pick.open = "" // Escape or a press outside closed it
	}
}

// pickerSheet is the content of the open sheet: the filter head, the rows,
// and the thinking level.
func (a *App) pickerSheet(c *mu.Context, p theme.Colors, panel mu.Element, r pickReq) {
	l := a.list(r.listKey)
	rows, count := a.pickerRows(r)
	if a.pick.hl >= len(rows) {
		a.pick.hl = len(rows) - 1
	}
	if a.pick.hl < 0 && len(rows) > 0 {
		a.pick.hl = 0
	}
	choose := func(row pickerRow) {
		a.pick.open = ""
		if r.pick != nil {
			r.pick(row.id)
		}
		a.invalidate()
	}

	var filter mu.Element
	mu.Row(c).Gap(8).Padding(6, 6, 6, 14).AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		mu.Icon(c, icon("search")).FontSize(12).TextColor(p.Pencil)
		before := a.pick.filter
		filter = mu.TextInputBase(a.k(c, "model_picker.filter"), &a.pick.filter).Label(i18n.T("model_picker.filter")).
			Placeholder(i18n.T("model_picker.filter")).PlaceholderColor(p.Pencil).FontSize(13).TextColor(p.Ink).
			Grow(1).Basis(0).MinWidth(0).AutoFocus()
		if a.pick.filter != before {
			a.pick.hl, a.pick.reveal = 0, true
		}
		if count >= 0 {
			text := i18n.TP("model_picker.count", count, map[string]any{"count": count})
			mu.Text(a.k(c, "model_picker.count"), strings.ToUpper(text)).FontSize(10).FontWeight(600).TextColor(p.Pencil).
				SingleLine().Shrink(0).Role(mu.RoleStatus)
		}
		loading := l != nil && l.phase == listLoading
		if a.button(c, p, "model_picker.refresh", i18n.T("model_picker.refresh"), bopt{kind: btnGhost, size: sizeSmall, disabled: loading}) && !loading {
			a.loadModels(r.listKey, r.provider, r.secrets, true)
		}
	})

	list := mu.Scroll(a.k(c, "model_picker.rows")).MaxHeight(pickerRowsH).Role(mu.RoleListBox).Label(i18n.T("model_picker.models_label"))
	list.Children(func() {
		mu.Column(c).Padding(4).Children(func() {
			message := func(key, text string, col mu.Color, alert bool) {
				m := uiText(a.k(c, key), text, 13, 400, col).LineHeight(18.0/13).Padding(8, 10)
				if alert {
					m.Role(mu.RoleAlert)
				} else {
					m.Role(mu.RoleStatus)
				}
			}
			idx := 0
			// The extra row comes first.
			if r.extra != "" {
				a.pickerRowView(c, p, rows[0], idx == a.pick.hl, choose)
				idx++
			}
			switch {
			case l == nil || l.phase == listLoading:
				message("model_picker.status", i18n.T("model_picker.loading"), p.Graphite, false)
			case l.phase == listFailed:
				message("model_picker.status", i18n.T("model_picker.load_failed", "error", l.err), p.Danger, true)
			case len(l.models) == 0:
				message("model_picker.status", i18n.T("model_picker.no_models"), p.Graphite, false)
			case count > 0 && !hasModelRow(rows):
				message("model_picker.status", i18n.T("model_picker.no_match"), p.Graphite, false)
			}
			for ; idx < len(rows); idx++ {
				a.pickerRowView(c, p, rows[idx], idx == a.pick.hl, choose)
			}
		})
	})

	if r.thinking {
		a.thinkingPanel(c, p, r.provider.ID)
	}

	// The arrows move the highlight, Enter picks it (the filter keeps the
	// focus, as a combo box does). Escape is PopoverBase's.
	switch {
	case panel.Shortcut(0, mu.KeyDown) && len(rows) > 0:
		a.pick.hl, a.pick.reveal = min(a.pick.hl+1, len(rows)-1), true
		c.Invalidate()
	case panel.Shortcut(0, mu.KeyUp) && len(rows) > 0:
		a.pick.hl, a.pick.reveal = max(a.pick.hl-1, 0), true
		c.Invalidate()
	case (filter.Submitted() || panel.Shortcut(0, mu.KeyEnter)) && a.pick.hl >= 0 && a.pick.hl < len(rows):
		choose(rows[a.pick.hl])
	}
}

func hasModelRow(rows []pickerRow) bool {
	for _, r := range rows {
		if r.key != "model_picker.extra" && r.key != "model_picker.custom" {
			return true
		}
	}
	return false
}

// pickerRowView is one row: the name in the display serif, the id at the
// right in mono, a note under it, and an ink dot on the chosen model.
func (a *App) pickerRowView(c *mu.Context, p theme.Colors, row pickerRow, hl bool, choose func(pickerRow)) {
	b := mu.ButtonBase(a.k(c, row.key)).Label(row.name).Role(mu.RoleOption).Checked(row.on).
		Description(strings.Trim(row.mono+". "+row.note, ". ")).
		Padding(6, 10).Gap(8).Radius(8).AlignItems(mu.Start).Cursor(mu.CursorPointer)
	switch {
	case row.on:
		b.Background(p.Sheet).InsetShadow(0, 1, 2, 0, p.ShadowA(0.14)).InsetShadow(0, -1, 0, 0, p.Highlight(0.8))
	case hl || b.Hovered():
		b.Background(p.Sheet)
	}
	if hl && a.pick.reveal {
		b.ScrollIntoView()
		a.pick.reveal = false
	}
	b.Children(func() {
		mu.Box(c).Width(8).Height(20).Shrink(0).AlignItems(mu.Center).Justify(mu.Center).Children(func() {
			if row.on {
				mu.Box(c).Size(7, 7).Radius(99).Background(p.Ink)
			}
		})
		mu.Column(c).Gap(1).Grow(1).Basis(0).MinWidth(0).Children(func() {
			mu.Row(c).Gap(12).AlignItems(mu.Center).Justify(mu.SpaceBetween).Children(func() {
				weight := 400
				if row.on {
					weight = 600
				}
				mu.Text(c, row.name).Font(serifFont(theme.Display)).FontSize(15).FontWeight(weight).LineHeight(20.0 / 15).
					TextColor(p.Ink).SingleLine().Ellipsis("…").Shrink(1).MinWidth(0)
				if row.mono != "" {
					col := p.Pencil
					if row.on {
						col = p.Graphite
					}
					mono(c, row.mono, 10, col).Shrink(0)
				}
			})
			if row.note != "" {
				uiText(c, row.note, 11, 400, p.Graphite).LineHeight(15.0 / 11).SingleLine().Ellipsis("…")
			}
		})
	})
	if clicked(c, b) {
		choose(row)
	}
}

// thinkingPanel is the thinking level of the provider's chosen model: a
// caps label, the level's name in the italic display serif, and the pen
// line, a slider with a stop for each level. Dragging, a click, or the
// arrows move it one stop at a time; the sheet stays open.
func (a *App) thinkingPanel(c *mu.Context, p theme.Colors, id string) {
	pr, ok := a.cfg.AI.Provider(id)
	if !ok {
		return
	}
	levels := a.thinkingChoices(pr)
	if len(levels) == 0 {
		return
	}
	chosen := indexOf(levels, pr.ThinkingSent())
	name := i18n.T("model_picker.default")
	if chosen >= 0 {
		name = config.ThinkingLabel(levels[chosen])
	}
	mu.Column(c).Gap(4).Padding(12, thinkingPad, 10, thinkingPad).Background(p.Sheet).BorderWidth(1, 0, 0, 0).BorderColor(p.Rule).Children(func() {
		mu.Row(c).AlignItems(mu.Center).Justify(mu.SpaceBetween).Children(func() {
			mu.Text(c, strings.ToUpper(i18n.T("model_picker.thinking"))).FontSize(10).FontWeight(600).TextColor(p.Graphite).SingleLine()
			mu.Text(c, name).Font(serifFont(theme.Display)).FontSize(21).LineHeight(24.0 / 21).Italic().TextColor(p.Ink).SingleLine()
		})
		w := float32(pickerSheetW - 2*thinkingPad)
		scale := w / penW
		value := float64(max(chosen, 0))
		before := value
		s := mu.SliderBase(a.k(c, "model_picker.thinking"), &value, 0, float64(len(levels)-1)).Step(1).
			Label(i18n.T("model_picker.thinking")).Value(name).
			Width(w).Height(float32(math.Ceil(float64(penLineH*scale)))).
			Padding(0, (penW-penLastStop)*scale, 0, penFirstStop*scale).Cursor(mu.CursorPointer)
		stops, marked := len(levels), chosen
		s.Draw(func(pt *mu.Painter, r mu.Rect) {
			v := float32(1)
			if stops > 1 {
				v = float32(value) / float32(stops-1)
			}
			paintPenLine(pt, r, v, stops, marked, p)
		})
		if s.Changed() || value != before {
			stop := int(math.Round(value))
			if stop >= 0 && stop < len(levels) && stop != chosen {
				level := levels[stop]
				a.editProvider(id, func(p *config.Provider) { p.SetThinking(&level) })
				c.Invalidate()
			}
		}
	})
}

func indexOf(list []string, s string) int {
	for i, x := range list {
		if x == s && s != "" {
			return i
		}
	}
	return -1
}

// The pen line (crates/sayso-ui/src/components/pen_line.rs): one stroke
// that gets heavier from the left to the right, with a tick for each stop.
// Ink fills it up to the value; the rest stays as an impression in the
// paper. The geometry is in design units, 352 wide, scaled to the box.
const (
	penW         = 352
	penStart     = 4
	penEnd       = 346
	penFirstStop = 24
	penLastStop  = 344
	penHalfH     = 22
	penTicks     = 10
	penLineH     = penHalfH*2 + penTicks
)

// penSpecks are specks of ink near the heavy end: x, the offset from the
// line's center, and the radius. One shows when the ink reaches it.
var penSpecks = [...][3]float32{
	{150, -5, 0.45}, {172, 6, 0.55}, {203, -7, 0.6}, {213, -9.5, 0.4}, {236, 8, 0.7}, {249, 10.5, 0.4},
	{262, -9, 0.8}, {270, -12.5, 0.45}, {289, 10, 0.9}, {296, 13.5, 0.45}, {309, -11, 0.5}, {318, -10, 1.0},
	{325, -14, 0.5}, {333, 11.5, 0.75}, {341, 15, 0.45}, {350, -7, 0.6}, {351, 9, 0.4},
}

func penStopX(i, count int) float32 {
	if count < 2 {
		return penLastStop
	}
	return penFirstStop + (penLastStop-penFirstStop)*float32(i)/float32(count-1)
}

// penCenter is the line's center at x, as an offset from the middle: a slow wave.
func penCenter(x float32) float32 {
	return float32(-3 * math.Sin(2*math.Pi*float64(x-penStart)/176))
}

// penHalfWidth is half the line's width at x: a hairline at the start,
// heavy at the end, a downstroke a little heavier than an upstroke.
func penHalfWidth(x float32) float32 {
	t := min(max((x-penStart)/(penEnd-penStart), 0), 1)
	slope := penCenter(x+0.5) - penCenter(x-0.5)
	return (0.45 + 6.6*float32(math.Pow(float64(t), 1.5))) * (1 + 2.2*slope)
}

func penInkEnd(v float32) float32 {
	if v >= 0.999 {
		return penEnd
	}
	return penFirstStop + (penLastStop-penFirstStop)*min(max(v, 0), 1)
}

// penOutline is the outline from the start to end, grown by grow, with a
// flat, slightly oblique end as a nib leaves it.
func penOutline(end, grow float32, at func(x, y float32) (float32, float32)) *mu.Path {
	var xs []float32
	for x := float32(penStart); x < end-1.5; x += 3 {
		xs = append(xs, x)
	}
	xs = append(xs, end)
	slant := min(penHalfWidth(end)*0.35, 1.3)
	last := len(xs) - 1
	path := &mu.Path{}
	for i, x := range xs {
		px := x
		if i == last {
			px = x + slant + grow
		}
		X, Y := at(px, penCenter(x)-penHalfWidth(x)-grow)
		if i == 0 {
			path.MoveTo(X, Y)
		} else {
			path.LineTo(X, Y)
		}
	}
	for i := last; i >= 0; i-- {
		x := xs[i]
		px := x
		if i == last {
			px = x - slant + grow
		}
		path.LineTo(at(px, penCenter(x)+penHalfWidth(x)+grow))
	}
	return path.Close()
}

// paintPenLine paints the pen line in r: v is where the ink ends, 0 at the
// first stop and 1 at the last; marked is the stop with the ink tick (-1 none).
func paintPenLine(pt *mu.Painter, r mu.Rect, v float32, stops, marked int, p theme.Colors) {
	scale := r.W / penW
	mid := r.Y + penHalfH*scale
	at := func(x, y float32) (float32, float32) { return r.X + x*scale, mid + y*scale }
	dot := func(x, y, rad float32, col mu.Color) {
		cx, cy := at(x, y)
		rad *= scale
		pt.Fill(mu.Rect{X: cx - rad, Y: cy - rad, W: rad * 2, H: rad * 2}, col, rad)
	}
	end := penInkEnd(v)
	if end < penEnd {
		pt.FillPath(penOutline(penEnd, 0.5, at), p.DebossShade)
		pt.FillPath(penOutline(penEnd, 0, at), p.Deboss)
	}
	pt.FillPath(penOutline(end, 0, at), p.Ink)
	dot(penStart+0.6, penCenter(penStart), 1.1, p.Ink)
	for _, s := range penSpecks {
		if s[0] <= end+7 {
			dot(s[0], penCenter(s[0])+s[1], s[2], p.Ink)
		}
	}
	tickTop := float32(penHalfH + 1)
	for i := range stops {
		x := r.X + penStopX(i, stops)*scale
		w, h, y, col := float32(1), float32(4), tickTop, p.Pencil
		if i == marked {
			w, h, y, col = 1.6, 8, tickTop-2, p.Ink
		}
		pt.Fill(mu.Rect{X: x - w/2, Y: mid + y*scale, W: w, H: h * scale}, col, w/2)
	}
}
