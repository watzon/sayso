package ui

import (
	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The kit: the controls every page shares, in the paper-and-ink look of
// crates/sayso-ui/src/components/controls.rs. Each control that takes
// input gets its key from App.k and a label, so screen readers, the tests
// and mygo-automation find it.

type btnKind int

const (
	btnSecondary btnKind = iota // a raised slip
	btnPrimary                  // ink filled
	btnGhost                    // text only
	btnDanger                   // a raised slip in danger red
)

type btnSize int

const (
	sizeMedium btnSize = iota // h32
	sizeSmall                 // h28
	sizeLarge                 // h38
)

// bopt are a button's options beyond its kind.
type bopt struct {
	kind     btnKind
	size     btnSize
	icon     string // leading icon
	trail    string // trailing icon
	grow     bool
	disabled bool
}

// btn is a medium text button; it reports a click.
func (a *App) btn(c *mu.Context, p theme.Colors, key, label string, kind btnKind) bool {
	return a.button(c, p, key, label, bopt{kind: kind})
}

// button is a text button with o's size, icons and kind.
func (a *App) button(c *mu.Context, p theme.Colors, key, label string, o bopt) bool {
	h, px, fs, r := float32(32), float32(14), float32(13), float32(8)
	switch o.size {
	case sizeSmall:
		h, px = 28, 12
	case sizeLarge:
		h, px, fs, r = 38, 18, 14, 10
	}
	b := mu.ButtonBase(a.k(c, key)).Label(label).Height(h).PaddingX(px).Gap(8).Radius(r).Shrink(0).
		Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer).Disabled(o.disabled)
	if o.grow {
		b.Grow(1)
	}
	fg, weight := p.Ink, 500
	switch o.kind {
	case btnPrimary:
		fg, weight = p.OnInk, 600
		inkButton(b, p, b.Pressed())
		if b.Hovered() && !b.Pressed() {
			b.Background(p.InkFill.Alpha(0.92))
		}
	case btnSecondary, btnDanger:
		if o.kind == btnDanger {
			fg = p.Danger
		}
		if b.Pressed() {
			debossed(b, p)
		} else {
			raisedSmall(b, p)
			if b.Hovered() {
				b.Background(p.Sheet)
			}
		}
	case btnGhost:
		fg = p.Graphite
		switch {
		case b.Pressed():
			b.Background(p.Deboss)
		case b.Hovered():
			b.Background(p.Deboss.Alpha(0.6))
		}
	}
	if o.disabled {
		b.Opacity(0.45)
	}
	b.Children(func() {
		if o.icon != "" {
			mu.Icon(c, dirIcon(c, o.icon)).FontSize(14).TextColor(fg)
		}
		uiText(c, label, fs, weight, fg).SingleLine()
		if o.trail != "" {
			mu.Icon(c, dirIcon(c, o.trail)).FontSize(14).TextColor(fg)
		}
	})
	return clicked(c, b)
}

// link is an accent text action ("Open History", "Copy").
func (a *App) link(c *mu.Context, p theme.Colors, key, label string, size float32, weight int, trail string) bool {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Gap(4).AlignItems(mu.Center).Shrink(0).Cursor(mu.CursorPointer)
	if b.Hovered() {
		b.Opacity(0.8)
	}
	b.Children(func() {
		uiText(c, label, size, weight, p.Accent).SingleLine()
		if trail != "" {
			mu.Icon(c, dirIcon(c, trail)).FontSize(12).TextColor(p.Accent)
		}
	})
	return clicked(c, b)
}

// iconBtn is a 30 px square button with an 18 px icon; label names it.
func (a *App) iconBtn(c *mu.Context, p theme.Colors, key, iconName, label string) bool {
	return a.iconBtnSized(c, p, key, iconName, label, 30, 18)
}

func (a *App) iconBtnSized(c *mu.Context, p theme.Colors, key, iconName, label string, box, size float32) bool {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Tooltip(label).Size(box, box).Shrink(0).Radius(8).
		Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	if b.Hovered() {
		b.Background(p.Deboss.Alpha(0.55))
	}
	b.Children(func() { mu.Icon(c, dirIcon(c, iconName)).FontSize(size).TextColor(p.Graphite) })
	return clicked(c, b)
}

// segOpt are a segmented control's options.
type segOpt struct {
	fill    bool     // segments share the width
	compact bool     // the sidebar's: h28, 12 px
	icons   []string // a 14 px icon before each label
}

// choice is a segmented control: a debossed track, the chosen segment a
// raised slip. keys name each option. It returns the option clicked, or -1.
func (a *App) choice(c *mu.Context, p theme.Colors, keys, labels []string, chosen int, fill bool) int {
	return a.segmented(c, p, keys, labels, chosen, segOpt{fill: fill})
}

