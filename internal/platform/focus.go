package platform

import "math"

// Helpers of the focus trackers that hold no system call, so every
// platform's tests check them.

// rectFromFloats rounds a frame given in points to whole points.
func rectFromFloats(x, y, w, h float64) Rect {
	return Rect{X: int(math.Round(x)), Y: int(math.Round(y)), Width: int(math.Round(w)), Height: int(math.Round(h))}
}

// intersect is the part of a and b that both cover; empty when none.
func intersect(a, b Rect) Rect {
	x0, y0 := max(a.X, b.X), max(a.Y, b.Y)
	x1, y1 := min(a.X+a.Width, b.X+b.Width), min(a.Y+a.Height, b.Y+b.Height)
	if x1 <= x0 || y1 <= y0 {
		return Rect{}
	}
	return Rect{X: x0, Y: y0, Width: x1 - x0, Height: y1 - y0}
}

// focusRect is the frame the focus is in: the focused input inside the
// focused window when it shows there, else the window. A web page or a
// long text view can report a frame far larger than the window, so the
// input counts only for its part in the window. ok is false without a
// window: a focused element with no window (Finder's desktop icons) is
// the desktop.
func focusRect(window, input Rect) (Rect, bool) {
	if window.Empty() {
		return Rect{}, false
	}
	if in := intersect(window, input); !in.Empty() {
		return in, true
	}
	return window, true
}

// sysWindow is one window of the system's list, front to back.
type sysWindow struct {
	pid    int64
	layer  int64
	bounds Rect
	name   string
}

// overlayTitle is the title of Sayso's overlay window (ui.Desktop).
const overlayTitle = "Sayso Overlay"

// frontWindow is the frame of the topmost normal window of pid in a list
// ordered front to back (CGWindowListCopyWindowInfo). Normal windows are
// on layer 0; the overlay, the tray popover, menus and panels float above
// it, so a press on the overlay or the popover never counts as focus even
// when Sayso is in front.
func frontWindow(list []sysWindow, pid int64) (Rect, bool) {
	for _, w := range list {
		if w.pid != pid || w.layer != 0 || w.bounds.Empty() || w.name == overlayTitle {
			continue
		}
		return w.bounds, true
	}
	return Rect{}, false
}
