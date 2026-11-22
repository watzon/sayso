package ui

import (
	"strings"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/theme"
)

// The kit: the few controls every page shares, in the paper-and-ink look.
// Each control that takes input gets its key from App.k and a label, so
// screen readers, the tests and mygo-automation find it.

type btnKind int

const (
	btnSecondary btnKind = iota // a raised slip
	btnPrimary                  // ink filled
	btnGhost                    // text only
	btnDanger
)

// btn is a text button; it reports a click.
func (a *App) btn(c *mu.Context, p theme.Colors, key, label string, kind btnKind) bool {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Padding(7, 14).Radius(8).Cursor(mu.CursorPointer)
	fg := p.Ink
	switch kind {
	case btnPrimary:
		fg = p.OnInk
		b.Background(p.InkFill)
		if b.Hovered() {
			b.Opacity(0.9)
		}
	case btnSecondary:
		b.Background(p.SheetRaised).Border(1, p.Rule).Shadow(0, 1, 1, 0, p.Shadow.Alpha(0.06))
		if b.Hovered() {
			b.Background(p.Sheet)
		}
	case btnGhost:
		fg = p.Graphite
		if b.Hovered() {
			b.Background(p.Deboss)
		}
	case btnDanger:
		fg = p.Danger
		b.Border(1, p.Danger.Alpha(0.4))
		if b.Hovered() {
			b.Background(p.DangerWash)
		}
	}
	b.Children(func() {
		mu.Text(c, label).FontSize(13).FontWeight(500).TextColor(fg).SingleLine()
	})
	return clicked(c, b)
}

// iconBtn is a square button with an icon; label names it.
func (a *App) iconBtn(c *mu.Context, p theme.Colors, key, iconName, label string) bool {
	b := mu.ButtonBase(a.k(c, key)).Label(label).Tooltip(label).Size(30, 30).Radius(8).
		Justify(mu.Center).AlignItems(mu.Center).Cursor(mu.CursorPointer)
	if b.Hovered() {
		b.Background(p.Deboss)
	}
	b.Children(func() { mu.Icon(c, icon(iconName)).FontSize(16).TextColor(p.Graphite) })
	return clicked(c, b)
}

// choice is a row of options on a debossed track, the chosen one a raised
// slip. keys name each option. It returns the option clicked, or -1.
func (a *App) choice(c *mu.Context, p theme.Colors, keys, labels []string, chosen int, fill bool) int {
	picked := -1
	track := mu.Row(c).Padding(3).Gap(2).Radius(9).Background(p.Deboss).
		InsetShadow(0, 1, 2, 0, p.DebossShade.Alpha(0.6)).Role(mu.RoleRadioGroup)
	track.Children(func() {
		for i, l := range labels {
			s := mu.ButtonBase(a.k(c, keys[i])).Label(l).Role(mu.RoleRadio).Checked(i == chosen).
				Padding(5, 12).Radius(7).Justify(mu.Center).Cursor(mu.CursorPointer)
			if fill {
				s.Grow(1).Basis(0)
			}
			fg := p.Graphite
			if i == chosen {
				fg = p.Ink
				s.Background(p.SheetRaised).Shadow(0, 1, 2, 0, p.Shadow.Alpha(0.12))
			}
			s.Children(func() { mu.Text(c, l).FontSize(13).FontWeight(500).TextColor(fg).SingleLine() })
			if clicked(c, s) {
				picked = i
			}
		}
	})
	return picked
}

// toggle is a switch bound to v, named label; changed runs after a change.
func (a *App) toggle(c *mu.Context, key, label string, v *bool, changed func()) {
	sw := mu.Switch(a.k(c, key), v).Label(label)
	if changed != nil {
		sw.OnChange(changed)
	}
}

