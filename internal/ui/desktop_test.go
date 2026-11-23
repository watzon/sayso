package ui

import (
	"runtime"
	"slices"
	"testing"
	"time"

	"github.com/egoist/mygo"
)

// layerLog records the calls a fake layer gets, in order: "hub.Show",
// "popover.Focus", "dock", "giveBack".
type layerLog struct{ calls []string }

func (l *layerLog) add(s string) { l.calls = append(l.calls, s) }

// fakeWin is a window of the fake layer.
type fakeWin struct {
	name    string
	log     *layerLog
	visible bool
	at      mygo.Point // where SetPosition last put it
}

func (w *fakeWin) Show()             { w.log.add(w.name + ".Show"); w.visible = true }
func (w *fakeWin) Focus()            { w.log.add(w.name + ".Focus"); w.visible = true }
func (w *fakeWin) Hide()             { w.log.add(w.name + ".Hide"); w.visible = false }
func (w *fakeWin) IsVisible() bool   { return w.visible }
func (w *fakeWin) IsDestroyed() bool { return false }
func (w *fakeWin) SetPosition(x, y int) {
	w.log.add(w.name + ".SetPosition")
	w.at = mygo.Point{X: x, Y: y}
}

// fakeLayer is the window system of a desktop with a tray item, whose hub
// is open behind another app.
type fakeLayer struct {
	log      layerLog
	hubWin   *fakeWin
	pop      *fakeWin
	made     bool
	noTray   bool
	gaveBack []time.Duration
	// cursor, trayAt and displays are the popover's anchor; with no
	// displays it is not placed.
	cursor  mygo.Point
	trayAt  mygo.Rectangle
	screens []mygo.Display
	// ov is the overlay window; nil has none.
	ov *fakeOverlay
}

func newFakeLayer() *fakeLayer {
	l := &fakeLayer{}
	l.hubWin = &fakeWin{name: "hub", log: &l.log, visible: true}
	l.pop = &fakeWin{name: "popover", log: &l.log}
	return l
}

func (l *fakeLayer) hub() layerWindow { return l.hubWin }
func (l *fakeLayer) popover(make bool) layerWindow {
	if l.noTray || !l.made && !make {
		return nil
	}
	if !l.made {
		l.made = true
		l.log.add("popover.make")
	}
	return l.pop
}
func (l *fakeLayer) showInDock() { l.log.add("dock") }
func (l *fakeLayer) popoverAnchor() (mygo.Point, mygo.Rectangle, []mygo.Display) {
	return l.cursor, l.trayAt, l.screens
}
func (l *fakeLayer) overlay() overlayWindow {
	if l.ov == nil {
		return nil
	}
	return l.ov
}
func (l *fakeLayer) displays() []mygo.Display { return l.screens }
func (l *fakeLayer) giveBack(d time.Duration) {
	l.log.add("giveBack")
	l.gaveBack = append(l.gaveBack, d)
}

func fakeDesktop(t *testing.T) (*Desktop, *fakeLayer) {
	e := newEnv(t)
	l := newFakeLayer()
	return &Desktop{a: e.a, layer: l}, l
}

// hubTouched reports a call that would bring the hub forward or activate
// Sayso: any call on the hub, or putting Sayso in the Dock.
func hubTouched(calls []string) bool {
	for _, c := range calls {
		if c == "dock" || len(c) > 4 && c[:4] == "hub." {
			return true
		}
	}
	return false
}

// A click on the menu bar icon shows the popover and nothing else: it
// never shows, focuses or activates the hub, and it never asks the
// popover for Focus, which activates the app (Hub behind another app,
// issue 2).
func TestTrayClickShowsOnlyThePopover(t *testing.T) {
	d, l := fakeDesktop(t)
	at := time.Unix(1_800_000_000, 0)
	d.trayClicked(at)
	if !l.pop.visible || !slices.Equal(l.log.calls, []string{"popover.make", "popover.Show"}) {
		t.Fatalf("calls %q, popover visible %v", l.log.calls, l.pop.visible)
	}
	// A second click closes it, as a menu bar menu, and gives the
	// keyboard back to the app the user came from; the hub stays behind.
	d.trayClicked(at.Add(time.Second))
	if l.pop.visible || hubTouched(l.log.calls) || !slices.Contains(l.log.calls, "giveBack") {
		t.Fatalf("calls %q, popover visible %v", l.log.calls, l.pop.visible)
	}
	// Opening it again makes no second popover.
	d.trayClicked(at.Add(2 * time.Second))
	if !l.pop.visible || hubTouched(l.log.calls) || countCalls(l.log.calls, "popover.make") != 1 {
		t.Fatalf("calls %q", l.log.calls)
	}
}

