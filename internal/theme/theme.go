// Package theme is Sayso's paper-and-ink palette and the MyGo widget theme
// made from it. The color math is in ink.go; the fonts,
// icons, provider logos, sound cues and paper textures are embedded.
package theme

import (
	"github.com/egoist/mygo/ui"
)

// Font families, as registered by RegisterFonts.
const (
	Display = "Fraunces"
	Body    = "Fraunces Text" // transcripts: the serif at text sizes
	UI      = "Instrument Sans"
	Mono    = "IBM Plex Mono"
)

// Colors is a Palette as MyGo colors.
type Colors struct {
	Dark                                   bool
	Ground, Sheet, SheetRaised             ui.Color
	Deboss, DebossShade, Rule              ui.Color
	Ink, InkFill, OnInk, Graphite, Pencil  ui.Color
	Accent, AccentWash, Danger, DangerWash ui.Color
	Success, Shadow                        ui.Color
}

// ShadowA is the paper's shadow color at alpha a. Dark paper needs deeper
// shadows to read, so a is tripled there, up to .7 (Rust paper.rs).
func (c Colors) ShadowA(a float32) ui.Color {
	if c.Dark {
		return c.Shadow.Alpha(min(a*3, 0.7))
	}
	return c.Shadow.Alpha(a)
}

// Highlight is the white catch-light on a paper edge at alpha a; on dark
// paper it is a faint .07 of that.
func (c Colors) Highlight(a float32) ui.Color {
	if c.Dark {
		return ui.RGBA(255, 255, 255, a*0.07)
	}
	return ui.RGBA(255, 255, 255, a)
}

// Over composites top at alpha a over the opaque base (the under-sheets).
func Over(base, top ui.Color, a float32) ui.Color { return base.Mix(top, a) }

// UIColor converts an Rgb to a MyGo color.
func UIColor(c Rgb) ui.Color { return ui.RGB(c.R(), c.G(), c.B()) }

// Colors converts the palette for MyGo components.
func (p Palette) Colors() Colors {
	u := UIColor
	return Colors{
		Dark:   p.Dark,
		Ground: u(p.Ground), Sheet: u(p.Sheet), SheetRaised: u(p.SheetRaised),
		Deboss: u(p.Deboss), DebossShade: u(p.DebossShade), Rule: u(p.Rule),
		Ink: u(p.Ink), InkFill: u(p.InkFill), OnInk: u(p.OnInk),
		Graphite: u(p.Graphite), Pencil: u(p.Pencil),
		Accent: u(p.Accent), AccentWash: u(p.AccentWash),
		Danger: u(p.Danger), DangerWash: u(p.DangerWash),
		Success: u(p.Success), Shadow: u(p.Shadow),
	}
}

// Theme is a built theme: the MyGo widget theme and the palette behind it.
type Theme struct {
	// UI is the widget theme to hand to MyGo.
	UI *ui.Theme
	// Palette has the exact sayso-ink values.
	Palette Palette
	// Colors is Palette as MyGo colors, for components that draw by hand.
	Colors Colors
}

// BuildMyGoTheme builds the theme of an ink on light or dark paper. A valid
// "#RRGGBB" customHex takes the place of the preset; an unknown preset is
// Sumi.
func BuildMyGoTheme(ink InkPreset, customHex string, dark bool) *Theme {
	p := NewPalette(inkFrom(ink, customHex), dark)
	return &Theme{UI: p.uiTheme(), Palette: p, Colors: p.Colors()}
}

// uiTheme is the MyGo widget theme for p: ink-filled primary controls and
// switches, accent focus rings, Instrument Sans.
func (p Palette) uiTheme() *ui.Theme {
	c := p.Colors()
	base := ui.LightTheme()
	if p.Dark {
		base = ui.DarkTheme()
	}
	t := *base
	t.Dark = p.Dark
	t.Background = c.Ground
	t.Surface, t.SurfaceHover, t.SurfacePressed = c.SheetRaised, c.Sheet, c.Deboss
	t.Border = c.Rule
	t.Text, t.TextMuted = c.Ink, c.Graphite
	t.Accent, t.AccentHover, t.AccentPressed, t.AccentText = c.InkFill, c.InkFill.Alpha(0.88), c.InkFill.Alpha(0.76), c.OnInk
	t.InactiveAccent = c.InkFill // the ink does not fade in an inactive window
	t.Danger, t.Success = c.Danger, c.Success
	t.Selection = c.AccentWash
	t.Focus = c.Accent
	t.Inverse, t.InverseText = c.InkFill, c.OnInk
	t.Scrollbar = c.DebossShade
	t.Font, t.FontSize = UI, 14
	t.Radius = 10
	return &t
}
