package ui

import (
	"bytes"
	"image/png"
	"log"
	"strings"
	"sync"
	"unicode"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The paper material of the Rust app: no
// surface has a border; depth comes from stacked shadows and a 1 px white
// catch-light on the top edge. Each helper sets the surface's fill and its
// stack, so a page reads as sheets, slips and wells pressed into paper.

// raised is a card or slip lying on the sheet.
func raised(e mu.Element, p theme.Colors) mu.Element {
	return e.Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).
		Shadow(0, 1, 2, 0, p.ShadowA(0.14)).Shadow(0, 8, 20, 0, p.ShadowA(0.08))
}

// raisedSmall is a secondary button, chip or select.
func raisedSmall(e mu.Element, p theme.Colors) mu.Element {
	return e.Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).InsetShadow(0, -1, 0, 0, p.ShadowA(0.18)).
		Shadow(0, 1, 1.5, 0, p.ShadowA(0.16))
}

// floating is a popover, menu or drawer.
func floating(e mu.Element, p theme.Colors) mu.Element {
	return e.Background(p.SheetRaised).InsetShadow(0, 1, 0, 0, p.Highlight(1)).InsetShadow(0, -1, 0, 0, p.ShadowA(0.10)).
		Shadow(0, 1, 2, 0, p.ShadowA(0.14)).Shadow(0, 12, 32, 0, p.ShadowA(0.16))
}

// sheetStack is the main page sheet; its 1 px spread shadow is the hairline
// edge.
func sheetStack(e mu.Element, p theme.Colors) mu.Element {
	return e.Background(p.Sheet).InsetShadow(0, 1, 0, 0, p.Highlight(0.95)).Shadow(0, 0, 0, 1, p.ShadowA(0.08)).
		Shadow(0, 1, 2, 0, p.ShadowA(0.10)).Shadow(0, 6, 20, 0, p.ShadowA(0.10))
}

// keycapStack is one key of a hotkey.
func keycapStack(e mu.Element, p theme.Colors) mu.Element {
	return e.Background(p.Sheet).InsetShadow(0, 1, 0, 0, p.Highlight(1)).InsetShadow(0, -1.5, 0, 0, p.ShadowA(0.22)).
		Shadow(0, 1, 1.5, 0, p.ShadowA(0.18))
}

// debossed is pressed into the paper: inputs, tracks, the active nav item.
func debossed(e mu.Element, p theme.Colors) mu.Element {
	return e.Background(p.Deboss).InsetShadow(0, 1, 3, 0, p.ShadowA(0.22)).InsetShadow(0, -1, 0, 0, p.Highlight(0.75))
}

// shallowWell is a large, shallow well.
func shallowWell(e mu.Element, p theme.Colors) mu.Element {
	return e.Background(p.Deboss).InsetShadow(0, 2, 5, 0, p.ShadowA(0.14)).InsetShadow(0, -1, 0, 0, p.Highlight(0.6))
}

var (
	black = mu.RGB(0, 0, 0)
	white = mu.RGB(255, 255, 255)
)

// inkButton is a primary button; pressed, it sinks.
func inkButton(e mu.Element, p theme.Colors, pressed bool) mu.Element {
	e.Background(p.InkFill)
	if pressed {
		return e.InsetShadow(0, 2, 4, 0, black.Alpha(0.5)).Shadow(0, 1, 0, 0, p.Highlight(0.7))
	}
	return e.InsetShadow(0, 1, 0, 0, white.Alpha(0.12)).InsetShadow(0, -2, 3, 0, black.Alpha(0.40)).Shadow(0, 1, 2, 0, p.ShadowA(0.30))
}

// ---- Grain ----

var (
	grainMu    sync.Mutex
	grainCache = map[string]*mu.Bitmap{}
)

