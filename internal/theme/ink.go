package theme

import (
	"fmt"
	"math"
	"strconv"
	"strings"
)

// Rgb is an sRGB color as 0xRRGGBB. It ports crates/sayso-ink.
type Rgb uint32

// R, G and B are the channels.
func (c Rgb) R() uint8 { return uint8(c >> 16) }
func (c Rgb) G() uint8 { return uint8(c >> 8) }
func (c Rgb) B() uint8 { return uint8(c) }

// ParseRgb parses "#RRGGBB" (the # is optional).
func ParseRgb(s string) (Rgb, bool) {
	s = strings.TrimLeft(strings.TrimSpace(s), "#")
	if len(s) != 6 {
		return 0, false
	}
	for _, r := range s {
		if !strings.ContainsRune("0123456789abcdefABCDEF", r) {
			return 0, false
		}
	}
	v, err := strconv.ParseUint(s, 16, 32)
	if err != nil {
		return 0, false
	}
	return Rgb(v), true
}

// Hex is "#RRGGBB" in capitals.
func (c Rgb) Hex() string { return fmt.Sprintf("#%06X", uint32(c)) }

// Luminance is the WCAG relative luminance.
func (c Rgb) Luminance() float64 {
	ch := func(v uint8) float64 {
		f := float64(v) / 255.0
		if f <= 0.03928 {
			return f / 12.92
		}
		return math.Pow((f+0.055)/1.055, 2.4)
	}
	return 0.2126*ch(c.R()) + 0.7152*ch(c.G()) + 0.0722*ch(c.B())
}

// Contrast is the WCAG contrast ratio, 1 to 21.
func (c Rgb) Contrast(o Rgb) float64 {
	a, b := c.Luminance(), o.Luminance()
	hi, lo := a, b
	if b > a {
		hi, lo = b, a
	}
	return (hi + 0.05) / (lo + 0.05)
}

func (c Rgb) toHSL() (h, s, l float64) {
	r := float64(c.R()) / 255.0
	g := float64(c.G()) / 255.0
	b := float64(c.B()) / 255.0
	max := math.Max(r, math.Max(g, b))
	min := math.Min(r, math.Min(g, b))
	l = (max + min) / 2.0
	if math.Abs(max-min) < 2.220446049250313e-16 { // f64::EPSILON
		return 0, 0, l
	}
	d := max - min
	if l > 0.5 {
		s = d / (2.0 - max - min)
	} else {
		s = d / (max + min)
	}
	switch max {
	case r:
		add := 0.0
		if g < b {
			add = 6.0
		}
		h = ((g-b)/d + add) * 60.0
	case g:
		h = ((b-r)/d + 2.0) * 60.0
	default:
		h = ((r-g)/d + 4.0) * 60.0
	}
	return h, s, l
}

func rgbFromHSL(h, s, l float64) Rgb {
	h = (h - 360.0*math.Floor(h/360.0)) / 360.0 // rem_euclid(360) / 360
	hue := func(p, q, t float64) float64 {
		if t < 0 {
			t++
		}
		if t > 1 {
			t--
		}
		switch {
		case t < 1.0/6.0:
			return p + (q-p)*6.0*t
		case t < 0.5:
			return q
		case t < 2.0/3.0:
			return p + (q-p)*(2.0/3.0-t)*6.0
		}
		return p
	}
	var r, g, b float64
	if s == 0 {
		r, g, b = l, l, l
	} else {
		var q float64
		if l < 0.5 {
			q = l * (1.0 + s)
		} else {
			q = l + s - l*s
		}
		p := 2.0*l - q
		r, g, b = hue(p, q, h+1.0/3.0), hue(p, q, h), hue(p, q, h-1.0/3.0)
	}
	to := func(c float64) uint32 { return uint32(math.Round(math.Min(math.Max(c, 0), 1) * 255.0)) }
	return Rgb(to(r)<<16 | to(g)<<8 | to(b))
}

// Mix moves toward other: t = 0 keeps c, t = 1 gives other.
func (c Rgb) Mix(other Rgb, t float64) Rgb {
	m := func(a, b uint8) uint32 {
		return uint32(math.Round(float64(a) + (float64(b)-float64(a))*t))
	}
	return Rgb(m(c.R(), other.R())<<16 | m(c.G(), other.G())<<8 | m(c.B(), other.B()))
}

// InkPreset is a named ink, as stored in config.
type InkPreset string

// The named inks.
const (
	Sumi      InkPreset = "sumi"
	Indigo    InkPreset = "indigo"
	IronGall  InkPreset = "iron_gall"
	Sepia     InkPreset = "sepia"
	Verdigris InkPreset = "verdigris"
	Oxblood   InkPreset = "oxblood"
)

// AllInks lists the named inks in menu order.
var AllInks = []InkPreset{Sumi, Indigo, IronGall, Sepia, Verdigris, Oxblood}

// Name is the display name: "Iron Gall".
func (p InkPreset) Name() string {
	switch p {
	case Indigo:
		return "Indigo"
	case IronGall:
		return "Iron Gall"
	case Sepia:
		return "Sepia"
	case Verdigris:
		return "Verdigris"
	case Oxblood:
		return "Oxblood"
	}
	return "Sumi"
}

// Color is the ink color. An unknown preset is Sumi.
func (p InkPreset) Color() Rgb {
	switch p {
	case Indigo:
		return 0x2D3C8C
	case IronGall:
		return 0x2F3A4F
	case Sepia:
		return 0x6B4A2E
	case Verdigris:
		return 0x2F5E52
	case Oxblood:
		return 0x6E2A24
	}
	return 0x1D1B18
}

