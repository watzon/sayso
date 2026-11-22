package ui

import (
	"fmt"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// styleDraft is the editor's copy of a style.
type styleDraft struct {
	id         string
	isNew      bool
	origin     enhance.StyleOrigin
	name       string
	desc       string
	ink        string
	prompt     string
	useBase    bool
	provider   string // "" is the default provider
	model      string
	timeout    string
	cleanup    bool
	tone       int
	layout     int
	email      bool
	apps       []string
	sites      []string
	newTarget  string
	hotkey     *config.Hotkey
	hotkeyDirt bool
}

type stylesState struct {
	editing  bool
	draft    styleDraft
	baseOpen bool
	base     string
	err      string
	names    map[string]string // bundle id → app name, for the rows
	// customInk shows the #hex field of the editor's ink.
	customInk bool
}

func (s *stylesState) close() { s.editing, s.baseOpen, s.err = false, false, "" }

// Ink presets the editor offers, as #RRGGBB.
var inkPresets = []theme.InkPreset{theme.Sumi, theme.Indigo, theme.IronGall, theme.Sepia, theme.Verdigris, theme.Oxblood}

func (a *App) stylesDir() string {
	if a.d.Paths.ConfigDir == "" {
		return ""
	}
	return a.d.Paths.StylesDir()
}

func (a *App) stylesPage(c *mu.Context, p theme.Colors) {
	pageHead(c, p, i18n.T("styles.title"), i18n.T("styles.intro", "dir", tildePath(a.stylesDir())), 560, func() {
		if a.button(c, p, "styles.base_prompt", i18n.T("styles.base.button"), bopt{icon: "file", size: sizeLarge}) {
			a.sty.baseOpen, a.sty.editing = true, false
			a.sty.base = a.styles.BasePrompt()
		}
		if a.button(c, p, "styles.new", i18n.T("styles.new_style"), bopt{kind: btnPrimary, icon: "plus", size: sizeLarge}) {
			a.editStyle(nil)
		}
	})
	for _, e := range a.styles.Errors {
		uiText(c, i18n.T("styles.file_error", "error", e), 13, 400, p.Danger)
	}
	active := a.ActiveStyle()
	a.featuredStyle(c, p, active)
	mu.Column(c).Role(mu.RoleRadioGroup).Label(i18n.T("styles.a11y.default_style")).Children(func() {
		for i := range a.styles.Entries {
			a.styleRow(c, p, &a.styles.Entries[i], active, i == len(a.styles.Entries)-1)
		}
	})
	a.providerLine(c, p)
}

// providerLine is the footer: where styles run, and Manage providers.
func (a *App) providerLine(c *mu.Context, p theme.Colors) {
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		prov, ok := a.defaultProvider()
		var line string
		dot := p.Success
		switch {
		case !a.cfg.AI.Enabled:
			line, dot = i18n.T("styles.provider_line.ai_off"), p.Pencil
		case !ok:
			line, dot = i18n.T("styles.provider_line.no_provider"), p.Pencil
		case prov.ModelName() == "":
			line = i18n.T("styles.provider_line.no_model", "provider", prov.Name)
		default:
			line = i18n.T("styles.provider_line.with_model", "provider", prov.Name, "model", prov.ModelName())
		}
		mu.Box(c).Size(8, 8).Radius(99).Background(dot)
		uiText(c, line, 13, 400, p.Graphite).Grow(1).Basis(0)
		label := i18n.T("styles.provider_line.manage")
		if !ok {
			label = i18n.T("styles.provider_line.set_up")
		}
		if a.link(c, p, "styles.providers", label, 13, 600, "chevron-right") {
			a.OpenSettings(SecProviders)
		}
	})
}

// featuredID is the style the page shows with an example (styles.rs).
const featuredID = "rambling"