// grainBitmap is a decoded 512 px paper grain, nil when it cannot decode.
func grainBitmap(name string) *mu.Bitmap {
	grainMu.Lock()
	defer grainMu.Unlock()
	if b, ok := grainCache[name]; ok {
		return b
	}
	var b *mu.Bitmap
	if data := theme.TexturePNG(name); data != nil {
		if img, err := png.Decode(bytes.NewReader(data)); err == nil {
			b = mu.NewBitmap(img)
		} else {
			log.Printf("texture %s: %v", name, err)
		}
	}
	grainCache[name] = b
	return b
}

// grainTile is the size of a grain tile in DIPs.
const grainTile = 512

// paintGrain tiles the board or sheet grain over r, unscaled from its
// top-left corner, clipped to radius.
func paintGrain(pt *mu.Painter, r mu.Rect, p theme.Colors, surface string, radius float32) {
	name := surface + "-light"
	if p.Dark {
		name = surface + "-dark"
	}
	b := grainBitmap(name)
	if b == nil {
		return
	}
	pt.Clip(r, radius, func() {
		for y := r.Y; y < r.Y+r.H; y += grainTile {
			for x := r.X; x < r.X+r.W; x += grainTile {
				pt.ImageRegion(b, mu.Rect{}, mu.Rect{X: x, Y: y, W: grainTile, H: grainTile}, mu.ImageOptions{})
			}
		}
	})
}

// grain is a Draw function painting the grain when the paper texture is on.
func (a *App) grain(p theme.Colors, surface string, radius float32) func(*mu.Painter, mu.Rect) {
	if !a.cfg.Appearance.PaperTexture {
		return func(*mu.Painter, mu.Rect) {}
	}
	return func(pt *mu.Painter, r mu.Rect) { paintGrain(pt, r, p, surface, radius) }
}

// ---- Type ----

// letterpress gives display text the pale copy 1 px under it.
func letterpress(e mu.Element, p theme.Colors) mu.Element {
	return e.TextShadow(0, 1, 0, p.Highlight(0.85))
}

// display is a page title or a big figure: Fraunces SemiBold, letterpressed.
func display(c *mu.Context, p theme.Colors, s string, size float32) mu.Element {
	return letterpress(heading(c, p, s, size), p)
}

// heading is a title in Fraunces SemiBold, with the line height of the
// Rust type scale (44/48, 22/28...).
func heading(c *mu.Context, p theme.Colors, s string, size float32) mu.Element {
	e := mu.Text(c, s).Font(serifFont(theme.Display)).FontSize(size).FontWeight(600).TextColor(p.Ink).LineHeight(headingLine(size))
	// MyGo never makes a line shorter than the font's ascent and descent
	// (Fraunces: about 1.18 em), where GPUI sets 44/48 and 40/44. Negative
	// margins give the line back the height Rust lays it out at.
	if d := (frauncesLine*size - headingLine(size)*size) / 2; d > 0 {
		e.Margin(-d, 0, -d, 0)
	}
	return e
}

// frauncesLine is the natural line height of Fraunces, ascent plus descent,
// in em.
const frauncesLine = 1.1777

func headingLine(size float32) float32 {
	switch {
	case size >= 40:
		return 48.0 / 44
	case size >= 26:
		return 1.15
	}
	return 28.0 / 22
}

// caps is a small uppercase label (text::caps): UI 12/16 Medium, Graphite.
func caps(c *mu.Context, p theme.Colors, s string) mu.Element {
	return mu.Text(c, strings.ToUpper(s)).FontSize(12).FontWeight(500).TextColor(p.Graphite).LineHeight(16.0 / 12).SingleLine()
}

// eyebrow is caps, kept for the secondary windows.
func eyebrow(c *mu.Context, p theme.Colors, s string) { caps(c, p, s) }

// ui is Instrument Sans text.
func uiText(c *mu.Context, s string, size float32, weight int, col mu.Color) mu.Element {
	return mu.Text(c, s).FontSize(size).FontWeight(weight).TextColor(col)
}