// Valid reports whether p is one of the named inks.
func (p InkPreset) Valid() bool {
	for _, k := range AllInks {
		if k == p {
			return true
		}
	}
	return false
}

// Ink is the ink setting: a named ink or a custom hex color.
type Ink struct {
	Preset InkPreset
	Custom Rgb
	// IsCustom selects Custom over Preset.
	IsCustom bool
}

// ParseInk parses the config value: "iron_gall" or "#123456".
func ParseInk(s string) (Ink, error) {
	if p := InkPreset(strings.TrimSpace(s)); p.Valid() {
		return Ink{Preset: p}, nil
	}
	if c, ok := ParseRgb(s); ok {
		return Ink{Custom: c, IsCustom: true}, nil
	}
	return Ink{}, fmt.Errorf("%q is neither a named ink nor a #RRGGBB color", s)
}

// String is the config value.
func (i Ink) String() string {
	if i.IsCustom {
		return i.Custom.Hex()
	}
	return string(i.Preset)
}

// Color is the ink color.
func (i Ink) Color() Rgb {
	if i.IsCustom {
		return i.Custom
	}
	return i.Preset.Color()
}

// inkFrom picks the ink for BuildMyGoTheme: a valid customHex wins.
func inkFrom(preset InkPreset, customHex string) Ink {
	if c, ok := ParseRgb(customHex); ok && strings.TrimSpace(customHex) != "" {
		return Ink{Custom: c, IsCustom: true}
	}
	if !preset.Valid() {
		preset = Sumi
	}
	return Ink{Preset: preset}
}

const (
	sumiAccentLight Rgb = 0x2D3C8C
	sumiAccentDark  Rgb = 0x8D9BE6
)

func derivedAccent(ink Rgb, dark bool) Rgb {
	h, s, _ := ink.toHSL()
	if s < 0.15 {
		// Near-black or gray inks have no hue to work from. Use indigo.
		if dark {
			return sumiAccentDark
		}
		return sumiAccentLight
	}
	if math.Abs(h-230.0) < 20.0 && s > 0.3 {
		// Indigo ink: the accent moves to a warm ochre so recording still stands out.
		if dark {
			return 0xE0B464
		}
		return 0x8A5A12
	}
	// Other hues: the complementary hue, at a fixed lightness per mode.
	if dark {
		return rgbFromHSL(h+180.0, 0.6, 0.72)
	}
	return rgbFromHSL(h+180.0, 0.48, 0.36)
}

// Palette is every color the UI uses; field names match the Paper design
// tokens.
type Palette struct {
	Dark bool
	// Ground is the window background and sidebar (the "board").
	Ground Rgb
	// Sheet is the raised content sheet.
	Sheet Rgb
	// SheetRaised is the top-most sheet: cards, popovers, the selected row.
	SheetRaised Rgb
	// Deboss is the inside of a pressed impression: fields, wells, tracks.
	Deboss      Rgb
	DebossShade Rgb
	Rule        Rgb
	// Ink is primary text and the waveform.
	Ink Rgb
	// InkFill fills primary buttons; text on it uses OnInk.
	InkFill    Rgb
	OnInk      Rgb
	Graphite   Rgb
	Pencil     Rgb
	Accent     Rgb
	AccentWash Rgb
	Danger     Rgb
	DangerWash Rgb
	Success    Rgb
	// Shadow is the color of drop shadows (used with alpha).
	Shadow Rgb
}

// NewPalette derives the palette of an ink on light or dark paper.
func NewPalette(ink Ink, dark bool) Palette {
	base := ink.Color()
	accent := derivedAccent(base, dark)
	if !dark {
		sheet := Rgb(0xF7F4EE)
		return Palette{
			Ground: 0xE9E4DA, Sheet: sheet, SheetRaised: 0xFCFAF6,
			Deboss: 0xE2DCD0, DebossShade: 0xCFC8BB, Rule: 0xD8D1C4,
			Ink: base, InkFill: base, OnInk: sheet,
			Graphite: 0x6A655C, Pencil: 0x8E887D,
			Accent: accent, AccentWash: accent.Mix(sheet, 0.84),
			Danger: 0x8E2F22, DangerWash: 0xF3E7DC, Success: 0x4E7A4A,
			Shadow: 0x3C2E1C,
		}
	}
	sheet := Rgb(0x201F1C)
	// Pale ink, tinted a little toward the chosen ink.
	h, _, _ := base.toHSL()
	pale := Rgb(0xECE6DA).Mix(rgbFromHSL(h, 0.4, 0.85), 0.12)
	return Palette{
		Dark:   true,
		Ground: 0x171614, Sheet: sheet, SheetRaised: 0x292825,
		Deboss: 0x141311, DebossShade: 0x0E0D0C, Rule: 0x34322E,
		Ink: pale, InkFill: pale, OnInk: sheet,
		Graphite: 0xA39D91, Pencil: 0x7D776C,
		Accent: accent, AccentWash: accent.Mix(sheet, 0.82),
		Danger: 0xE08A7C, DangerWash: 0x3A2420, Success: 0x8FB98A,
		Shadow: 0x000000,
	}
}

// ContrastReport is where a custom ink fails contrast; Settings shows it as
// a warning.
type ContrastReport struct {
	LightRatio, DarkRatio float64
}

// Passes is WCAG AA for body text in both modes.
func (r ContrastReport) Passes() bool { return r.LightRatio >= 4.5 && r.DarkRatio >= 4.5 }

// NewContrastReport checks an ink on both papers.
func NewContrastReport(ink Ink) ContrastReport {
	l, d := NewPalette(ink, false), NewPalette(ink, true)
	return ContrastReport{LightRatio: l.Ink.Contrast(l.Sheet), DarkRatio: d.Ink.Contrast(d.Sheet)}
}
