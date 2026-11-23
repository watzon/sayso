package ui

import (
	"image"
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
)

// The detail's text lines are 44 DIPs apart and its scroll box at most
// 154 high, with a 28 DIP fade at an edge past which there is more text
// (history.go historyDetail).
const (
	detailLine = 44
	detailMaxH = 154
	detailFade = 28
)

// detailTextBox is the visible box of the selected entry's text: the text
// laid out from the scroll container's top, moved up as far as it scrolls.
func detailTextBox(t *testing.T, a *App, tt *mu.Tester) mu.Rect {
	t.Helper()
	name := i18n.T("history.a11y.final_text")
	for _, info := range tt.AccessAll() {
		if info.Name == name {
			b := info.Bounds
			return mu.Rect{X: b.X, Y: b.Y + a.hist.textScroll.Y, W: b.W, H: min(max(b.H, 66), detailMaxH)}
		}
	}
	t.Fatalf("no final text; texts %q", tt.Texts())
	return mu.Rect{}
}

// rowInk is the darkest luminance of row y of img across box: low where
// a glyph crosses it.
func rowInk(img *image.RGBA, box mu.Rect, y int) int {
	darkest := 255
	for x := int(box.X); x < int(box.X+box.W); x++ {
		c := img.RGBAAt(x, y)
		darkest = min(darkest, (299*int(c.R)+587*int(c.G)+114*int(c.B))/1000)
	}
	return darkest
}

// edgeFaded reports whether the top (or bottom) edge of box is faded out.
// Every line holds the same text, so without a fade a row near the edge
// inks as the row a line or two further in does, outside either fade;
// under a fade it is much lighter.
func edgeFaded(img *image.RGBA, box mu.Rect, top bool) bool {
	lo, hi := int(box.Y), int(box.Y+box.H)
	from, step := hi-detailFade/2, -detailLine
	if top {
		from, step = lo, detailLine
	}
	for y := from; y < from+detailFade/2; y++ {
		for k := 1; k <= 2; k++ {
			ref := y + k*step
			if ref < lo+detailFade || ref >= hi-detailFade {
				continue
			}
			if rowInk(img, box, ref)+80 < rowInk(img, box, y) {
				return true
			}
		}
	}
	return false
}

// wantFades checks which edges of the selected entry's text are faded.
func wantFades(t *testing.T, a *App, tt *mu.Tester, state string, top, bottom bool) {
	t.Helper()
	box, img := detailTextBox(t, a, tt), tt.Image()
	if got := edgeFaded(img, box, true); got != top {
		t.Errorf("%s (scroll %+v): top faded %v, want %v", state, a.hist.textScroll, got, top)
	}
	if got := edgeFaded(img, box, false); got != bottom {
		t.Errorf("%s (scroll %+v): bottom faded %v, want %v", state, a.hist.textScroll, got, bottom)
	}
}

func lines(line string, n int) string {
	return strings.TrimSuffix(strings.Repeat(line+"\n", n), "\n")
}

// A short text that overflows its box fades at the edges past which there
// is more of it: the bottom at its start, both edges in the middle, the
// top at its end. An entry chosen afterwards shows from its start. The
// old rule faded only the bottom of texts over 140 bytes, always.
func TestHistoryTextFadesTheEdgesWithMorePastThem(t *testing.T) {
	e := newEnv(t)
	other := lines("MMMM", 6)
	text := lines("MMMM", 5)
	if len(text) > 140 {
		t.Fatalf("the text is %d bytes; it must be short", len(text))
	}
	e.addEntry(t, other, "Notes", false)
	e.addEntry(t, text, "Telegram", false)
	e.a.SetPage(PageHistory)
	tt := e.hub(t)
	tt.Frame()
	if got := e.a.hist.entries[0].FinalText; got != text {
		t.Fatalf("selected %q, want the short text", got)
	}
	if s := e.a.hist.textScroll; s.MaxY <= 1 || s.Y != 0 {
		t.Fatalf("scroll %+v: the 5 lines should overflow %v DIPs, from the top", s, detailMaxH)
	}
	wantFades(t, e.a, tt, "at the top", false, true)

	// 12 DIPs down: a glyph crosses both edges of the box.
	e.a.hist.textScroll.Y = 12
	tt.Frame()
	if s := e.a.hist.textScroll; s.Y != 12 {
		t.Fatalf("scroll %+v: want 12", s)
	}
	wantFades(t, e.a, tt, "in the middle", true, true)

	box := detailTextBox(t, e.a, tt)
	tt.Scroll(box.X+box.W/2, box.Y+box.H/2, 0, 1000)
	tt.Frame()
	if s := e.a.hist.textScroll; s.Y < s.MaxY-1 {
		t.Fatalf("scroll %+v: the touchpad did not reach the end", s)
	}
	wantFades(t, e.a, tt, "at the bottom", true, false)

	press(t, tt, entryRowName(e.a, e.a.hist.entries[1]))
	tt.Frame()
	if e.a.hist.selected != e.a.hist.entries[1].ID {
		t.Fatal("the other entry was not selected")
	}
	if s := e.a.hist.textScroll; s.Y != 0 || s.MaxY <= 1 {
		t.Fatalf("scroll %+v: the other entry should show from its top", s)
	}
	wantFades(t, e.a, tt, "another entry", false, true)
}

// A text over 140 bytes that fits its box, in a wide window, is not
// faded: the old rule faded its last line anyway.
func TestHistoryTextThatFitsIsNotFaded(t *testing.T) {
	e := newEnv(t)
	text := lines(strings.Repeat("il ", 26), 3)
	if len(text) <= 140 {
		t.Fatalf("the text is %d bytes; it must be long", len(text))
	}
	e.addEntry(t, text, "Electron", false)
	e.a.SetPage(PageHistory)
	tt := mu.NewTester(e.a.HubView, 1800, HubHeight)
	tt.Frame()

	box := detailTextBox(t, e.a, tt)
	if s := e.a.hist.textScroll; s.MaxY > 1 {
		t.Fatalf("scroll %+v, box %+v: the three lines should fit", s, box)
	}
	if box.H != 3*detailLine {
		t.Fatalf("box %+v: want three lines high", box)
	}
	wantFades(t, e.a, tt, "fitting", false, false)
}