// serif is list text: Fraunces (display cut) Regular 15/18.
func serif(c *mu.Context, s string, size float32, col mu.Color) mu.Element {
	return mu.Text(c, s).Font(serifFont(theme.Display)).FontSize(size).FontWeight(400).TextColor(col)
}

// mono is IBM Plex Mono Medium, or the interface face for text in a script
// mono would break (monoFont).
func mono(c *mu.Context, s string, size float32, col mu.Color) mu.Element {
	return mu.Text(c, s).Font(monoFont(s)).FontSize(size).FontWeight(500).TextColor(col).SingleLine()
}

// monoFont is the face of mono text s: IBM Plex Mono for digits, sizes and
// Latin, Greek or Cyrillic words, else the interface face of the locale.
// Plex Mono has no Arabic or Hebrew, and the fallback set in its fixed
// advances pulls joined Arabic letters apart ("الإنجليزية", "لم يبدأ"), so
// translated text in such a script ("1.3 GB · الإنجليزية") reads in sans.
// A path, an id or a hex color the user types stays mono: use theme.Mono.
func monoFont(s string) string {
	if !shaped(s) {
		return theme.Mono
	}
	return theme.UIFor(i18n.CurrentLocale())
}

// shaped reports whether s has a letter of a script whose glyphs join or
// combine into clusters (Arabic, Hebrew, Indic, Thai and the like), which
// neither a monospace face nor letter spacing may set. Latin, Greek,
// Cyrillic and the CJK scripts set fine either way.
func shaped(s string) bool {
	for _, r := range s {
		if r < 0x0370 || !unicode.IsLetter(r) {
			continue
		}
		if !unicode.In(r, unicode.Latin, unicode.Greek, unicode.Cyrillic, unicode.Han,
			unicode.Hiragana, unicode.Katakana, unicode.Hangul, unicode.Bopomofo) {
			return true
		}
	}
	return false
}

// tracking is letter spacing em×size for s, or none for text in a script
// whose letters join (shaped): spacing them breaks the joins.
func tracking(s string, em, size float32) float32 {
	if shaped(s) {
		return 0
	}
	return em * size
}

// sectionHead is a 22 px serif title over a rule, with a note at the right.
func sectionHead(c *mu.Context, p theme.Colors, title, note string) {
	mu.Row(c).AlignItems(mu.Baseline).Gap(12).PaddingY(0).Padding(0, 0, 10, 0).BorderWidth(0, 0, 1, 0).BorderColor(p.Rule).Children(func() {
		heading(c, p, title, 22).Role(mu.RoleHeading).Grow(1)
		if note != "" {
			uiText(c, note, 13, 400, p.Graphite).SingleLine()
		}
	})
}

// ---- Dots, seals, badges ----

// statusDot is an 8 px dot in a 14 px halo of its color.
func statusDot(c *mu.Context, col mu.Color) {
	mu.Box(c).Size(14, 14).Shrink(0).Radius(99).Background(col.Alpha(0.16)).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
		mu.Box(c).Size(8, 8).Radius(99).Background(col)
	})
}

// checkDot is a checklist mark: a check on a debossed dot, or a warning.
func checkDot(c *mu.Context, p theme.Colors, ok bool) {
	bg, mark, fg := p.Deboss, "check", p.Ink
	if !ok {
		bg, mark, fg = p.DangerWash, "warning", p.Danger
	}
	mu.Box(c).Size(16, 16).Shrink(0).Radius(99).Background(bg).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
		mu.Icon(c, dirIcon(c, mark)).FontSize(10).TextColor(fg)
	})
}

// inkColor is a style's ink as a color, and false for none (Raw).
func inkColor(hex string, p theme.Colors) (mu.Color, bool) {
	if rgb, ok := theme.ParseRgb(hex); ok {
		return theme.UIColor(rgb), true
	}
	return p.Ink, false
}

