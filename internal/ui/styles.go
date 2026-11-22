package ui

import (
	"fmt"
	"strconv"
	"strings"

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
	mu.Row(c).AlignItems(mu.End).Gap(12).Children(func() {
		mu.Column(c).Grow(1).Children(func() {
			pageHeader(c, p, i18n.T("styles.title"), i18n.T("styles.intro", "dir", a.stylesDir()))
		})
		if a.btn(c, p, "styles.base_prompt", i18n.T("styles.base.button"), btnSecondary) {
			a.sty.baseOpen, a.sty.editing = true, false
			a.sty.base = a.styles.BasePrompt()
		}
		if a.btn(c, p, "styles.new", i18n.T("styles.new_style"), btnPrimary) {
			a.editStyle(nil)
		}
	})
	for _, e := range a.styles.Errors {
		mu.Text(c, i18n.T("styles.file_error", "error", e)).FontSize(12.5).TextColor(p.Danger)
	}
	a.providerLine(c, p)
	a.featuredStyle(c, p)
	active := a.ActiveStyle()
	mu.Column(c).Radius(12).Background(p.SheetRaised).Border(1, p.Rule).Dividers(1, p.Rule).Clip().
		Role(mu.RoleRadioGroup).Label(i18n.T("styles.a11y.default_style")).Children(func() {
		for i := range a.styles.Entries {
			a.styleRow(c, p, &a.styles.Entries[i], active)
		}
	})
}

func (a *App) providerLine(c *mu.Context, p theme.Colors) {
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		prov, ok := a.defaultProvider()
		var line string
		switch {
		case !a.cfg.AI.Enabled:
			line = i18n.T("styles.provider_line.ai_off")
		case !ok:
			line = i18n.T("styles.provider_line.no_provider")
		case prov.ModelName() == "":
			line = i18n.T("styles.provider_line.no_model", "provider", prov.Name)
		default:
			line = i18n.T("styles.provider_line.with_model", "provider", prov.Name, "model", prov.ModelName())
		}
		mu.Text(c, line).FontSize(13).TextColor(p.Graphite).Grow(1)
		label := i18n.T("styles.provider_line.manage")
		if !ok {
			label = i18n.T("styles.provider_line.set_up")
		}
		if a.btn(c, p, "styles.providers", label, btnGhost) {
			a.OpenSettings(SecProviders)
		}
	})
}

// featuredStyle shows what the default style does, before and after.
func (a *App) featuredStyle(c *mu.Context, p theme.Colors) {
	card(c, p).Children(func() {
		mu.Row(c).Gap(20).Children(func() {
			mu.Column(c).Gap(6).Grow(1).Basis(0).Children(func() {
				eyebrow(c, p, i18n.T("styles.featured.you_say"))
				mu.Text(c, i18n.T("styles.featured.said")).Font(theme.Body).FontSize(14).TextColor(p.Graphite).LineHeight(1.45)
			})
			mu.Column(c).Gap(6).Grow(1).Basis(0).Children(func() {
				eyebrow(c, p, i18n.T("styles.featured.sayso_writes"))
				mu.Text(c, i18n.T("styles.featured.written")).Font(theme.Body).FontSize(14).TextColor(p.Ink).LineHeight(1.45)
			})
		})
	})
}

func (a *App) styleRow(c *mu.Context, p theme.Colors, e *enhance.StyleEntry, active string) {
	st := &e.Style
	name := styleName(st)
	mu.Row(c).Gap(14).Padding(12, 16).AlignItems(mu.Center).Children(func() {
		r := mu.ButtonBase(a.k(c, "styles.default."+st.ID)).Role(mu.RoleRadio).Checked(st.ID == active).
			Label(i18n.T("styles.a11y.make_default", "name", name)).Size(20, 20).Radius(99).Border(1.5, p.Pencil).
			Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
		r.Children(func() {
			if st.ID == active {
				mu.Box(c).Size(10, 10).Radius(99).Background(p.InkFill)
			}
		})
		if clicked(c, r) {
			a.SetActiveStyle(st.ID)
		}
		seal(c, st.Ink, p)
		mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
				mu.Text(c, name).FontSize(14.5).FontWeight(600).TextColor(p.Ink)
				if st.ID == active {
					slip(c, p, i18n.T("styles.default"), p.Accent)
				}
				if st.ID == "clean" {
					slip(c, p, i18n.T("styles.recommended"), p.Success)
				}
			})
			mu.Text(c, styleDescription(st)).FontSize(12.5).TextColor(p.Graphite).SingleLine().Ellipsis("…")
			mu.Text(c, a.styleRoute(st)).FontSize(11.5).TextColor(p.Pencil)
		})
		if hk, ok := a.cfg.StyleHotkeys.Get(st.ID); ok {
			mu.Row(c).Gap(3).Children(func() {
				for _, k := range hk.Keycaps() {
					keycap(c, p, k)
				}
			})
		}
		if a.btn(c, p, "styles.edit."+st.ID, i18n.T("styles.edit"), btnGhost) {
			a.editStyle(e)
		}
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

