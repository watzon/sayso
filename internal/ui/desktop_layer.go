package ui

import (
	"time"

	"github.com/egoist/mygo"

	"github.com/watzon/sayso/internal/platform"
)

// The window layer under Desktop's tray path: the hub, the tray popover and
// the app's activation. mygoLayer is MyGo's; the tests give a fake that
// records each call, to prove that a click on the tray icon shows the
// popover and nothing else.

// layerWindow is what the tray path asks of a window; *mygo.Window has it.
type layerWindow interface {
	Show()
	Focus()
	Hide()
	IsVisible() bool
	IsDestroyed() bool
	// SetPosition moves the window's top-left corner, in DIPs of the
	// screen, where y grows down from the top of the primary display.
	SetPosition(x, y int)
}

type shellLayer interface {
	// hub is the hub window, nil before Start.
	hub() layerWindow
	// popover is the tray popover; make makes it on first use. Nil
	// without a tray item, or when it is not made and make is false.
	popover(make bool) layerWindow
	// showInDock puts Sayso in the Dock, for the hub (macOS).
	showInDock()
	// giveBack gives the keyboard back to the app the user came from,
	// after delay, if Sayso still has it.
	giveBack(delay time.Duration)
	// popoverAnchor is what the popover is placed by: the pointer, the
	// tray icon's bounds (empty when the system does not say) and the
	// displays.
	popoverAnchor() (cursor mygo.Point, tray mygo.Rectangle, displays []mygo.Display)
	// overlay is the overlay window, nil before Start.
	overlay() overlayWindow
	// displays are the displays, the primary one first.
	displays() []mygo.Display
}

// mygoLayer is the layer of the running app.
type mygoLayer struct{ d *Desktop }

func (l mygoLayer) hub() layerWindow {
	if l.d.hub == nil {
		return nil
	}
	return l.d.hub
}

func (l mygoLayer) popover(make bool) layerWindow {
	w := l.d.wins[WinPopover]
	if (w == nil || w.IsDestroyed()) && make {
		if w = l.d.newPopover(); w != nil {
			l.d.wins[WinPopover] = w
		}
	}
	if w == nil {
		return nil
	}
	return w
}

func (l mygoLayer) popoverAnchor() (mygo.Point, mygo.Rectangle, []mygo.Display) {
	var tray mygo.Rectangle
	if l.d.tray != nil {
		tray = l.d.tray.Bounds()
	}
	return mygo.Screen.CursorScreenPoint(), tray, mygo.Screen.Displays()
}

func (l mygoLayer) overlay() overlayWindow {
	if l.d.overlay == nil {
		return nil
	}
	return l.d.overlay
}

func (mygoLayer) displays() []mygo.Display { return mygo.Screen.Displays() }

func (mygoLayer) showInDock() { mygo.App.SetActivationPolicy(mygo.ActivationPolicyRegular) }

func (mygoLayer) giveBack(delay time.Duration) {
	time.AfterFunc(delay, func() {
		mygo.RunOnMain(func() {
			if mygo.App.IsActive() {
				platform.DeactivateApp()
			}
		})
	})
}

// popoverOptions are the tray popover's flyout options.
//
// It must not be a Popover: MyGo shows an NSPopover of a tray item with
// [NSApp activateIgnoringOtherApps:YES], so that it takes the keyboard,
// and activating Sayso brings its main window, the hub, in front of the
// app the user is in. Without Popover, MyGo draws the flyout in its own
// nonactivating panel, which becomes the key window without activating
// the app: the popover gets the keyboard, and every other window of
// Sayso stays where it was. Focusable keeps it closing as a menu does: on
// a press outside it (another app, the desktop, the menu bar or a window
// of Sayso), when another window takes the keyboard, and on Escape. A
// press on the tray icon is left to the icon's click, which toggles it.
// The view draws its own sheet: Shadow and CornerRadius give it the
// system's shadow around 12 px corners.
func popoverOptions(tray *mygo.Tray, content mygo.Content) mygo.FlyoutOptions {
	return mygo.FlyoutOptions{
		Tray: tray, Placement: mygo.PlacementBottom, Gap: 6, Width: PopoverW, Height: PopoverH,
		Shadow: true, Focusable: true, CornerRadius: 12, Content: content,
	}
}

