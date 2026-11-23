package ui

import (
	"slices"
	"testing"
	"time"

	"github.com/egoist/mygo"
)

// Two displays stacked, as Chris's (2026-11-22): the primary at the bottom,
// another above it, at negative y. Each has a 25 px menu bar of its own
// (macOS, displays with separate Spaces).
var (
	bottomDisplay = mygo.Display{ID: 1, Primary: true,
		Bounds: mygo.Rectangle{X: 0, Y: 0, Width: 2560, Height: 1440}, WorkArea: mygo.Rectangle{X: 0, Y: 25, Width: 2560, Height: 1415}}
	topDisplay = mygo.Display{ID: 2,
		Bounds: mygo.Rectangle{X: 0, Y: -1440, Width: 2560, Height: 1440}, WorkArea: mygo.Rectangle{X: 0, Y: -1415, Width: 2560, Height: 1415}}
	stacked = []mygo.Display{bottomDisplay, topDisplay}

	// The tray icon in each display's menu bar, and a press on it.
	bottomIcon  = mygo.Rectangle{X: 2000, Y: 0, Width: 30, Height: 24}
	topIcon     = mygo.Rectangle{X: 2000, Y: -1440, Width: 30, Height: 24}
	bottomPress = mygo.Point{X: 2015, Y: 12}
	topPress    = mygo.Point{X: 2015, Y: -1428}
)

// A Windows display with its taskbar at the foot.
var taskbarDisplay = mygo.Display{ID: 1, Primary: true,
	Bounds: mygo.Rectangle{X: 0, Y: 0, Width: 1920, Height: 1080}, WorkArea: mygo.Rectangle{X: 0, Y: 0, Width: 1920, Height: 1032}}

func TestPlacePopover(t *testing.T) {
	const w, h = PopoverW, PopoverH
	// Under the icon (or the pointer) at x 2015, 6 px under the menu bar.
	under := func(menuBarFoot int) mygo.Point { return mygo.Point{X: 2015 - w/2, Y: menuBarFoot + popoverGap} }
	narrow := mygo.Display{Bounds: mygo.Rectangle{X: 3000, Y: 0, Width: 300, Height: 400}, WorkArea: mygo.Rectangle{X: 3000, Y: 25, Width: 300, Height: 375}}
	left := mygo.Display{Bounds: mygo.Rectangle{X: -1920, Y: 0, Width: 1920, Height: 1080}, WorkArea: mygo.Rectangle{X: -1920, Y: 25, Width: 1920, Height: 1055}}
	for _, tc := range []struct {
		name     string
		cursor   mygo.Point
		tray     mygo.Rectangle
		displays []mygo.Display
		want     mygo.Point
	}{
		{"bottom display, icon there", bottomPress, bottomIcon, stacked, under(25)},
		// Chris's report: the icon's bounds name the display above.
		{"bottom display, icon reported on the top one", bottomPress, topIcon, stacked, under(25)},
		{"top display, icon there", topPress, topIcon, stacked, under(-1415)},
		{"top display, icon reported on the bottom one", topPress, bottomIcon, stacked, under(-1415)},
		{"no icon bounds", bottomPress, mygo.Rectangle{}, stacked, under(25)},
		{"left edge", mygo.Point{X: 10, Y: 12}, mygo.Rectangle{}, stacked, mygo.Point{X: popoverMargin, Y: 25 + popoverGap}},
		{"right edge", mygo.Point{X: 2555, Y: 12}, mygo.Rectangle{X: 2540, Y: 0, Width: 20, Height: 24}, stacked,
			mygo.Point{X: 2560 - w - popoverMargin, Y: 25 + popoverGap}},
		{"right edge of the top display", mygo.Point{X: 2555, Y: -1430}, mygo.Rectangle{}, stacked,
			mygo.Point{X: 2560 - w - popoverMargin, Y: -1415 + popoverGap}},
		{"display at negative x", mygo.Point{X: -100, Y: 10}, mygo.Rectangle{X: -115, Y: 0, Width: 30, Height: 24}, []mygo.Display{bottomDisplay, left},
			mygo.Point{X: -w - popoverMargin, Y: 25 + popoverGap}},
		{"taskbar at the foot", mygo.Point{X: 1712, Y: 1060}, mygo.Rectangle{X: 1700, Y: 1040, Width: 24, Height: 40}, []mygo.Display{taskbarDisplay},
			mygo.Point{X: 1712 - w/2, Y: 1032 - h - popoverGap}},
		{"taskbar at the foot, no icon bounds", mygo.Point{X: 1712, Y: 1060}, mygo.Rectangle{}, []mygo.Display{taskbarDisplay},
			mygo.Point{X: 1712 - w/2, Y: 1032 - h - popoverGap}},
		// Where none fits, it centers across and keeps to the top.
		{"display narrower than the popover", mygo.Point{X: 3100, Y: 10}, mygo.Rectangle{}, []mygo.Display{bottomDisplay, narrow},
			mygo.Point{X: 3000 + (300-w)/2, Y: 25}},
		// Between displays, the nearest one has it.
		{"pointer off every display", mygo.Point{X: 2700, Y: 100}, mygo.Rectangle{}, stacked, mygo.Point{X: 2560 - w - popoverMargin, Y: 25 + popoverGap}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := placePopover(tc.cursor, tc.tray, tc.displays, w, h)
			if !ok || got != tc.want {
				t.Errorf("placePopover = %+v, %v; want %+v", got, ok, tc.want)
			}
		})
	}
	if _, ok := placePopover(bottomPress, bottomIcon, nil, w, h); ok {
		t.Error("placed without displays")
	}
}