// row lays out a settings row: the title and detail on the left, the
// control on the right.
func (a *App) row(c *mu.Context, p theme.Colors, title, detail string, control func()) {
	mu.Row(c).Gap(24).Padding(12, 0).AlignItems(mu.Center).Children(func() {
		mu.Column(c).Gap(3).Grow(1).Basis(0).Children(func() {
			mu.Text(c, title).FontSize(14).FontWeight(500).TextColor(p.Ink)
			if detail != "" {
				mu.Text(c, detail).FontSize(12.5).TextColor(p.Graphite).LineHeight(1.35)
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

// group is a titled card of rows.
func (a *App) group(c *mu.Context, p theme.Colors, title string, fn func()) {
	mu.Column(c).Gap(6).Children(func() {
		if title != "" {
			eyebrow(c, p, title)
		}
		mu.Column(c).Padding(4, 18).Radius(12).Background(p.SheetRaised).Border(1, p.Rule).
			Shadow(0, 1, 2, 0, p.Shadow.Alpha(0.05)).Dividers(1, p.Rule).Children(fn)
	})
}

// field is a single-line text input bound to v.
func (a *App) field(c *mu.Context, p theme.Colors, key, label, placeholder string, v *string) mu.Element {
	return mu.TextInput(a.k(c, key), v).Label(label).Placeholder(placeholder)
}

// pageHeader is a page's eyebrow, title and intro.
func pageHeader(c *mu.Context, p theme.Colors, title, intro string) {
	mu.Column(c).Gap(8).Children(func() {
		heading(c, p, title, 34).Role(mu.RoleHeading)
		if intro != "" {
			mu.Text(c, intro).FontSize(14).TextColor(p.Graphite).MaxWidth(640).LineHeight(1.4)
		}
	})
}

func eyebrow(c *mu.Context, p theme.Colors, s string) {
	mu.Text(c, strings.ToUpper(s)).FontSize(11.5).FontWeight(500).LetterSpacing(1.1).TextColor(p.Graphite)
}

func heading(c *mu.Context, p theme.Colors, s string, size float32) mu.Element {
	return mu.Text(c, s).Font(theme.Display).FontSize(size).FontWeight(600).LetterSpacing(-0.02 * size).TextColor(p.Ink)
}

func rule(c *mu.Context, p theme.Colors) {
	mu.Box(c).Height(1).Background(p.Rule)
}

// keycap is one key of a hotkey.
func keycap(c *mu.Context, p theme.Colors, label string) {
	mu.Box(c).Padding(2, 7).Radius(5).Background(p.SheetRaised).Border(1, p.Rule).
		Shadow(0, 1, 0, 0, p.DebossShade).Children(func() {
		mu.Text(c, label).Font(theme.Mono).FontSize(12).TextColor(p.Ink)
	})
}

// slip is a small label on a wash.
func slip(c *mu.Context, p theme.Colors, label string, fg mu.Color) {
	mu.Box(c).Padding(2, 8).Radius(99).Background(fg.Alpha(0.12)).Children(func() {
		mu.Text(c, label).FontSize(11.5).FontWeight(500).TextColor(fg).SingleLine()
	})
}

// card is a raised sheet.
func card(c *mu.Context, p theme.Colors) mu.Element {
	return mu.Column(c).Padding(18, 20).Gap(12).Radius(12).Background(p.SheetRaised).Border(1, p.Rule).
		Shadow(0, 1, 2, 0, p.Shadow.Alpha(0.06))
}

// waveform draws levels as bars.
func waveform(c *mu.Context, levels []float32, col mu.Color) mu.Element {
	return mu.Box(c).Draw(func(pt *mu.Painter, r mu.Rect) {
		n := len(levels)
		if n == 0 {
			return
		}
		gap := float32(2)
		w := (r.W - gap*float32(n-1)) / float32(n)
		if w < 1 {
			w = 1
		}
		for i, l := range levels {
			v := l * 6
			if v > 1 {
				v = 1
			}
			h := r.H * (0.08 + 0.92*v)
			x := r.X + float32(i)*(w+gap)
			pt.Fill(mu.Rect{X: x, Y: r.Y + (r.H-h)/2, W: w, H: h}, col, w/2)
		}
	})
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