// drawer is a 500px panel over the right side of the sheet.
// footer stays under the scrolling body, so Save is always in reach.
func (a *App) drawer(c *mu.Context, p theme.Colors, body, footer func()) {
	mu.Column(c).Absolute().Top(0).Right(0).Bottom(0).Width(500).ZIndex(5).Role(mu.RoleDialog).
		Background(p.SheetRaised).BorderStartWidth(1).BorderColor(p.Rule).Radius(0, 12, 12, 0).
		Shadow(-8, 0, 28, 0, p.Shadow.Alpha(0.16)).Children(func() {
		mu.Scroll(c.Key("drawer")).Grow(1).Padding(28, 28).Gap(16).Children(body)
		rule(c, p)
		mu.Row(c).Gap(8).Padding(14, 28).AlignItems(mu.Center).Children(footer)
	})
}

func (a *App) styleEditor(c *mu.Context, p theme.Colors) {
	d := &a.sty.draft
	title := i18n.T("styles.editor.new_title")
	if !d.isNew {
		title = i18n.T("styles.editor.edit_title", "name", d.name)
	}
	mu.Row(c).AlignItems(mu.Center).Children(func() {
		heading(c, p, title, 24).Grow(1)
		if a.iconBtn(c, p, "styles.editor.close", "close", i18n.T("styles.a11y.close")) {
			a.sty.close()
		}
	})
	switch d.origin {
	case enhance.OriginBuiltIn:
		if !d.isNew {
			mu.Text(c, i18n.T("styles.editor.built_in_note")).FontSize(12.5).TextColor(p.Graphite)
		}
	case enhance.OriginOverridden:
		mu.Text(c, i18n.T("styles.editor.overridden_note")).FontSize(12.5).TextColor(p.Graphite)
	}
	if a.sty.err != "" {
		mu.Text(c, a.sty.err).FontSize(13).TextColor(p.Danger).Role(mu.RoleAlert)
	}
	labelled := func(label string, fn func()) {
		mu.Column(c).Gap(6).Children(func() {
			mu.Text(c, label).FontSize(12.5).FontWeight(500).TextColor(p.Graphite)
			fn()
		})
	}
	labelled(i18n.T("styles.editor.name"), func() {
		a.field(c, p, "styles.editor.name", i18n.T("styles.editor.name"), i18n.T("styles.editor.name_placeholder"), &d.name)
	})
	labelled(i18n.T("styles.editor.description"), func() {
		a.field(c, p, "styles.editor.description", i18n.T("styles.editor.description"), i18n.T("styles.editor.description_placeholder"), &d.desc)
	})
	labelled(i18n.T("styles.editor.ink"), func() { a.inkPicker(c, p, "styles.editor.ink", &d.ink) })
	labelled(i18n.T("styles.editor.prompt"), func() {
		mu.TextArea(a.k(c, "styles.editor.prompt"), &d.prompt).Label(i18n.T("styles.editor.prompt")).
			Placeholder(i18n.T("styles.editor.prompt_placeholder")).Height(140)
		mu.Text(c, i18n.T("styles.editor.prompt_hint")).FontSize(12).TextColor(p.Pencil)
	})
	a.toggleRow(c, p, "styles.editor.use_base", i18n.T("styles.editor.use_base"), i18n.T("styles.editor.use_base_detail"), &d.useBase, nil)
	labelled(i18n.T("styles.editor.shortcut"), func() {
		a.recorder(c, p, "styles.editor.hotkey", i18n.T("styles.editor.shortcut"), d.hotkey, func(h *config.Hotkey) {
			d.hotkey, d.hotkeyDirt = h, true
		})
		mu.Text(c, i18n.T("styles.editor.shortcut_hint", "style", d.name)).FontSize(12).TextColor(p.Pencil)
	})
	labelled(i18n.T("styles.editor.provider"), func() { a.providerPicker(c, p, d) })
	labelled(i18n.T("styles.editor.timeout"), func() {
		secs := a.cfg.AI.HTTPTimeoutMs / 1000
		a.field(c, p, "styles.editor.timeout", i18n.T("styles.editor.timeout"), i18n.T("styles.editor.timeout_placeholder", "seconds", secs), &d.timeout).Width(140)
	})
	a.cleanupControls(c, p, d)
	a.targetsEditor(c, p, d)
	mu.Text(c, i18n.T("styles.editor.saved_in", "dir", a.stylesDir())).FontSize(12).TextColor(p.Pencil)
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
		if a.btn(c, p, "styles.editor.cancel", i18n.T("styles.cancel"), btnSecondary) {
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

// inkPicker is the six inks and a custom #hex.
func (a *App) inkPicker(c *mu.Context, p theme.Colors, key string, hex *string) {
	mu.Row(c).Gap(8).AlignItems(mu.Center).Wrap().Children(func() {
		for _, ink := range inkPresets {
			col := ink.Color()
			on := strings.EqualFold(*hex, col.Hex())
			b := mu.ButtonBase(a.k(c, key+"."+string(ink))).Label(i18n.T("core.ink."+string(ink))).Role(mu.RoleRadio).Checked(on).
				Size(26, 26).Radius(99).Background(theme.UIColor(col)).Cursor(mu.CursorPointer)
			if on {
				b.Border(2, p.Accent)
			}
			if clicked(c, b) {
				*hex = col.Hex()
			}
		}
		a.field(c, p, key+".custom", i18n.T("settings.appearance.custom_ink"), "#2D3C8C", hex).Width(110)
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
		mu.Text(c, i18n.T("styles.editor.no_providers")).FontSize(12.5).TextColor(p.Pencil)
	}
	sel := options[0]
	for i, id := range ids {
		if id == d.provider {
			sel = options[i]
		}
	}
	mu.Select(a.k(c, "styles.editor.provider"), &sel, options).Label(i18n.T("styles.editor.provider")).OnChange(func() {
		for i, o := range options {
			if o == sel {
				d.provider = ids[i]
			}
		}
	})
	a.field(c, p, "styles.editor.model", i18n.T("styles.editor.model"), i18n.T("styles.editor.provider_model"), &d.model)
}

func (a *App) cleanupControls(c *mu.Context, p theme.Colors, d *styleDraft) {
	a.toggleRow(c, p, "styles.editor.cleanup", i18n.T("styles.builtin.clean.name"), i18n.T("styles.editor.cleanup_hint"), &d.cleanup, nil)
	if !d.cleanup {
		return
	}
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		mu.Text(c, i18n.T("styles.editor.tone")).FontSize(12.5).TextColor(p.Graphite).Width(70)
		if i := a.choice(c, p, []string{"styles.editor.tone.formal", "styles.editor.tone.casual", "styles.editor.tone.very_casual"},
			[]string{i18n.T("styles.editor.tone_formal"), i18n.T("styles.editor.tone_casual"), i18n.T("styles.editor.tone_very_casual")}, d.tone, false); i >= 0 {
			d.tone = i
		}
	})
	mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
		mu.Text(c, i18n.T("styles.editor.layout")).FontSize(12.5).TextColor(p.Graphite).Width(70)
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
		mu.Text(c, i18n.T("styles.targets.title")).FontSize(12.5).FontWeight(500).TextColor(p.Graphite)
		mu.Text(c, i18n.T("styles.targets.intro_sites", "style", d.name)).FontSize(12).TextColor(p.Pencil)
		if !a.cfg.AI.StyleRules.Enabled {
			mu.Text(c, i18n.T("styles.targets.off")).FontSize(12).TextColor(p.Pencil)
		}
		mu.Row(c).Gap(6).Wrap().Children(func() {
			if len(d.apps)+len(d.sites) == 0 {
				mu.Text(c, i18n.T("styles.targets.summary_none")).FontSize(12.5).TextColor(p.Pencil)
			}
			for i, t := range append(append([]string(nil), d.apps...), d.sites...) {
				mu.Row(c).Gap(4).Padding(2, 4, 2, 10).Radius(99).Background(p.Deboss).AlignItems(mu.Center).Children(func() {
					mu.Text(c, t).FontSize(12.5).TextColor(p.Ink)
					if a.iconBtn(c, p, fmt.Sprintf("styles.target.remove.%d", i), "close", i18n.T("styles.a11y.remove_app", "name", t)) {
						d.apps, d.sites = removeString(d.apps, t), removeString(d.sites, t)
					}
				})
			}
		})
		mu.Row(c).Gap(6).Children(func() {
			in := a.field(c, p, "styles.target.new", i18n.T("styles.targets.add_site"), i18n.T("styles.targets.placeholder_site_bundle"), &d.newTarget).Grow(1)
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
			if a.btn(c, p, "styles.target.add", i18n.T("dictionary.add"), btnSecondary) {
				add()
			}
		})
		mu.Text(c, i18n.T("styles.targets.site_hint")).FontSize(12).TextColor(p.Pencil)
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
	mu.Row(c).AlignItems(mu.Center).Children(func() {
		heading(c, p, i18n.T("styles.base.title"), 24).Grow(1)
		if a.iconBtn(c, p, "styles.base.close", "close", i18n.T("styles.a11y.close")) {
			a.sty.close()
		}
	})
	note := i18n.T("styles.base.default_note")
	if a.styles.UserBasePrompt != nil {
		note = i18n.T("styles.base.changed_note", "file", enhance.BasePromptFile)
	}
	mu.Text(c, note).FontSize(12.5).TextColor(p.Graphite)
	mu.Text(c, i18n.T("styles.base.rules")).FontSize(12.5).FontWeight(500).TextColor(p.Graphite)
	mu.TextArea(a.k(c, "styles.base.text"), &a.sty.base).Label(i18n.T("styles.base.rules")).
		Placeholder(i18n.T("styles.base.placeholder")).Height(320)
	mu.Text(c, i18n.T("styles.base.rules_hint")).FontSize(12).TextColor(p.Pencil)
	if a.sty.err != "" {
		mu.Text(c, a.sty.err).FontSize(13).TextColor(p.Danger)
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
		if a.btn(c, p, "styles.base.cancel", i18n.T("styles.cancel"), btnSecondary) {
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