// featuredStyle is the recommended style with what it does, before and after.
func (a *App) featuredStyle(c *mu.Context, p theme.Colors, active string) {
	e := a.styleEntry(featuredID)
	if e == nil {
		e = a.styleEntry(active)
	}
	mu.Row(c).Padding(20, 22).Gap(24).Radius(14).Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).
		Shadow(0, 1, 2, 0, p.ShadowA(0.14)).Shadow(0, 8, 22, 0, p.ShadowA(0.08)).Children(func() {
		mu.Column(c).Gap(8).Width(252).Shrink(0).Children(func() {
			if e == nil {
				return
			}
			st := &e.Style
			mu.Row(c).Height(20).Gap(8).AlignItems(mu.Center).Children(func() {
				sealDot(c, p, st.Ink, 14)
				caps(c, p, i18n.T("styles.recommended"))
			})
			heading(c, p, styleName(st), 26).LineHeight(32.0 / 26)
			uiText(c, styleDescription(st), 14, 400, p.Graphite).LineHeight(20.0 / 14)
			mu.Row(c).Gap(8).Padding(4, 0, 0, 0).AlignItems(mu.Center).Children(func() {
				if st.ID == active {
					badge(c, p, i18n.T("styles.default"), toneInk, "")
				} else if a.button(c, p, "styles.featured.make_default", i18n.T("styles.make_default"), bopt{size: sizeSmall}) {
					a.SetActiveStyle(st.ID)
				}
				if a.button(c, p, "styles.featured.edit", i18n.T("styles.edit"), bopt{kind: btnGhost, size: sizeSmall}) {
					a.editStyle(e)
				}
			})
		})
		debossed(mu.Row(c).Grow(1).Basis(0).MinWidth(0).Radius(10).AlignItems(mu.Stretch), p).Children(func() {
			half := func(label string, body func()) {
				mu.Column(c).Grow(1).Basis(0).Padding(14, 16).Gap(6).Children(func() {
					uiText(c, strings.ToUpper(label), 11, 500, p.Graphite).SingleLine()
					body()
				})
			}
			half(i18n.T("styles.featured.you_say"), func() {
				mu.Text(c, i18n.T("styles.featured.said")).Font(theme.Mono).FontSize(12).FixedLineHeight(18).TextColor(p.Graphite)
			})
			mu.Box(c).Width(1).Background(p.DebossShade)
			half(i18n.T("styles.featured.sayso_writes"), func() {
				uiText(c, i18n.T("styles.featured.written"), 14, 500, p.Ink).LineHeight(20.0 / 14)
			})
		})
	})
}

func (a *App) styleEntry(id string) *enhance.StyleEntry {
	for i := range a.styles.Entries {
		if a.styles.Entries[i].Style.ID == id {
			return &a.styles.Entries[i]
		}
	}
	return nil
}

// appNames maps bundle ids to the names History knows.
func (a *App) appNames() map[string]string {
	if a.sty.names == nil {
		a.sty.names = map[string]string{}
		if a.d.Store != nil {
			if apps, err := a.d.Store.AppList(); err == nil {
				for _, ac := range apps {
					a.sty.names[ac.BundleID] = ac.Name
				}
			}
		}
	}
	return a.sty.names
}

// targetLine names the apps and sites a style is used in, or "".
func (a *App) targetLine(id string) string {
	r, ok := a.cfg.AI.StyleRules.Rule(id)
	if !ok {
		return ""
	}
	names := a.appNames()
	// The apps on this computer first, by the names the user knows
	// (styles.rs target_names).
	installed, _ := a.installedNames(r.Apps)
	var here, away []string
	for _, b := range r.Apps {
		if n := installed[b]; n != "" {
			here = append(here, n)
			continue
		}
		n := names[b]
		if n == "" {
			n = appNameFor(b)
		}
		away = append(away, n)
	}
	all := append(here, away...)
	all = append(all, r.Sites...)
	switch len(all) {
	case 0:
		return ""
	case 1:
		return all[0]
	case 2:
		return i18n.T("styles.targets.names_two", "one", all[0], "two", all[1])
	}
	return i18n.T("styles.targets.names_many", "one", all[0], "two", all[1], "more", len(all)-2)
}

