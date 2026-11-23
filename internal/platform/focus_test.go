package platform

import "testing"

func TestFocusRect(t *testing.T) {
	window := Rect{X: 100, Y: -1200, Width: 1000, Height: 800}
	for _, tc := range []struct {
		name          string
		window, input Rect
		want          Rect
		ok            bool
	}{
		{"input inside the window", window, Rect{X: 300, Y: -900, Width: 400, Height: 30}, Rect{X: 300, Y: -900, Width: 400, Height: 30}, true},
		// A web area reports the whole page; only its part in the window counts.
		{"input larger than the window", window, Rect{X: 100, Y: -1200, Width: 1000, Height: 9000}, window, true},
		{"input outside the window", window, Rect{X: 3000, Y: 0, Width: 10, Height: 10}, window, true},
		{"no input", window, Rect{}, window, true},
		{"no window (Finder's desktop)", Rect{}, Rect{X: 0, Y: 0, Width: 2560, Height: 1440}, Rect{}, false},
	} {
		got, ok := focusRect(tc.window, tc.input)
		if got != tc.want || ok != tc.ok {
			t.Errorf("%s: focusRect = %+v, %v; want %+v, %v", tc.name, got, ok, tc.want, tc.ok)
		}
	}
}

func TestIntersect(t *testing.T) {
	a := Rect{X: 0, Y: -100, Width: 100, Height: 200}
	if got := intersect(a, Rect{X: 50, Y: 50, Width: 100, Height: 100}); got != (Rect{X: 50, Y: 50, Width: 50, Height: 50}) {
		t.Errorf("overlap: %+v", got)
	}
	if got := intersect(a, Rect{X: 100, Y: 0, Width: 10, Height: 10}); !got.Empty() {
		t.Errorf("touching edges: %+v", got)
	}
}

func TestRectFromFloats(t *testing.T) {
	if got := rectFromFloats(10.4, -1439.6, 799.5, 600.49); got != (Rect{X: 10, Y: -1440, Width: 800, Height: 600}) {
		t.Errorf("rectFromFloats = %+v", got)
	}
}

// The window list is front to back. The overlay and the tray popover float
// above layer 0, so they never count, even with Sayso in front; its hub
// does.
func TestFrontWindow(t *testing.T) {
	const sayso, editor = 500, 600
	overlay := sysWindow{pid: sayso, layer: 25, bounds: Rect{X: 1000, Y: -200, Width: 400, Height: 120}, name: overlayTitle}
	popover := sysWindow{pid: sayso, layer: 101, bounds: Rect{X: 1900, Y: 30, Width: 320, Height: 420}}
	hub := sysWindow{pid: sayso, layer: 0, bounds: Rect{X: 200, Y: -1300, Width: 1100, Height: 760}, name: "Sayso"}
	menuBar := sysWindow{pid: editor, layer: 24, bounds: Rect{X: 0, Y: 0, Width: 2560, Height: 25}}
	editorTop := sysWindow{pid: editor, layer: 0, bounds: Rect{X: 0, Y: -1440, Width: 1280, Height: 1400}}
	editorBack := sysWindow{pid: editor, layer: 0, bounds: Rect{X: 0, Y: 40, Width: 1280, Height: 1400}}
	list := []sysWindow{overlay, popover, menuBar, hub, editorTop, editorBack}

	if r, ok := frontWindow(list, editor); !ok || r != editorTop.bounds {
		t.Errorf("editor: %+v, %v", r, ok)
	}
	if r, ok := frontWindow(list, sayso); !ok || r != hub.bounds {
		t.Errorf("Sayso in front: %+v, %v; want the hub", r, ok)
	}
	if _, ok := frontWindow([]sysWindow{overlay, popover}, sayso); ok {
		t.Error("the overlay or the popover counted as focus")
	}
	// A layer-0 window named as the overlay never counts either.
	named := sysWindow{pid: sayso, layer: 0, bounds: overlay.bounds, name: overlayTitle}
	if _, ok := frontWindow([]sysWindow{named}, sayso); ok {
		t.Error("the overlay counted as focus")
	}
	if _, ok := frontWindow([]sysWindow{{pid: editor, layer: 0}}, editor); ok {
		t.Error("an empty window counted as focus")
	}
	if _, ok := frontWindow(list, 42); ok {
		t.Error("an app with no window has focus")
	}
}