func (a *App) segmented(c *mu.Context, p theme.Colors, keys, labels []string, chosen int, o segOpt) int {
	picked := -1
	h, px, fs := float32(30), float32(14), float32(13)
	if o.compact {
		h, px, fs = 28, 4, 12
	}
	track := debossed(mu.Row(c).Padding(3).Gap(2).Radius(10).Shrink(0), p).Role(mu.RoleRadioGroup)
	if o.fill {
		track.Grow(1)
	}
	track.Children(func() {
		for i, l := range labels {
			s := mu.ButtonBase(a.k(c, keys[i])).Label(l).Role(mu.RoleRadio).Checked(i == chosen).
				Height(h).PaddingX(px).Gap(6).Radius(7).Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
			if o.fill || o.compact {
				s.Grow(1).Basis(0)
			}
			fg, weight := p.Graphite, 500
			if i == chosen {
				fg, weight = p.Ink, 600
				s.Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).Shadow(0, 1, 2, 0, p.ShadowA(0.18))
			} else if s.Hovered() {
				fg = p.Ink
			}
			s.Children(func() {
				if i < len(o.icons) {
					mu.Icon(c, dirIcon(c, o.icons[i])).FontSize(14).TextColor(fg)
				}
				uiText(c, l, fs, weight, fg).SingleLine()
			})
			if clicked(c, s) {
				picked = i
			}
		}
	})
	return picked
}

// toggle is the paper switch bound to v, named label; changed runs after
// a change.
func (a *App) toggle(c *mu.Context, key, label string, v *bool, changed func()) {
	p := a.colors(c)
	sw := mu.SwitchBase(a.k(c, key), v).Label(label).Size(46, 26).Shrink(0).Padding(3).Radius(99).Cursor(mu.CursorPointer)
	on := *v
	if on {
		sw.Background(p.InkFill).InsetShadow(0, 1, 3, 0, black.Alpha(0.45))
	} else {
		sw.Background(p.Deboss).InsetShadow(0, 1, 3, 0, p.ShadowA(0.3)).InsetShadow(0, -1, 0, 0, p.Highlight(0.7))
	}
	target := float32(0)
	if on {
		target = 20
	}
	x := sw.Animate("knob", target, 160_000_000)
	sw.Children(func() {
		knob := mu.Box(c).Size(20, 20).Radius(99).MarginStart(x).Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1))
		if on {
			knob.Shadow(0, 1, 2, 0, p.ShadowA(0.35))
			if p.Dark {
				knob.Background(p.OnInk)
			}
		} else {
			knob.Shadow(0, 1, 2, 0, p.ShadowA(0.30))
		}
	})
	if changed != nil {
		sw.OnChange(changed)
	}
}

// row lays out a settings row: the title and detail on the left, the
// control on the right, a rule under it.
func (a *App) row(c *mu.Context, p theme.Colors, title, detail string, control func()) {
	mu.Row(c).Gap(24).PaddingY(14).AlignItems(mu.Center).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		mu.Column(c).Gap(2).Grow(1).Basis(0).Children(func() {
			uiText(c, title, 15, 600, p.Ink).LineHeight(18.0 / 15)
			if detail != "" {
				uiText(c, detail, 13, 400, p.Graphite).LineHeight(16.0 / 13)
			}
		})
		if control != nil {
			control()
		}
	})
}

// toggleRow is a row with a switch.
func (a *App) toggleRow(c *mu.Context, p theme.Colors, key, title, detail string, v *bool, changed func()) {
	a.row(c, p, title, detail, func() { a.toggle(c, key, title, v, changed) })
}

// group is a titled run of rows: a caps label, then rows split by rules.
// No card.
func (a *App) group(c *mu.Context, p theme.Colors, title string, fn func()) {
	mu.Column(c).PaddingY(0).Padding(8, 0, 0, 0).Children(func() {
		if title != "" {
			caps(c, p, title).Padding(0, 0, 4, 0)
		}
		mu.Column(c).Children(fn)
	})
}

// field is a single-line text input in a debossed well, bound to v. w > 0
// fixes its width, w < 0 lets it grow, 0 stretches it.
func (a *App) field(c *mu.Context, p theme.Colors, key, label, placeholder string, v *string) mu.Element {
	return a.fieldW(c, p, key, label, placeholder, v, 0)
}

func (a *App) fieldW(c *mu.Context, p theme.Colors, key, label, placeholder string, v *string, w float32) mu.Element {
	in := mu.TextInputBase(a.k(c, key), v).Label(label).Placeholder(placeholder).PlaceholderColor(p.Pencil).
		Height(36).PaddingX(12).Radius(9).FontSize(14).TextColor(p.Ink)
	debossed(in, p)
	switch {
	case w > 0:
		in.Width(w).Shrink(0)
	case w < 0:
		in.Grow(1).Basis(0)
	}
	return in
}