// styleRowDetail is the detail column of a style row and its color.
func (a *App) styleRowDetail(st *enhance.Style, p theme.Colors) (string, mu.Color, bool) {
	if line := a.targetLine(st.ID); line != "" {
		return line, p.Ink, true
	}
	if !st.UsesAI() {
		return i18n.T("styles.route.no_ai", "computer", computer()), p.Graphite, false
	}
	if st.Provider == nil && st.Model == nil {
		return "", p.Graphite, false // the footer names the default provider
	}
	line := a.styleRoute(st)
	col := p.Pencil
	if pr, ok := a.cfg.AI.Provider(derefOr(st.Provider, "")); ok {
		col = p.Success
		if pr.IsCloud() {
			col = p.Accent
		}
	}
	return line, col, false
}

func (a *App) styleRow(c *mu.Context, p theme.Colors, e *enhance.StyleEntry, active string, last bool) {
	st := &e.Style
	name := styleName(st)
	row := mu.Row(c).Height(48).PaddingX(4).Gap(14).AlignItems(mu.Center)
	if !last {
		row.BorderWidth(0, 0, 1, 0).BorderColor(p.Rule)
	}
	if row.Hovered() {
		row.Background(p.Deboss.Alpha(0.55))
	}
	row.Children(func() {
		r := mu.ButtonBase(a.k(c, "styles.default."+st.ID)).Role(mu.RoleRadio).Checked(st.ID == active).
			Label(i18n.T("styles.a11y.make_default", "name", name)).FillHeight().Gap(14).Grow(1).Basis(0).MinWidth(0).
			AlignItems(mu.Center).Cursor(mu.CursorPointer)
		r.Children(func() {
			ink := st.Ink
			if st.IsRaw() {
				ink = "" // Raw has the open ring
			}
			sealDot(c, p, ink, 14)
			heading(c, p, name, 18).LineHeight(24.0 / 18).Width(112).Shrink(0).SingleLine().Ellipsis("…")
			uiText(c, styleDescription(st), 14, 400, p.Graphite).SingleLine().Ellipsis("…").Grow(1).Basis(0).MinWidth(0)
			mu.Row(c).Width(236).Shrink(0).Gap(6).AlignItems(mu.Center).Children(func() {
				line, col, target := a.styleRowDetail(st, p)
				if line == "" {
					return
				}
				if target {
					mu.Icon(c, icon("arrow-right")).FontSize(11).TextColor(col)
				}
				uiText(c, line, 12, 500, col).SingleLine().Ellipsis("…")
			})
		})
		if clicked(c, r) {
			a.SetActiveStyle(st.ID)
		}
		// Fixed lanes for the shortcut and for Default / Edit, so the
		// columns line up on every row.
		mu.Row(c).Width(76).Shrink(0).Justify(mu.End).AlignItems(mu.Center).Children(func() {
			if hk, ok := a.cfg.StyleHotkeys.Get(st.ID); ok {
				miniKey(c, p, hk.CompactLabel(), p.Ink)
			}
		})
		mu.Row(c).Width(80).Shrink(0).Justify(mu.End).AlignItems(mu.Center).Children(func() {
			if row.Hovered() || row.FocusWithin() {
				if a.button(c, p, "styles.edit."+st.ID, i18n.T("styles.edit"), bopt{kind: btnGhost, size: sizeSmall}) {
					a.editStyle(e)
				}
			} else if st.ID == active {
				badge(c, p, i18n.T("styles.default"), toneInk, "")
			}
		})
	})
}