// on reports whether the popover at at lies inside d's work area.
func on(at mygo.Point, d mygo.Display) bool {
	wa := d.WorkArea
	return at.X >= wa.X && at.X+PopoverW <= wa.X+wa.Width && at.Y >= wa.Y && at.Y+PopoverH <= wa.Y+wa.Height
}

// Each open places the popover again, on the display whose icon was
// clicked, before it shows, and touches nothing else: no hub, no Focus
// (Chris, 2026-11-22: it opened on the display above, then kept opening
// where it first had).
func TestTrayPopoverFollowsTheClickedDisplay(t *testing.T) {
	d, l := fakeDesktop(t)
	l.displays = stacked
	at := time.Unix(1_800_000_000, 0)
	click := func(cursor mygo.Point, tray mygo.Rectangle, want mygo.Display, name string) {
		t.Helper()
		l.cursor, l.trayAt = cursor, tray
		l.log.calls = nil
		at = at.Add(time.Second)
		d.trayClicked(at)
		calls := slices.DeleteFunc(slices.Clone(l.log.calls), func(c string) bool { return c == "popover.make" })
		if !slices.Equal(calls, []string{"popover.SetPosition", "popover.Show"}) || !l.pop.visible {
			t.Fatalf("%s: calls %q, visible %v", name, l.log.calls, l.pop.visible)
		}
		if !on(l.pop.at, want) {
			t.Errorf("%s: the popover at %+v is not on display %d", name, l.pop.at, want.ID)
		}
		// The icon's click closes it again.
		at = at.Add(time.Second)
		d.trayClicked(at)
		if l.pop.visible || hubTouched(l.log.calls) || slices.Contains(l.log.calls, "popover.Focus") {
			t.Fatalf("%s: calls %q", name, l.log.calls)
		}
	}
	click(bottomPress, topIcon, bottomDisplay, "bottom, icon reported above")
	click(topPress, topIcon, topDisplay, "top")
	click(bottomPress, bottomIcon, bottomDisplay, "bottom again")
	click(bottomPress, topIcon, bottomDisplay, "bottom, icon reported above again")
	click(topPress, bottomIcon, topDisplay, "top, icon reported below")
}

// ShowWindow(WinPopover) places the popover as it opens, and leaves an open
// one where it is.
func TestShowWindowPopoverPlacesOnlyAsItOpens(t *testing.T) {
	d, l := fakeDesktop(t)
	l.displays, l.cursor, l.trayAt = stacked, topPress, topIcon
	d.ShowWindow(WinPopover, true)
	first := l.pop.at
	if !on(first, topDisplay) {
		t.Fatalf("the popover at %+v is not on the top display", first)
	}
	l.cursor = bottomPress
	d.ShowWindow(WinPopover, true)
	if l.pop.at != first || countCalls(l.log.calls, "popover.SetPosition") != 1 {
		t.Errorf("an open popover moved: %+v, calls %q", l.pop.at, l.log.calls)
	}
	if hubTouched(l.log.calls) || slices.Contains(l.log.calls, "popover.Focus") {
		t.Errorf("calls %q", l.log.calls)
	}
}