// popoverGap is the room between the popover and the menu bar, the taskbar
// or the tray icon, and above the foot of the work area; popoverMargin
// keeps it off the work area's sides.
const popoverGap, popoverMargin = 6, 8

// placePopover is where the tray popover, w by h DIPs, goes as it opens:
// on the display the pointer is on, which is the one whose tray icon was
// clicked.
//
// MyGo places a tray flyout once, as it is made, so a popover reopened on
// another display showed where it first opened; and on macOS the icon's
// bounds can name another display's copy of the menu bar, with displays
// stacked one over another. The icon's bounds place it only when they are
// on the pointer's display: under the icon below a menu bar at the top,
// above it over a taskbar at the foot. Otherwise it goes under the
// pointer, at the top or the foot of the display's work area, whichever
// half the pointer is in. It stays inside the work area. ok is false
// without displays, where MyGo's own placement stands.
func placePopover(cursor mygo.Point, tray mygo.Rectangle, displays []mygo.Display, w, h int) (at mygo.Point, ok bool) {
	disp, ok := displayAt(cursor, displays)
	if !ok {
		return mygo.Point{}, false
	}
	b, wa := disp.Bounds, disp.WorkArea
	if wa.Width <= 0 || wa.Height <= 0 {
		wa = b
	}
	mid := b.Y + b.Height/2
	if tray.Width > 0 && tray.Height > 0 && inRect(b, mygo.Point{X: tray.X + tray.Width/2, Y: tray.Y + tray.Height/2}) {
		at.X = tray.X + (tray.Width-w)/2
		if tray.Y+tray.Height/2 < mid {
			at.Y = max(tray.Y+tray.Height, wa.Y) + popoverGap
		} else {
			at.Y = min(tray.Y, wa.Y+wa.Height) - h - popoverGap
		}
	} else {
		at.X = cursor.X - w/2
		if cursor.Y < mid {
			at.Y = wa.Y + popoverGap
		} else {
			at.Y = wa.Y + wa.Height - h - popoverGap
		}
	}
	at.X = clampIn(at.X, wa.X+popoverMargin, wa.X+wa.Width-w-popoverMargin, wa.X+(wa.Width-w)/2)
	at.Y = clampIn(at.Y, wa.Y, wa.Y+wa.Height-h-popoverGap, wa.Y)
	return at, true
}

// displayAt is the display that has p, else the nearest to it (the
// primary one first, as Displays lists it, on a tie).
func displayAt(p mygo.Point, displays []mygo.Display) (mygo.Display, bool) {
	best, bestDist := -1, 0
	for i, d := range displays {
		if inRect(d.Bounds, p) {
			return d, true
		}
		b := d.Bounds
		dx := max(b.X-p.X, 0, p.X-(b.X+b.Width-1))
		dy := max(b.Y-p.Y, 0, p.Y-(b.Y+b.Height-1))
		if dist := dx*dx + dy*dy; best < 0 || dist < bestDist {
			best, bestDist = i, dist
		}
	}
	if best < 0 {
		return mygo.Display{}, false
	}
	return displays[best], true
}

func inRect(r mygo.Rectangle, p mygo.Point) bool {
	return p.X >= r.X && p.X < r.X+r.Width && p.Y >= r.Y && p.Y < r.Y+r.Height
}

// clampIn is v kept in [lo, hi], or fallback where the room is too small
// for any.
func clampIn(v, lo, hi, fallback int) int {
	if hi < lo {
		return fallback
	}
	return min(max(v, lo), hi)
}