// editStyle opens the editor drawer on a style, or a new one for nil.
func (a *App) editStyle(e *enhance.StyleEntry) {
	d := styleDraft{isNew: e == nil, useBase: true, ink: theme.Indigo.Color().Hex()}
	if e != nil {
		st := e.Style
		d.id, d.origin, d.name, d.desc, d.ink, d.prompt = st.ID, e.Origin, styleName(&st), st.Description, st.Ink, st.Prompt
		if d.desc == "" {
			d.desc = styleDescription(&st)
		}
		d.useBase = !st.Standalone
		d.provider, d.model = derefOr(st.Provider, ""), derefOr(st.Model, "")
		if st.TimeoutMs != nil {
			d.timeout = strconv.FormatFloat(float64(*st.TimeoutMs)/1000, 'f', -1, 64)
		}
		if cs := st.CleanupSettingsOf(); cs != nil {
			if s, ok := cs.Known(); ok {
				d.cleanup, d.tone, d.layout, d.email = true, int(s.Tone), int(s.Layout), s.Format == enhance.FormatEmail
			}
		}
		if r, ok := a.cfg.AI.StyleRules.Rule(st.ID); ok {
			d.apps, d.sites = append([]string(nil), r.Apps...), append([]string(nil), r.Sites...)
		}
		if hk, ok := a.cfg.StyleHotkeys.Get(st.ID); ok {
			d.hotkey = &hk
		}
	}
	a.sty.draft, a.sty.editing, a.sty.baseOpen, a.sty.err = d, true, false, ""
	a.invalidate()
}

// styleID makes a file id from a name: "My Slack" is "my-slack".
func styleID(name string) string {
	var b strings.Builder
	dash := false
	for _, r := range strings.ToLower(strings.TrimSpace(name)) {
		switch {
		case r >= 'a' && r <= 'z' || r >= '0' && r <= '9':
			b.WriteRune(r)
			dash = false
		case !dash && b.Len() > 0:
			b.WriteByte('-')
			dash = true
		}
	}
	return strings.TrimSuffix(b.String(), "-")
}

// SaveDraft writes the style being edited.
func (a *App) SaveDraft() error {
	d := &a.sty.draft
	if strings.TrimSpace(d.name) == "" {
		return fmt.Errorf("%s", i18n.T("styles.error.needs_name"))
	}
	if d.isNew {
		d.id = styleID(d.name)
	}
	st := &enhance.Style{ID: d.id, Name: strings.TrimSpace(d.name), Ink: d.ink, Description: d.desc, Prompt: d.prompt, Standalone: !d.useBase}
	if d.provider != "" {
		st.Provider = ptr(d.provider)
	}
	if d.model != "" {
		st.Model = ptr(d.model)
	}
	if t := strings.TrimSpace(d.timeout); t != "" {
		secs, err := strconv.ParseFloat(t, 64)
		if err != nil || secs < 0.5 || secs > 120 {
			return fmt.Errorf("%s", i18n.T("styles.error.bad_timeout"))
		}
		st.TimeoutMs = ptr(uint64(secs * 1000))
	}
	if d.cleanup {
		f := enhance.FormatPlain
		if d.email {
			f = enhance.FormatEmail
		}
		st.Cleanup = enhance.CleanupField{State: enhance.CleanupSettingsSet,
			Settings: enhance.NewCleanupSettings(enhance.Tone(d.tone), enhance.Layout(d.layout), f)}
	}
	if dir := a.stylesDir(); dir != "" {
		if _, err := enhance.SaveStyle(dir, st); err != nil {
			return fmt.Errorf("%s", i18n.T("styles.error.save_failed", "error", err.Error()))
		}
	}
	a.cfg.AI.StyleRules.SetTargets(st.ID, d.apps, d.sites)
	if d.hotkeyDirt {
		a.cfg.StyleHotkeys.Set(st.ID, d.hotkey)
	}
	a.saveConfig()
	a.reloadStyles()
	return nil
}

func (a *App) styleDrawers(c *mu.Context, p theme.Colors) {
	if a.sty.editing {
		a.drawer(c, p, func() { a.styleEditor(c, p) }, func() { a.styleEditorActions(c, p) })
	} else if a.sty.baseOpen {
		a.drawer(c, p, func() { a.baseEditor(c, p) }, func() { a.baseEditorActions(c, p) })
	}
}

