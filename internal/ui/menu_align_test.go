package ui

import (
	"image"
	"math"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
)

// The labels of a menu line up at the left of its lines, after the icon or
// check column, whatever their length. ButtonBase centers its children on
// both axes, so a menu item that does not ask for its start draws a short
// label in the middle of a line as wide as the longest. A label inside a
// labeled button is not in the accessibility tree, so these tests read the
// first column of ink after the icon column in the rendered frame.

// inkStart is the left edge, in DIPs, of the first column of r right of
// from that has a pixel unlike r's background, which is sampled at r's
// right padding. It is r's right edge when the line has no ink there.
func inkStart(img *image.RGBA, scale float32, r mu.Rect, from float32) float32 {
	px := func(v float32) int { return int(math.Round(float64(v * scale))) }
	y0, y1 := px(r.Y+6), px(r.Y+r.H-6)
	bg := img.RGBAAt(px(r.X+r.W)-3, px(r.Y+r.H/2))
	for x := px(from); x < px(r.X+r.W)-4; x++ {
		for y := y0; y < y1; y++ {
			c := img.RGBAAt(x, y)
			if diff(c.R, bg.R) > 48 || diff(c.G, bg.G) > 48 || diff(c.B, bg.B) > 48 {
				return float32(x) / scale
			}
		}
	}
	return r.X + r.W
}

func diff(a, b uint8) int {
	if a > b {
		return int(a - b)
	}
	return int(b - a)
}

// labelStarts checks that the label of each named control, an item of an
// open menu, starts lead DIPs from the item's left edge, the icon or check
// column being the first iconW DIPs after its left padding pad.
func labelStarts(t *testing.T, tt *mu.Tester, width int, names []string, pad, iconW, lead float32) {
	t.Helper()
	img := tt.Image()
	scale := float32(img.Bounds().Dx()) / float32(width)
	var first float32
	for i, n := range names {
		r, ok := controlBounds(tt, n)
		if !ok {
			t.Fatalf("no menu item %q", n)
		}
		x := inkStart(img, scale, r, r.X+pad+iconW+1)
		// A glyph's side bearing puts its ink up to a DIP or two right of
		// its box.
		if got := x - r.X; got < lead-1 || got > lead+2.5 {
			t.Errorf("%q: label ink at item.left+%.1f, want item.left+%v (item %+v)", n, got, lead, r)
		}
		if i == 0 {
			first = x
		} else if d := x - first; d < -1.5 || d > 1.5 {
			t.Errorf("%q starts at x=%.1f, %q at x=%.1f: the labels are not lined up", n, x, names[0], first)
		}
	}
}

// The ⋯ menu of a history entry (Rust kit.rs action_menu): 10 px left
// padding, a 14 px icon, a 10 px gap, then the label.
func TestHistoryMenuLabelsStartAligned(t *testing.T) {
	e := newEnv(t)
	e.addEntry(t, "first apple note", "Notes", true)
	e.a.SetPage(PageHistory)
	tt := e.hub(t)
	e.a.selectEntry(e.a.hist.entries[0].ID)
	tt.Frame()
	press(t, tt, i18n.T("history.a11y.more_actions"))
	names := []string{
		i18n.T("history.menu.copy"), i18n.T("history.menu.copy_metadata"), i18n.T("history.menu.enhance"),
		i18n.T("history.menu.transcribe_again"), i18n.T("history.delete"),
	}
	labelStarts(t, tt, HubWidth, names, 10, 14, 10+14+10)
	// The icon is drawn in its column, at the left.
	img := tt.Image()
	scale := float32(img.Bounds().Dx()) / HubWidth
	for _, n := range names {
		r, _ := controlBounds(tt, n)
		if x := inkStart(img, scale, r, r.X+1) - r.X; x < 10 || x > 13 {
			t.Errorf("%q: first ink at item.left+%.1f, want the icon at item.left+10", n, x)
		}
	}
}

// The models page's Sort menu (Rust kit.rs menu_items): 10 px padding, a
// 14 px check column, an 8 px gap, then the label.
func TestModelsSortMenuLabelsStartAligned(t *testing.T) {
	e := newEnv(t)
	e.a.SetPage(PageModels)
	tt := e.hub(t)
	press(t, tt, i18n.T("models.sort.button", "key", i18n.T("models.sort.recommended")))
	names := []string{i18n.T("models.sort.recommended"), i18n.T("models.group.accuracy"), i18n.T("models.group.speed"), i18n.T("models.group.size")}
	labelStarts(t, tt, HubWidth, names, 10, 14, 10+14+8)
}
