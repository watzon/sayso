package ui

import (
	"image/color"
	"testing"

	mu "github.com/egoist/mygo/ui"
)

// winBar is the room the Windows caption buttons take at the top right.
var winBar = mu.TitleBar{Height: 32, Right: 138, Bottom: 32}

// Under the Windows caption buttons the hub shows the bare board, the
// color beside the page: no sheet of paper, peeking or not, reaches above
// the page. Only the soft shadow of the page, about 14 px, rises above it.
func TestNoSheetShowsUnderTheWindowsCaption(t *testing.T) {
	for _, dark := range []bool{false, true} {
		e := newEnv(t)
		e.a.cfg.Appearance.PaperTexture = false
		tt := e.hub(t)
		tt.SetDark(dark)
		tt.SetTitleBar(winBar)
		tt.Frame()
		img := tt.Image()
		w := img.Bounds().Dx()
		board := img.RGBAAt(w-2, 20)
		for y := 2; y < int(winBar.Bottom)-15; y++ {

			for _, x := range []int{sidebarWidth + 40, (sidebarWidth + w) / 2, w - int(winBar.Right) - 10, w - 60} {
				if got := img.RGBAAt(x, y); !near(got, board) {
					t.Fatalf("dark %v: (%d, %d) is %v under the caption, the board is %v", dark, x, y, got, board)
				}
			}
		}
		if got := img.RGBAAt((sidebarWidth+w)/2, int(winBar.Bottom)+30); near(got, board) {
			t.Errorf("dark %v: no page under the caption band (%v)", dark, got)
		}
	}
}

// The traffic lights of macOS sit over the sidebar, so the page keeps its
// 10 px margin at the top.
func TestTheMacPageStaysAtTheTop(t *testing.T) {
	e := newEnv(t)
	e.a.cfg.Appearance.PaperTexture = false
	tt := e.hub(t)
	tt.SetTitleBar(macBar)
	tt.Frame()
	img := tt.Image()
	w := img.Bounds().Dx()
	board := img.RGBAAt(w-2, 20)
	if got := img.RGBAAt((sidebarWidth+w)/2, 20); near(got, board) {
		t.Errorf("the page does not start near the top on macOS: (%d, 20) is the board", (sidebarWidth+w)/2)
	}
}

func near(a, b color.RGBA) bool {
	d := func(x, y uint8) int {
		if x > y {
			return int(x - y)
		}
		return int(y - x)
	}
	return d(a.R, b.R) <= 2 && d(a.G, b.G) <= 2 && d(a.B, b.B) <= 2
}