// drawer is a 500 px floating panel inside the sheet, over a scrim that
// closes it. footer stays under the scrolling body, so Save is always in
// reach.
func (a *App) drawer(c *mu.Context, p theme.Colors, body, footer func()) {
	scrim := mu.ButtonBase(c.Key("drawer.scrim")).Label(i18n.T("styles.a11y.close")).Absolute().Top(0).Left(0).Right(0).Bottom(0).
		ZIndex(4).Background(p.Ground.Alpha(0.45)).Focusable()
	if clicked(c, scrim) {
		a.sty.close()
	}
	// 12 px inside the sheet, which sits 10 px inside the window (styles.rs).
	floating(mu.Column(c).Absolute().Top(22).Right(22).Bottom(22).Width(500).ZIndex(5).Radius(sheetRadius).Clip(), p).
		Role(mu.RoleDialog).Children(func() {
		mu.Scroll(c.Key("drawer")).Grow(1).Padding(28).Gap(18).Children(body)
		mu.Row(c).Gap(8).Padding(14, 28).AlignItems(mu.Center).BorderWidth(1, 0, 0, 0).BorderColor(p.Rule).Children(footer)
	})
}

func (a *App) styleEditor(c *mu.Context, p theme.Colors) {
	d := &a.sty.draft
	title := i18n.T("styles.editor.new_title")
	if !d.isNew {
		title = i18n.T("styles.editor.edit_title", "name", d.name)
	}
	mu.Column(c).Gap(10).Padding(6, 0, 0, 0).Children(func() {
		mu.Row(c).AlignItems(mu.Start).Children(func() {
			heading(c, p, title, 28).Grow(1).Role(mu.RoleHeading)
			if a.iconBtnSized(c, p, "styles.editor.close", "close", i18n.T("styles.a11y.close"), 30, 16) {
				a.sty.close()
			}
		})
		switch d.origin {
		case enhance.OriginBuiltIn:
			if !d.isNew {
				uiText(c, i18n.T("styles.editor.built_in_note"), 13, 400, p.Graphite)
			}
		case enhance.OriginOverridden:
			uiText(c, i18n.T("styles.editor.overridden_note"), 13, 400, p.Graphite)
		}
	})
	if a.sty.err != "" {
		uiText(c, a.sty.err, 13, 500, p.Danger).Role(mu.RoleAlert)
	}
	a.labelled(c, p, i18n.T("styles.editor.name"), func() {
		a.field(c, p, "styles.editor.name", i18n.T("styles.editor.name"), i18n.T("styles.editor.name_placeholder"), &d.name)
	})
	a.labelled(c, p, i18n.T("styles.editor.description"), func() {
		a.field(c, p, "styles.editor.description", i18n.T("styles.editor.description"), i18n.T("styles.editor.description_placeholder"), &d.desc)
	})
	a.labelled(c, p, i18n.T("styles.editor.ink"), func() { a.inkPicker(c, p, "styles.editor.ink", &d.ink) })
	a.labelled(c, p, i18n.T("styles.editor.provider"), func() { a.providerPicker(c, p, d) })
	a.labelled(c, p, i18n.T("styles.editor.prompt"), func() {
		a.textArea(c, p, "styles.editor.prompt", i18n.T("styles.editor.prompt"), i18n.T("styles.editor.prompt_placeholder"), &d.prompt, 150)
		uiText(c, i18n.T("styles.editor.prompt_hint"), 12, 400, p.Graphite)
	})
	mu.Row(c).Gap(16).AlignItems(mu.Center).Children(func() {
		mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
			uiText(c, i18n.T("styles.editor.use_base"), 13, 600, p.Ink)
			uiText(c, i18n.T("styles.editor.use_base_detail"), 12, 400, p.Graphite).LineHeight(1.35)
		})
		a.toggle(c, "styles.editor.use_base", i18n.T("styles.editor.use_base"), &d.useBase, nil)
	})
	mu.Row(c).Gap(12).AlignItems(mu.Start).Children(func() {
		mu.Column(c).Grow(1).Basis(0).Children(func() {
			a.labelled(c, p, i18n.T("styles.editor.model"), func() {
				a.field(c, p, "styles.editor.model", i18n.T("styles.editor.model"), i18n.T("styles.editor.provider_model"), &d.model)
			})
		})
		mu.Column(c).Width(150).Shrink(0).Children(func() {
			a.labelled(c, p, i18n.T("styles.editor.timeout"), func() {
				secs := a.cfg.AI.HTTPTimeoutMs / 1000
				a.field(c, p, "styles.editor.timeout", i18n.T("styles.editor.timeout"), i18n.T("styles.editor.timeout_placeholder", "seconds", secs), &d.timeout)
			})
		})
	})
	a.labelled(c, p, i18n.T("styles.editor.shortcut"), func() {
		a.recorder(c, p, "styles.editor.hotkey", i18n.T("styles.editor.shortcut"), d.hotkey, func(h *config.Hotkey) {
			d.hotkey, d.hotkeyDirt = h, true
		})
		uiText(c, i18n.T("styles.editor.shortcut_hint", "style", d.name), 12, 400, p.Graphite)
	})
	a.cleanupControls(c, p, d)
	a.targetsEditor(c, p, d)
	uiText(c, i18n.T("styles.editor.saved_in", "dir", tildePath(a.stylesDir())), 12, 400, p.Graphite)
}