// textArea is a debossed multi-line well.
func (a *App) textArea(c *mu.Context, p theme.Colors, key, label, placeholder string, v *string, h float32) mu.Element {
	in := mu.TextAreaBase(a.k(c, key), v).Label(label).Placeholder(placeholder).PlaceholderColor(p.Pencil).
		Height(h).Padding(10, 12).Radius(10).FontSize(14).TextColor(p.Ink).LineHeight(1.4)
	return debossed(in, p)
}

// searchWell is a debossed search field with its icon and an optional
// trailing mini key.
func (a *App) searchWell(c *mu.Context, p theme.Colors, key, placeholder string, v *string, h, w float32, mini string) mu.Element {
	var in mu.Element
	box := debossed(mu.Row(c).Height(h).PaddingX(12).Gap(10).Radius(10).AlignItems(mu.Center), p)
	if w > 0 {
		box.Width(w).Shrink(0)
	}
	box.Children(func() {
		mu.Icon(c, icon("search")).FontSize(14).TextColor(p.Graphite)
		in = mu.TextInputBase(a.k(c, key), v).Label(placeholder).Placeholder(placeholder).PlaceholderColor(p.Graphite).
			Grow(1).Basis(0).FontSize(14).TextColor(p.Ink).Role(mu.RoleSearch)
		if mini != "" {
			miniKey(c, p, mini, p.Graphite)
		}
	})
	return in
}

// selectBtn is a drop-down drawn as a secondary button with a chevron;
// its menu is a floating list. It returns the trigger, for OnChange.
func (a *App) selectBtn(c *mu.Context, p theme.Colors, key, label string, sel *string, options []string, w float32) mu.Element {
	s := mu.SelectBase(a.k(c, key), sel)
	t := s.Trigger.Label(label).Height(32).PaddingX(14).Gap(8).Radius(8).Shrink(0).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	if w > 0 {
		t.MinWidth(w)
	}
	if t.Pressed() || s.Open() {
		debossed(t, p)
	} else {
		raisedSmall(t, p)
		if t.Hovered() {
			t.Background(p.Sheet)
		}
	}
	t.Children(func() {
		uiText(c, *sel, 13, 500, p.Ink).SingleLine().Grow(1)
		mu.Icon(c, icon("chevron-down")).FontSize(14).TextColor(p.Ink)
	})
	s.Popup(func(panel mu.Element) {
		// The highlighted option scrolls into view (SelectBase), and the
		// popup shows above the trigger where there is no room below.
		list := a.floatingList(c, p, panel.Margin(4, 0, 0, 0).Padding(4).Radius(10).MinWidth(200), key+".list", 312)
		list.Children(func() {
			for _, o := range options {
				it := menuLine(s.Item(o).Height(30).PaddingX(10).Gap(8).Radius(7))
				if it.Highlighted() {
					it.Background(p.Deboss)
				}
				it.Children(func() {
					mu.Box(c).Width(14).Shrink(0).Children(func() {
						if o == *sel {
							mu.Icon(c, icon("check")).FontSize(12).TextColor(p.Ink)
						}
					})
					uiText(c, o, 13, 500, p.Ink).SingleLine()
				})
			}
		})
	})
	return t
}

// chip is a filter chip: a raised pill, or an ink one while on.
func (a *App) chip(c *mu.Context, p theme.Colors, key, label string, on bool) bool {
	return a.chipT(c, p, key, label, on, "")
}

func (a *App) chipT(c *mu.Context, p theme.Colors, key, label string, on bool, trail string) bool {
	return clicked(c, a.chipBase(c, p, key, label, on, trail, mu.RoleToggleButton))
}

// chipBase is a chip with its role (a toggle, or a radio of a group).
func (a *App) chipBase(c *mu.Context, p theme.Colors, key, label string, on bool, trail string, role mu.Role) mu.Element {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Role(role).Checked(on).
		Height(28).PaddingX(11).Gap(6).Radius(99).AlignItems(mu.Center).Shrink(0).Cursor(mu.CursorPointer)
	fg, weight := p.Graphite, 500
	if on {
		fg, weight = p.OnInk, 600
		b.Background(p.InkFill).InsetShadow(0, -1.5, 2, 0, black.Alpha(0.35))
	} else {
		raisedSmall(b, p)
		if b.Hovered() {
			fg = p.Ink
		}
	}
	b.Children(func() {
		uiText(c, label, 13, weight, fg).SingleLine()
		if trail != "" {
			mu.Icon(c, dirIcon(c, trail)).FontSize(10).TextColor(fg)
		}
	})
	return b
}

