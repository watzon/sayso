package ui

import (
	"bytes"
	"fmt"
	"log"
	"regexp"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// Right to left. MyGo mirrors the layout of a window laid out right to left
// (ui.Direction): Rows run from the right, Start and End swap, the logical
// edges (PaddingStart, MarginStart, Start, BorderEndWidth) move, scroll
// bars, popovers, selects and menus open from the right, and text aligns
// and shapes as the language reads. Sayso picks the direction of its own
// interface language (i18n.IsRTL), not MyGo's App locale, so each window's
// root takes it at the top of its view (applyDirection), before anything
// is built in it or in its overlay layer. What MyGo leaves to the app:
// physical edges (Padding with four values, Left, Right), Draw callbacks,
// and icons, which it never mirrors (dirIcon).

// applyDirection lays the window out in the direction of the interface
// language, and gives the root the sans fallbacks of a script the bundled
// fonts lack. Call it first in a view: an element takes the direction of
// the element it is built in.
func applyDirection(c *mu.Context) {
	d := mu.LeftToRight
	if i18n.IsRTL() {
		d = mu.RightToLeft
	}
	root := c.Root().Direction(d)
	if f := theme.UIFor(i18n.CurrentLocale()); f != theme.UI {
		root.Font(f)
	}
}

// rtl reports whether what is built now reads right to left.
func rtl(c *mu.Context) bool { return c.Direction() == mu.RightToLeft }

// mirroredIcons point along the line, back or forth, or show which side the
// sidebar is on: right to left they point the other way, as SF Symbols and
// Material's auto-mirrored icons do. Icons that point nowhere (check, plus,
// close, mic, play, search) never mirror.
var mirroredIcons = map[string]bool{
	"chevron-left":  true,
	"chevron-right": true,
	"arrow-right":   true,
	"arrow-left":    true,
	"sidebar":       true,
	"undo":          true,
}

var mirroredCache = map[string]*mu.SVG{}

// dirIcon is icon(name), mirrored right to left when the icon points along
// the line.
func dirIcon(c *mu.Context, name string) *mu.SVG {
	if !mirroredIcons[name] || !rtl(c) {
		return icon(name)
	}
	iconMu.Lock()
	s, ok := mirroredCache[name]
	iconMu.Unlock()
	if ok {
		return s
	}
	s, err := mu.ParseSVG(mirrorSVG(theme.IconSVG(name)))
	if err != nil {
		log.Printf("icon %s mirrored: %v", name, err)
		return icon(name)
	}
	iconMu.Lock()
	mirroredCache[name] = s
	iconMu.Unlock()
	return s
}

var viewBoxRe = regexp.MustCompile(`viewBox="([-\d.]+) [-\d.]+ ([-\d.]+) [-\d.]+"`)

// mirrorSVG flips an icon across its vertical center line by wrapping what
// is inside its svg element in a mirroring group.
func mirrorSVG(data []byte) []byte {
	m := viewBoxRe.FindSubmatch(data)
	open := bytes.IndexByte(data, '>')
	end := bytes.LastIndex(data, []byte("</svg>"))
	if m == nil || open < 0 || end < open {
		return data
	}
	var x, w float64
	fmt.Sscan(string(m[1]), &x)
	fmt.Sscan(string(m[2]), &w)
	var b bytes.Buffer
	b.Write(data[:open+1])
	fmt.Fprintf(&b, `<g transform="matrix(-1 0 0 1 %g 0)">`, 2*x+w)
	b.Write(data[open+1 : end])
	b.WriteString("</g></svg>")
	return b.Bytes()
}

// mirrored is fn painted mirrored across the vertical center line of its
// box when on, for a drawing that runs along the line: a progress fill, a
// waveform whose newest level is at the end, the days of a week. Read the
// direction while building (rtl), never in the callback. Off, it is fn.
func mirrored(on bool, fn func(*mu.Painter, mu.Rect)) func(*mu.Painter, mu.Rect) {
	if !on {
		return fn
	}
	return func(pt *mu.Painter, r mu.Rect) {
		pt.Transform(mu.Scaling(-1, 1), r.X+r.W/2, r.Y, func() { fn(pt, r) })
	}
}

// ltr keeps an element and what is in it left to right in a window laid out
// right to left: a hotkey's keycaps, a path, a model id, a hex color.
func ltr(e mu.Element) mu.Element { return e.Direction(mu.LeftToRight) }