// labelled is an editor field: its label over the control.
func (a *App) labelled(c *mu.Context, p theme.Colors, label string, fn func()) {
	mu.Column(c).Gap(6).Children(func() {
		uiText(c, label, 13, 600, p.Ink)
		fn()
	})
}

func (a *App) styleEditorActions(c *mu.Context, p theme.Colors) {
	d := &a.sty.draft
	{
		if a.btn(c, p, "styles.editor.save", i18n.T("styles.save"), btnPrimary) {
			if err := a.SaveDraft(); err != nil {
				a.sty.err = err.Error()
			} else {
				a.sty.close()
			}
		}
		if a.btn(c, p, "styles.editor.cancel", i18n.T("styles.cancel"), btnGhost) {
			a.sty.close()
		}
		mu.Spacer(c)
		if d.origin == enhance.OriginOverridden && a.btn(c, p, "styles.editor.reset", i18n.T("styles.editor.reset_built_in"), btnGhost) {
			a.resetStyle(d.id)
		}
		if !d.isNew && d.origin == enhance.OriginUser && a.btn(c, p, "styles.editor.delete", i18n.T("styles.editor.delete"), btnDanger) {
			a.resetStyle(d.id)
			a.cfg.AI.StyleRules.RemoveStyle(d.id)
			a.cfg.StyleHotkeys.Set(d.id, nil)
			a.saveConfig()
		}
	}
}

func (a *App) resetStyle(id string) {
	if dir := a.stylesDir(); dir != "" {
		if err := enhance.ResetStyle(dir, id); err != nil {
			a.sty.err = err.Error()
			return
		}
	}
	a.reloadStyles()
	a.sty.close()
	a.invalidate()
}

// inkPicker is the six inks as seals, and a custom #hex.
func (a *App) inkPicker(c *mu.Context, p theme.Colors, key string, hex *string) {
	custom := true
	// A plain Graphite text button, as Rust's "Custom #6B4A6E".
	customBtn := func(label string) bool {
		b := mu.ButtonBase(a.k(c, key+".custom_show")).Label(label).PaddingX(4).Cursor(mu.CursorPointer)
		col := p.Graphite
		if b.Hovered() {
			col = p.Ink
		}
		b.Children(func() { uiText(c, label, 13, 500, col).SingleLine() })
		return clicked(c, b)
	}
	mu.Row(c).Gap(14).AlignItems(mu.Center).Wrap().Children(func() {
		for _, ink := range inkPresets {
			col := ink.Color()
			on := strings.EqualFold(*hex, col.Hex())
			custom = custom && !on
			b := mu.ButtonBase(a.k(c, key+"."+string(ink))).Label(i18n.T("core.ink."+string(ink))).Role(mu.RoleRadio).Checked(on).
				Size(26, 26).Radius(99).Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
			if on {
				b.Border(2, p.Ink)
			}
			b.Children(func() { sealDot(c, p, col.Hex(), 22) })
			if clicked(c, b) {
				*hex = col.Hex()
				a.sty.customInk = false
			}
		}
		if custom && *hex != "" && !a.sty.customInk {
			if customBtn(i18n.T("styles.editor.ink_custom", "ink", strings.ToUpper(*hex))) {
				a.sty.customInk = true
			}
		} else if a.sty.customInk {
			a.fieldW(c, p, key+".custom", i18n.T("settings.appearance.custom_ink"), "#2D3C8C", hex, 110)
		} else if customBtn(i18n.T("settings.appearance.custom")) {
			a.sty.customInk = true
		}
	})
}