// ShowWindow(WinPopover) takes the same path: Show alone.
func TestShowWindowPopoverNeverFocuses(t *testing.T) {
	d, l := fakeDesktop(t)
	d.ShowWindow(WinPopover, true)
	if slices.Contains(l.log.calls, "popover.Focus") || hubTouched(l.log.calls) || !l.pop.visible {
		t.Fatalf("calls %q", l.log.calls)
	}
	d.ShowWindow(WinPopover, false)
	if l.pop.visible || hubTouched(l.log.calls) {
		t.Fatalf("calls %q", l.log.calls)
	}
}

// "Open Sayso" closes the popover keeping the keyboard in Sayso, then
// brings the hub in front and focuses it (PopoverView's open action).
func TestOpenSaysoFromThePopoverFocusesTheHub(t *testing.T) {
	d, l := fakeDesktop(t)
	d.a.d.Shell = d
	d.a.cfg.General.DockIconWithHub = false
	d.trayClicked(time.Unix(1_800_000_000, 0))
	d.a.d.Shell.ClosePopover(true)
	d.a.d.Shell.ShowHub()
	want := []string{"popover.make", "popover.Show", "popover.Hide", "hub.Show", "hub.Focus"}
	if !slices.Equal(l.log.calls, want) {
		t.Fatalf("calls %q, want %q", l.log.calls, want)
	}
	// The Dock icon comes with the hub when the user asked for that.
	l.log.calls = nil
	d.a.cfg.General.DockIconWithHub = true
	d.ShowHub()
	if !slices.Contains(l.log.calls, "hub.Focus") || runtime.GOOS == "darwin" && l.log.calls[0] != "dock" {
		t.Fatalf("calls %q", l.log.calls)
	}
}

// A dismissal (a press outside it, or the loss of the keyboard) gives the
// keyboard back after the press reached what is under it, unless the
// press landed in a window of Sayso; the hub is never shown for it.
func TestPopoverDismissalGivesTheKeyboardBack(t *testing.T) {
	d, l := fakeDesktop(t)
	at := time.Now() // popoverClosed notes the close at time.Now
	d.trayClicked(at)
	d.popoverClosed(exitDismissed, false)
	if len(l.gaveBack) != 1 || l.gaveBack[0] != 120*time.Millisecond || hubTouched(l.log.calls) {
		t.Fatalf("give back %v, calls %q", l.gaveBack, l.log.calls)
	}
	// The icon's click that follows the press that dismissed it does not
	// open it again.
	l.pop.visible = false
	d.trayClicked(at.Add(100 * time.Millisecond))
	if l.pop.visible {
		t.Fatal("the click that dismissed the popover opened it again")
	}
	// A press in the hub keeps Sayso active and gives nothing back.
	d2, l2 := fakeDesktop(t)
	d2.trayClicked(at)
	d2.popoverClosed(exitDismissed, true)
	if len(l2.gaveBack) != 0 || hubTouched(l2.log.calls) {
		t.Fatalf("give back %v, calls %q", l2.gaveBack, l2.log.calls)
	}
}

// Without a tray item there is no popover, and a click does nothing.
func TestTrayClickWithoutATrayItem(t *testing.T) {
	d, l := fakeDesktop(t)
	l.noTray = true
	d.trayClicked(time.Unix(1_800_000_000, 0))
	d.ClosePopover(false)
	if len(l.log.calls) != 0 {
		t.Fatalf("calls %q", l.log.calls)
	}
}

// The popover must be a focusable flyout but not a system popover: MyGo
// shows an NSPopover of a tray item with activateIgnoringOtherApps, which
// brings the hub in front of the app the user is in. Its own flyout panel
// is nonactivating: it takes the keyboard and leaves the app inactive.
func TestPopoverIsANonactivatingFlyout(t *testing.T) {
	o := popoverOptions(nil, nil)
	if o.Popover {
		t.Fatal("the tray popover is an NSPopover, which activates Sayso and raises the hub")
	}
	if !o.Focusable {
		t.Fatal("the popover does not take the keyboard (Escape, Tab, Enter) or close as a menu does")
	}
	if o.Width != PopoverW || o.Height != PopoverH || !o.Shadow || o.CornerRadius != 12 {
		t.Errorf("look: %+v", o)
	}
}

func countCalls(calls []string, c string) int {
	n := 0
	for _, x := range calls {
		if x == c {
			n++
		}
	}
	return n
}