// sealDot is a style's domed ink dot; a style with no ink is a pencil ring.
func sealDot(c *mu.Context, p theme.Colors, hex string, size float32) mu.Element {
	col, ok := inkColor(hex, p)
	d := mu.Box(c).Size(size, size).Shrink(0).Radius(99)
	if !ok {
		return d.Border(1, p.Pencil)
	}
	return d.Background(col).InsetShadow(0, -1.5, 2, 0, black.Alpha(0.35)).InsetShadow(0, 1, 0, 0, white.Alpha(0.25))
}

// seal is the 12 px seal dot of a style.
func seal(c *mu.Context, hex string, p theme.Colors) { sealDot(c, p, hex, 12) }

type badgeTone int

const (
	toneAccent badgeTone = iota
	toneInk
	toneMuted
)

// badge is a small uppercase pill: "DEFAULT", "☁ CLOUD".
func badge(c *mu.Context, p theme.Colors, label string, tone badgeTone, iconName string) {
	bg, fg := p.AccentWash, p.Accent
	switch tone {
	case toneInk:
		bg, fg = p.InkFill, p.OnInk
	case toneMuted:
		bg, fg = p.Deboss, p.Graphite
	}
	mu.Row(c).Height(22).Shrink(0).PaddingX(8).Gap(5).Radius(99).Background(bg).AlignItems(mu.Center).Children(func() {
		if iconName != "" {
			mu.Icon(c, dirIcon(c, iconName)).FontSize(11).TextColor(fg)
		}
		uiText(c, strings.ToUpper(label), 11, 600, fg).SingleLine()
	})
}

// slip is a small label; it draws as a muted badge.
func slip(c *mu.Context, p theme.Colors, label string, _ mu.Color) { badge(c, p, label, toneMuted, "") }

// styleTag is a style's name in a debossed pill with its ink dot.
func styleTag(c *mu.Context, p theme.Colors, name, hex string) {
	debossed(mu.Row(c).Height(22).Width(84).Shrink(0).PaddingX(9).Gap(6).Radius(99).AlignItems(mu.Center), p).Children(func() {
		col, ok := inkColor(hex, p)
		dot := mu.Box(c).Size(6, 6).Shrink(0).Radius(99)
		if ok {
			dot.Background(col)
		} else {
			dot.Border(1, p.Pencil)
		}
		uiText(c, name, 12, 500, p.Graphite).SingleLine().Ellipsis("…")
	})
}

// keycaps are the keys of a hotkey. small is the 20 px size.
func keycaps(c *mu.Context, p theme.Colors, keys []string, small bool) {
	h, px, size, r, gap := float32(26), float32(8), float32(12), float32(6), float32(4)
	if small {
		h, px, size, r, gap = 20, 6, 11, 5, 3
	}
	// A chord reads left to right in every language, as macOS shows it.
	ltr(mu.Row(c)).Gap(gap).AlignItems(mu.Center).Shrink(0).Children(func() {
		for _, k := range keys {
			pad := px
			if len([]rune(k)) > 2 && !small {
				pad = 10
			}
			keycapStack(mu.Box(c).Height(h).MinWidth(h).PaddingX(pad).Radius(r).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
				mono(c, k, size, p.Ink)
			})
		}
	})
}

// keycap is one medium key.
func keycap(c *mu.Context, p theme.Colors, label string) { keycaps(c, p, []string{label}, false) }

// miniKey is a hotkey as one small key ("^⌥S", "⌘F").
func miniKey(c *mu.Context, p theme.Colors, label string, fg mu.Color) {
	keycapStack(mu.Box(c).Height(20).PaddingX(6).Radius(5).Shrink(0).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
		ltr(mono(c, label, 11, fg))
	})
}

// ---- App badges ----

// appTones are the stable colors of a letter badge.
var appTones = []mu.Color{
	mu.RGB(0x4A, 0x5B, 0x8C), mu.RGB(0x6B, 0x4A, 0x6E), mu.RGB(0x3F, 0x6E, 0x5E), mu.RGB(0x8C, 0x5A, 0x3C),
	mu.RGB(0x5A, 0x62, 0x6E), mu.RGB(0x7A, 0x3E, 0x3E), mu.RGB(0x3C, 0x6A, 0x8A),
}