func (a *App) providerPicker(c *mu.Context, p theme.Colors, d *styleDraft) {
	def := i18n.T("styles.editor.default_provider")
	if pr, ok := a.defaultProvider(); ok {
		def = i18n.T("styles.editor.default_provider_named", "provider", pr.Name)
	}
	options := []string{def}
	ids := []string{""}
	for _, pr := range a.cfg.AI.Providers {
		options, ids = append(options, pr.Name), append(ids, pr.ID)
	}
	if len(a.cfg.AI.Providers) == 0 {
		uiText(c, i18n.T("styles.editor.no_providers"), 13, 400, p.Graphite)
	}
	sel := options[0]
	for i, id := range ids {
		if id == d.provider {
			sel = options[i]
		}
	}
	a.selectBtn(c, p, "styles.editor.provider", i18n.T("styles.editor.provider"), &sel, options, 0).AlignSelf(mu.Stretch).OnChange(func() {
		for i, o := range options {
			if o == sel {
				d.provider = ids[i]
			}
		}
	})
}

func (a *App) cleanupControls(c *mu.Context, p theme.Colors, d *styleDraft) {
	a.toggleRow(c, p, "styles.editor.cleanup", i18n.T("styles.builtin.clean.name"), i18n.T("styles.editor.cleanup_hint"), &d.cleanup, nil)
	if !d.cleanup {
		return
	}
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		uiText(c, i18n.T("styles.editor.tone"), 13, 600, p.Ink).Width(70)
		if i := a.choice(c, p, []string{"styles.editor.tone.formal", "styles.editor.tone.casual", "styles.editor.tone.very_casual"},
			[]string{i18n.T("styles.editor.tone_formal"), i18n.T("styles.editor.tone_casual"), i18n.T("styles.editor.tone_very_casual")}, d.tone, false); i >= 0 {
			d.tone = i
		}
	})
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		uiText(c, i18n.T("styles.editor.layout"), 13, 600, p.Ink).Width(70)
		if i := a.choice(c, p, []string{"styles.editor.layout.paragraphs", "styles.editor.layout.lists"},
			[]string{i18n.T("styles.editor.layout_paragraphs"), i18n.T("styles.editor.layout_lists")}, d.layout, false); i >= 0 {
			d.layout = i
		}
	})
	a.toggleRow(c, p, "styles.editor.email", i18n.T("styles.editor.format_email"), "", &d.email, nil)
}

// targetsEditor is "Use automatically": apps and sites for this style.
func (a *App) targetsEditor(c *mu.Context, p theme.Colors, d *styleDraft) {
	mu.Column(c).Gap(8).Role(mu.RoleGroup).Label(i18n.T("styles.a11y.choices")).Children(func() {
		uiText(c, i18n.T("styles.targets.title"), 13, 600, p.Ink)
		uiText(c, i18n.T("styles.targets.intro_sites", "style", d.name), 12, 400, p.Graphite)
		if !a.cfg.AI.StyleRules.Enabled {
			uiText(c, i18n.T("styles.targets.off"), 12, 400, p.Graphite)
		}
		mu.Row(c).Gap(6).Wrap().Children(func() {
			if len(d.apps)+len(d.sites) == 0 {
				uiText(c, i18n.T("styles.targets.summary_none"), 13, 400, p.Graphite)
			}
			for i, t := range append(append([]string(nil), d.apps...), d.sites...) {
				debossed(mu.Row(c).Height(28).Gap(2).Padding(0, 2, 0, 11).Radius(99).AlignItems(mu.Center), p).Children(func() {
					uiText(c, t, 13, 500, p.Ink)
					if a.iconBtnSized(c, p, fmt.Sprintf("styles.target.remove.%d", i), "close", i18n.T("styles.a11y.remove_app", "name", t), 22, 10) {
						d.apps, d.sites = removeString(d.apps, t), removeString(d.sites, t)
					}
				})
			}
		})
		mu.Row(c).Gap(6).Children(func() {
			in := a.fieldW(c, p, "styles.target.new", i18n.T("styles.targets.add_site"), i18n.T("styles.targets.placeholder_site_bundle"), &d.newTarget, -1)
			add := func() {
				t := strings.TrimSpace(d.newTarget)
				if t == "" {
					return
				}
				if strings.Contains(t, ".") && !strings.Contains(t, " ") && (strings.Contains(t, "/") || strings.Count(t, ".") == 1) {
					d.sites = append(d.sites, t)
				} else {
					d.apps = append(d.apps, t)
				}
				d.newTarget = ""
			}
			in.OnSubmit(add)
			if a.button(c, p, "styles.target.add", i18n.T("dictionary.add"), bopt{icon: "plus"}) {
				add()
			}
		})
		uiText(c, i18n.T("styles.targets.site_hint"), 12, 400, p.Graphite)
	})
}