// pageHead is a page's title (display 44), intro and actions at the right,
// bottom-aligned.
func pageHead(c *mu.Context, p theme.Colors, title, intro string, introW float32, actions func()) {
	mu.Row(c).AlignItems(mu.End).GapX(24).GapY(16).Wrap().Children(func() {
		mu.Column(c).Gap(6).Grow(1).Basis(340).Children(func() {
			display(c, p, title, 44).Role(mu.RoleHeading)
			if intro != "" {
				uiText(c, intro, 15, 400, p.Graphite).LineHeight(22.0 / 15).MaxWidth(introW)
			}
		})
		if actions != nil {
			mu.Row(c).Gap(8).AlignItems(mu.Center).Children(actions)
		}
	})
}

// pageHeader is a title and intro with no actions.
func pageHeader(c *mu.Context, p theme.Colors, title, intro string) {
	pageHead(c, p, title, intro, 560, nil)
}

func rule(c *mu.Context, p theme.Colors) {
	mu.Box(c).Height(1).Shrink(0).Background(p.Rule)
}

// card is a raised sheet with no border.
func card(c *mu.Context, p theme.Colors) mu.Element {
	return raised(mu.Column(c).Padding(18, 20).Gap(12).Radius(12), p)
}

// menuLine lays out a line of a menu, a select or a pick list: its content
// from the leading edge, centered across. ButtonBase centers its children,
// which in a list as wide as its longest line puts a short label in the
// middle. The line keeps its height in a list that scrolls.
func menuLine(e mu.Element) mu.Element {
	return e.AlignItems(mu.Center).Justify(mu.Start).Shrink(0).Cursor(mu.CursorPointer)
}

// menuItemBase is a 30 px line of a menu or a pick list (Rust kit.rs
// menu_items).
func (a *App) menuItemBase(c *mu.Context, key, label string) mu.Element {
	return menuLine(mu.ButtonBase(a.k(c, key)).Label(label).Height(30).PaddingX(10).Gap(8).Radius(7))
}

// floatingList gives panel, a menu's, a select's or a pick list's, the
// floating look and clips it, and returns the list for its lines, which
// scrolls past maxH: without it, a long list draws past the panel's
// bottom over the page. Give the list its lines with Children.
func (a *App) floatingList(c *mu.Context, p theme.Colors, panel mu.Element, key string, maxH float32) mu.Element {
	floating(panel.Clip(), p)
	return mu.Scroll(a.k(c, key)).MaxHeight(maxH)
}

// clicked reports a click on el and asks for another frame, so that what
// the click changed shows at once in the elements built before it.
func clicked(c *mu.Context, el mu.Element) bool {
	if el.Clicked() {
		c.Invalidate()
		return true
	}
	return false
}

// radioDot is controls.rs radio_dot: ink with a paper center when chosen,
// debossed otherwise.
func radioDot(c *mu.Context, p theme.Colors, on bool) {
	d := mu.Box(c).Size(18, 18).Shrink(0).Radius(99)
	if !on {
		d.Background(p.Deboss).InsetShadow(0, 1, 2, 0, p.ShadowA(0.3))
		return
	}
	d.Background(p.InkFill).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
		mu.Box(c).Size(6, 6).Radius(99).Background(p.OnInk)
	})
}

// stepper is controls.rs Stepper: − value + in a debossed well. It returns
// -1 or +1 when a step is pressed, else 0.
func (a *App) stepper(c *mu.Context, p theme.Colors, key, label, value string) int {
	delta := 0
	debossed(mu.Row(a.k(c, key)).Role(mu.RoleGroup).Label(label).Height(32).PaddingX(3).Gap(2).Radius(9).
		Shrink(0).AlignItems(mu.Center), p).Children(func() {
		step := func(k, glyph, name string, d int) {
			b := mu.ButtonBase(a.k(c, key+"."+k)).Label(name).Size(26, 26).Radius(6).Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
			col := p.Graphite
			if b.Hovered() {
				b.Background(p.SheetRaised)
				col = p.Ink
			}
			b.Children(func() { uiText(c, glyph, 15, 400, col) })
			if clicked(c, b) {
				delta = d
			}
		}
		step("dec", "−", i18n.T("a11y.decrease"), -1)
		mu.Row(c).MinWidth(40).PaddingX(4).Justify(mu.Center).Children(func() {
			mu.Text(c, value).Font(theme.Mono).FontSize(13).FontWeight(500).TextColor(p.Ink).SingleLine()
		})
		step("inc", "+", i18n.T("a11y.increase"), 1)
	})
	return delta
}