// appBadge is an app's icon, or its first letter on a tone of its own.
func (a *App) appBadge(c *mu.Context, name, bundle string, size float32) {
	if b := a.appIcon(bundle); b != nil {
		mu.Image(c, b).Size(size, size).Shrink(0).Label(name)
		return
	}
	letter := "?"
	for _, r := range strings.TrimSpace(name) {
		letter = strings.ToUpper(string(r))
		break
	}
	h := 0
	for _, r := range bundle + name {
		h = h*31 + int(r)
	}
	if h < 0 {
		h = -h
	}
	mu.Box(c).Size(size, size).Shrink(0).Radius(size*0.25).Background(appTones[h%len(appTones)]).
		InsetShadow(0, -1, 0, 0, black.Alpha(0.25)).Justify(mu.Center).AlignItems(mu.Center).Children(func() {
		mu.Text(c, letter).FontSize(size * 0.46).FontWeight(700).TextColor(white)
	})
}

// appIcons caches app icons by bundle id; nil means the app has none.
type appIcons struct {
	mu    sync.Mutex
	known map[string]*mu.Bitmap
	busy  map[string]bool
	// names are the display names of installed apps; "" for an app that
	// is not installed.
	names     map[string]string
	namesBusy bool
}

// appIcon is the icon of an app, read once off the UI thread; nil until
// it is read, or when there is none.
func (a *App) appIcon(bundle string) *mu.Bitmap {
	if bundle == "" || a.d.Platform == nil || a.d.Platform.AppIcons == nil {
		return nil
	}
	ic := &a.icons
	ic.mu.Lock()
	defer ic.mu.Unlock()
	if b, ok := ic.known[bundle]; ok {
		return b
	}
	if ic.busy[bundle] {
		return nil
	}
	if ic.busy == nil {
		ic.busy, ic.known = map[string]bool{}, map[string]*mu.Bitmap{}
	}
	ic.busy[bundle] = true
	src := a.d.Platform.AppIcons
	go func() {
		var bm *mu.Bitmap
		if data := src.AppIconPNG(bundle, 64); len(data) > 0 {
			if img, err := png.Decode(bytes.NewReader(data)); err == nil {
				bm = mu.NewBitmap(img)
			}
		}
		ic.mu.Lock()
		ic.known[bundle] = bm
		delete(ic.busy, bundle)
		ic.mu.Unlock()
		if bm != nil {
			a.post(a.invalidate)
		}
	}()
	return nil
}

// installedNames are the names of the installed apps among bundles, read
// once off the UI thread. ok is false until all of them are read.
func (a *App) installedNames(bundles []string) (names map[string]string, ok bool) {
	if a.d.Platform == nil || a.d.Platform.AppIcons == nil {
		return nil, true
	}
	ic := &a.icons
	ic.mu.Lock()
	defer ic.mu.Unlock()
	var missing []string
	for _, b := range bundles {
		if _, known := ic.names[b]; !known {
			missing = append(missing, b)
		}
	}
	if len(missing) == 0 {
		// A copy: a later lookup writes the cache from its goroutine.
		out := make(map[string]string, len(bundles))
		for _, b := range bundles {
			out[b] = ic.names[b]
		}
		return out, true
	}
	if ic.namesBusy {
		return nil, false
	}
	ic.namesBusy = true
	src := a.d.Platform.AppIcons
	go func() {
		got := make(map[string]string, len(missing))
		for _, b := range missing {
			got[b] = src.AppName(b)
		}
		ic.mu.Lock()
		if ic.names == nil {
			ic.names = map[string]string{}
		}
		for b, n := range got {
			ic.names[b] = n
		}
		ic.namesBusy = false
		ic.mu.Unlock()
		a.post(a.invalidate)
	}()
	return nil, false
}