func removeString(list []string, s string) []string {
	out := list[:0:0]
	for _, x := range list {
		if x != s {
			out = append(out, x)
		}
	}
	return out
}

func (a *App) baseEditor(c *mu.Context, p theme.Colors) {
	mu.Row(c).AlignItems(mu.Start).Children(func() {
		heading(c, p, i18n.T("styles.base.title"), 28).Grow(1).Role(mu.RoleHeading)
		if a.iconBtnSized(c, p, "styles.base.close", "close", i18n.T("styles.a11y.close"), 30, 16) {
			a.sty.close()
		}
	})
	note := i18n.T("styles.base.default_note")
	if a.styles.UserBasePrompt != nil {
		note = i18n.T("styles.base.changed_note", "file", enhance.BasePromptFile)
	}
	uiText(c, note, 14, 400, p.Graphite).LineHeight(1.4)
	a.labelled(c, p, i18n.T("styles.base.rules"), func() {
		a.textArea(c, p, "styles.base.text", i18n.T("styles.base.rules"), i18n.T("styles.base.placeholder"), &a.sty.base, 380)
		uiText(c, i18n.T("styles.base.rules_hint"), 12, 400, p.Graphite).LineHeight(1.4)
	})
	if a.sty.err != "" {
		uiText(c, a.sty.err, 13, 500, p.Danger)
	}
}

func (a *App) baseEditorActions(c *mu.Context, p theme.Colors) {
	{
		if a.btn(c, p, "styles.base.save", i18n.T("styles.save"), btnPrimary) {
			if dir := a.stylesDir(); dir != "" {
				if err := enhance.SaveBasePrompt(dir, a.sty.base); err != nil {
					a.sty.err = i18n.T("styles.error.base_save_failed", "error", err.Error())
					return
				}
			}
			a.reloadStyles()
			a.sty.close()
		}
		if a.btn(c, p, "styles.base.cancel", i18n.T("styles.cancel"), btnGhost) {
			a.sty.close()
		}
		mu.Spacer(c)
		if a.styles.UserBasePrompt != nil && a.btn(c, p, "styles.base.reset", i18n.T("styles.base.reset"), btnGhost) {
			if dir := a.stylesDir(); dir != "" {
				enhance.ResetBasePrompt(dir)
			}
			a.reloadStyles()
			a.sty.close()
		}
	}
}

// appNameFor guesses an app's name from its bundle id or executable, as
// settings/ext.rs app_name_for: "com.microsoft.Outlook" is "Outlook",
// "slack.exe" is "Slack".
func appNameFor(bundleID string) string {
	last := bundleID
	if n := len(bundleID); n > 4 && strings.EqualFold(bundleID[n-4:], ".exe") {
		last = bundleID[:n-4]
	} else if i := strings.LastIndexByte(bundleID, '.'); i >= 0 {
		last = bundleID[i+1:]
	}
	if last == "" {
		return bundleID
	}
	r, size := utf8.DecodeRuneInString(last)
	return string(unicode.ToUpper(r)) + last[size:]
}
