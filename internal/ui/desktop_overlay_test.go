package ui

import (
	"sync"
	"testing"
	"time"

	"github.com/egoist/mygo"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/platform"
)

// fakeOverlay is the overlay window of the fake layer.
type fakeOverlay struct {
	visible      bool
	x, y, w, h   int
	sizes, moves int
}

func (o *fakeOverlay) IsVisible() bool                 { return o.visible }
func (o *fakeOverlay) ShowInactive()                   { o.visible = true }
func (o *fakeOverlay) Hide()                           { o.visible = false }
func (o *fakeOverlay) SetSize(w, h int)                { o.w, o.h = w, h; o.sizes++ }
func (o *fakeOverlay) SetPosition(x, y int)            { o.x, o.y = x, y; o.moves++ }
func (o *fakeOverlay) Position() (int, int)            { return o.x, o.y }
func (o *fakeOverlay) SetIgnoreMouseEvents(bool)       {}
func (o *fakeOverlay) SetInputRegion([]mygo.Rectangle) {}

func (o *fakeOverlay) at() mygo.Point { return mygo.Point{X: o.x, Y: o.y} }

// drag moves it as the user does, which SetPosition does not count.
func (o *fakeOverlay) drag(x, y int) { o.x, o.y = x, y }
func (o *fakeOverlay) reset()        { o.sizes, o.moves = 0, 0 }
func (o *fakeOverlay) onDisplay(d mygo.Display) bool {
	return inRect(d.Bounds, mygo.Point{X: o.x + o.w/2, Y: o.y + o.h/2})
}

// Side by side: the primary on the left, another on its right.
var (
	leftDisplay = mygo.Display{ID: 11, Primary: true,
		Bounds: mygo.Rectangle{X: 0, Y: 0, Width: 1920, Height: 1080}, WorkArea: mygo.Rectangle{X: 0, Y: 25, Width: 1920, Height: 1055}}
	rightDisplay = mygo.Display{ID: 12,
		Bounds: mygo.Rectangle{X: 1920, Y: 0, Width: 2560, Height: 1440}, WorkArea: mygo.Rectangle{X: 1920, Y: 0, Width: 2560, Height: 1400}}
	sideBySide = []mygo.Display{leftDisplay, rightDisplay}
)

// Focus frames: a window on each stacked display, and an input in one.
var (
	bottomWindow = platform.FocusFrame{Rect: platform.Rect{X: 200, Y: 300, Width: 1200, Height: 800}}
	topWindow    = platform.FocusFrame{Rect: platform.Rect{X: 200, Y: -1200, Width: 1200, Height: 800}}
	nowhere      = platform.FocusFrame{}
)

// overlayDesktop is a desktop on displays with a visible idle pill.
func overlayDesktop(t *testing.T, displays []mygo.Display) (*Desktop, *fakeLayer, *fakeOverlay) {
	t.Helper()
	d, l := fakeDesktop(t)
	d.a.d.Shell = d
	l.screens = displays
	l.ov = &fakeOverlay{}
	d.a.cfg.Overlay.Style = config.OverlayPill
	d.a.cfg.Overlay.IdlePill = true
	d.a.cfg.Overlay.FollowDisplay = true
	return d, l, l.ov
}

// pillAt is where the pill goes on disp; notchAt the notch.
func pillAt(d *Desktop, disp mygo.Display) mygo.Point {
	w, h := d.overlaySize()
	wa := disp.WorkArea
	return mygo.Point{X: wa.X + (wa.Width-w)/2, Y: wa.Y + wa.Height - h}
}

func notchAt(d *Desktop, disp mygo.Display) mygo.Point {
	w, _ := d.overlaySize()
	b := disp.Bounds
	return mygo.Point{X: b.X + (b.Width-w)/2, Y: b.Y}
}

func setState(d *Desktop, k dictation.StateKind) {
	d.a.state = dictation.State{Kind: k}
	d.a.syncOverlay()
}

func TestFocusDisplay(t *testing.T) {
	retina := []mygo.Display{
		{ID: 1, Primary: true, ScaleFactor: 2, Bounds: mygo.Rectangle{X: 0, Y: 0, Width: 1920, Height: 1080}},
		{ID: 2, ScaleFactor: 2, Bounds: mygo.Rectangle{X: 1920, Y: 0, Width: 1920, Height: 1080}},
	}
	for _, tc := range []struct {
		name     string
		f        platform.FocusFrame
		displays []mygo.Display
		want     int64
		ok       bool
	}{
		{"window on the bottom display", bottomWindow, stacked, 1, true},
		{"window on the top display, negative y", topWindow, stacked, 2, true},
		{"input on the top display", platform.FocusFrame{Rect: platform.Rect{X: 900, Y: -400, Width: 300, Height: 24}}, stacked, 2, true},
		{"window mostly on the top display", platform.FocusFrame{Rect: platform.Rect{X: 0, Y: -900, Width: 800, Height: 1000}}, stacked, 2, true},
		{"window mostly on the bottom display", platform.FocusFrame{Rect: platform.Rect{X: 0, Y: -100, Width: 800, Height: 1000}}, stacked, 1, true},
		{"window on the right display", platform.FocusFrame{Rect: platform.Rect{X: 2500, Y: 100, Width: 900, Height: 700}}, sideBySide, 12, true},
		{"window on the left display", platform.FocusFrame{Rect: platform.Rect{X: 100, Y: 100, Width: 900, Height: 700}}, sideBySide, 11, true},
		{"display named (Windows)", platform.FocusFrame{Display: 12}, sideBySide, 12, true},
		{"display named but gone", platform.FocusFrame{Display: 99}, sideBySide, 0, false},
		{"pixels at scale 2 (X11)", platform.FocusFrame{Rect: platform.Rect{X: 4400, Y: 200, Width: 1600, Height: 1200}, Pixels: true}, retina, 2, true},
		{"off every display: the nearest", platform.FocusFrame{Rect: platform.Rect{X: 5000, Y: 100, Width: 100, Height: 100}}, sideBySide, 12, true},
		{"no frame", nowhere, stacked, 0, false},
		{"no displays", topWindow, nil, 0, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := focusDisplay(tc.f, tc.displays)
			if got != tc.want || ok != tc.ok {
				t.Fatalf("focusDisplay = %d, %v; want %d, %v", got, ok, tc.want, tc.ok)
			}
		})
	}
}

func TestOverlayFrame(t *testing.T) {
	const w, h = 400, 120
	if got := overlayFrame(topDisplay, config.OverlayPill, w, h); got != (mygo.Rectangle{X: 1080, Y: -h, Width: w, Height: h}) {
		t.Errorf("pill on the top display: %+v", got)
	}
	if got := overlayFrame(topDisplay, config.OverlayNotch, w, h); got != (mygo.Rectangle{X: 1080, Y: -1440, Width: w, Height: h}) {
		t.Errorf("notch on the top display: %+v", got)
	}
	if got := overlayFrame(rightDisplay, config.OverlayPill, w, h); got != (mygo.Rectangle{X: 1920 + 1080, Y: 1400 - h, Width: w, Height: h}) {
		t.Errorf("pill on the right display: %+v", got)
	}
	// A display that reports no work area uses its bounds.
	bare := mygo.Display{Bounds: mygo.Rectangle{X: 0, Y: 0, Width: 1000, Height: 800}}
	if got := overlayFrame(bare, config.OverlayPill, w, h); got != (mygo.Rectangle{X: 300, Y: 800 - h, Width: w, Height: h}) {
		t.Errorf("pill without a work area: %+v", got)
	}
}

// The idle pill shows on the primary display before any focus is known;
// moving the pointer to the other display never moves it; focusing a
// window there does, at once, though the pill is already visible.
func TestOverlayFollowsFocusNotThePointer(t *testing.T) {
	d, l, ov := overlayDesktop(t, stacked)
	d.a.syncOverlay()
	if !ov.visible || ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("idle pill at %v visible %v, want %v", ov.at(), ov.visible, pillAt(d, bottomDisplay))
	}
	// The pointer goes to the top display; the focus stays below.
	l.cursor = topPress
	ov.reset()
	d.focusSeen(bottomWindow, true)
	d.a.syncOverlay()
	setState(d, dictation.StateRecording)
	setState(d, dictation.StateIdle)
	if ov.at() != pillAt(d, bottomDisplay) || ov.moves != 0 || ov.sizes != 0 {
		t.Fatalf("the pointer moved the pill: at %v, %d moves", ov.at(), ov.moves)
	}
	// A window on the top display takes the focus.
	d.focusSeen(topWindow, true)
	if ov.at() != pillAt(d, topDisplay) || ov.moves != 1 {
		t.Fatalf("pill at %v after %d moves, want %v", ov.at(), ov.moves, pillAt(d, topDisplay))
	}
	if ov.at().Y >= 0 || !ov.onDisplay(topDisplay) {
		t.Fatalf("pill at %v is not on the top display", ov.at())
	}
	// The focus comes back down.
	d.focusSeen(bottomWindow, true)
	if ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("pill at %v, want %v", ov.at(), pillAt(d, bottomDisplay))
	}
}

// The notch goes to the top center of the focused display's bounds, at
// negative y above the primary one.
func TestNotchFollowsFocusToTheTopDisplay(t *testing.T) {
	d, _, ov := overlayDesktop(t, stacked)
	d.a.cfg.Overlay.Style = config.OverlayNotch
	d.focusSeen(topWindow, true)
	if ov.visible || ov.moves != 0 {
		t.Fatalf("the idle notch showed or moved for a focus change: visible %v, %d moves", ov.visible, ov.moves)
	}
	setState(d, dictation.StateRecording)
	if !ov.visible || ov.at() != notchAt(d, topDisplay) || ov.at().Y != -1440 {
		t.Fatalf("notch at %v visible %v, want %v", ov.at(), ov.visible, notchAt(d, topDisplay))
	}
}

// When the focus is nowhere (the desktop, the tray popover, the overlay),
// the overlay stays on the last display that had it.
func TestOverlayKeepsTheLastDisplayWhenTheFocusIsUnknown(t *testing.T) {
	d, l, ov := overlayDesktop(t, stacked)
	d.a.syncOverlay()
	d.focusSeen(topWindow, true)
	l.cursor = bottomPress // a press on the bottom display's desktop
	ov.reset()
	d.focusSeen(nowhere, false)
	d.ShowWindow(WinPopover, true)
	d.ShowWindow(WinPopover, false)
	d.focusSeen(nowhere, false)
	setState(d, dictation.StateRecording)
	setState(d, dictation.StateIdle)
	if ov.at() != pillAt(d, topDisplay) || ov.moves != 0 {
		t.Fatalf("pill at %v after %d moves, want it kept at %v", ov.at(), ov.moves, pillAt(d, topDisplay))
	}
	// Hidden and shown again, it opens there too.
	d.ShowOverlay(false)
	d.ShowOverlay(true)
	if ov.at() != pillAt(d, topDisplay) {
		t.Fatalf("pill at %v, want %v", ov.at(), pillAt(d, topDisplay))
	}
}

// A disconnected display sends the overlay to the primary one.
func TestOverlayLeavesADisconnectedDisplay(t *testing.T) {
	d, l, ov := overlayDesktop(t, stacked)
	d.a.syncOverlay()
	d.focusSeen(topWindow, true)
	l.screens = []mygo.Display{bottomDisplay}
	d.displaysChanged()
	if ov.at() != pillAt(d, bottomDisplay) || d.focus.known {
		t.Fatalf("pill at %v (focus known %v), want %v", ov.at(), d.focus.known, pillAt(d, bottomDisplay))
	}
	// Plugged back in, the overlay waits for the focus to go there.
	l.screens = stacked
	d.displaysChanged()
	if ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("pill at %v, want %v", ov.at(), pillAt(d, bottomDisplay))
	}
}

// With Follow display off the overlay stays on the primary display, and
// turning it on or off places it at once.
func TestFollowDisplayOff(t *testing.T) {
	d, _, ov := overlayDesktop(t, stacked)
	d.a.cfg.Overlay.FollowDisplay = false
	d.a.syncOverlay()
	d.focusSeen(topWindow, true)
	setState(d, dictation.StateRecording)
	if ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("pill at %v with Follow display off, want %v", ov.at(), pillAt(d, bottomDisplay))
	}
	setState(d, dictation.StateIdle)
	d.a.cfg.Overlay.FollowDisplay = true
	d.a.overlayChanged()
	if ov.at() != pillAt(d, topDisplay) {
		t.Fatalf("turning Follow display on left the pill at %v, want %v", ov.at(), pillAt(d, topDisplay))
	}
	d.a.cfg.Overlay.FollowDisplay = false
	d.a.overlayChanged()
	if ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("turning Follow display off left the pill at %v, want %v", ov.at(), pillAt(d, bottomDisplay))
	}
}

// A hidden overlay (no idle pill) is not shown by a focus change; it opens
// on the focused display as the dictation starts.
func TestHiddenOverlayOpensOnTheFocusedDisplay(t *testing.T) {
	d, _, ov := overlayDesktop(t, sideBySide)
	d.a.cfg.Overlay.IdlePill = false
	d.a.syncOverlay()
	if ov.visible {
		t.Fatal("the overlay shows with no idle pill")
	}
	d.focusSeen(platform.FocusFrame{Rect: platform.Rect{X: 2500, Y: 100, Width: 900, Height: 700}}, true)
	if ov.visible || ov.moves != 0 {
		t.Fatalf("a focus change showed or moved the hidden overlay: visible %v, %d moves", ov.visible, ov.moves)
	}
	setState(d, dictation.StateRecording)
	if !ov.visible || ov.at() != pillAt(d, rightDisplay) {
		t.Fatalf("pill at %v visible %v, want %v", ov.at(), ov.visible, pillAt(d, rightDisplay))
	}
}

// The overlay moves only when its display, frame or size changes: not on
// each state change, nor for a focus change on the same display, and a
// pill dragged along its display stays until the focus leaves it. A
// dictation that starts with the pill dragged to another display brings
// it back to the focused one.
func TestOverlayMovesOnlyWhenItMust(t *testing.T) {
	d, _, ov := overlayDesktop(t, stacked)
	d.a.syncOverlay()
	if ov.sizes != 1 || ov.moves != 1 {
		t.Fatalf("first show: %d sizes, %d moves", ov.sizes, ov.moves)
	}
	ov.reset()
	d.focusSeen(bottomWindow, true)
	d.focusSeen(platform.FocusFrame{Rect: platform.Rect{X: 1500, Y: 500, Width: 300, Height: 30}}, true)
	d.a.showNotice("hello")
	setState(d, dictation.StateRecording)
	setState(d, dictation.StateIdle)
	if ov.moves != 0 || ov.sizes != 0 {
		t.Fatalf("%d moves, %d sizes with nothing to move for", ov.moves, ov.sizes)
	}
	// Dragged along the bottom display: it stays through a dictation.
	ov.drag(100, 1200)
	setState(d, dictation.StateRecording)
	setState(d, dictation.StateIdle)
	if ov.at() != (mygo.Point{X: 100, Y: 1200}) {
		t.Fatalf("the dragged pill went to %v", ov.at())
	}
	// Dragged to the top display while the focus stays below: the next
	// dictation brings it back.
	ov.drag(100, -300)
	setState(d, dictation.StateRecording)
	if ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("pill at %v as the dictation started, want %v", ov.at(), pillAt(d, bottomDisplay))
	}
	// A new size resizes and places it.
	ov.reset()
	d.a.cfg.Overlay.Size = config.OverlaySmall
	d.SetOverlaySize(config.OverlaySmall)
	w, h := d.overlaySize()
	if ov.sizes != 1 || ov.w != w || ov.h != h || ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("after resize: %d sizes, %dx%d at %v", ov.sizes, ov.w, ov.h, ov.at())
	}
}

// The pill tells the preview which half of its display it is in, on the
// display it moved to.
func TestOverlayPreviewHalfOnTheTopDisplay(t *testing.T) {
	d, _, _ := overlayDesktop(t, stacked)
	d.a.cfg.Overlay.Style = config.OverlayNotch
	d.focusSeen(topWindow, true)
	setState(d, dictation.StateRecording)
	if !d.a.ov.previewBelow {
		t.Fatal("the notch at the top of the top display opens its preview upward")
	}
}

// fakeTracker is a FocusTracker whose answers the test sets.
type fakeTracker struct {
	mu      sync.Mutex
	f       platform.FocusFrame
	ok      bool
	asked   int
	changed func()
	stopped bool
}

func (t *fakeTracker) set(f platform.FocusFrame, ok bool) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.f, t.ok = f, ok
}

func (t *fakeTracker) Focused() (platform.FocusFrame, bool) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.asked++
	return t.f, t.ok
}

func (t *fakeTracker) Watch(changed func()) (func(), error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.changed = changed
	return func() {
		t.mu.Lock()
		defer t.mu.Unlock()
		t.stopped = true
	}, nil
}

func (t *fakeTracker) fire() {
	t.mu.Lock()
	fn := t.changed
	t.mu.Unlock()
	fn()
}

// The tracker is asked off the main thread, as the watcher and the
// overlay ask, and its answer reaches the overlay through Post; stopping
// stops the watcher.
func TestFocusTrackerDrivesTheOverlay(t *testing.T) {
	d, _, ov := overlayDesktop(t, stacked)
	posts := make(chan func(), 16)
	d.a.d.Post = func(fn func()) { posts <- fn }
	tr := &fakeTracker{}
	tr.set(bottomWindow, true)
	d.focusTracker = tr
	d.startFocus()
	defer d.stopFocus()
	run := func() {
		t.Helper()
		select {
		case fn := <-posts:
			fn()
		case <-time.After(2 * time.Second):
			t.Fatal("the tracker's answer never came")
		}
	}
	run()
	if !d.focus.known || d.focus.display != bottomDisplay.ID {
		t.Fatalf("focus %+v after start", d.focus)
	}
	d.a.syncOverlay()
	tr.set(topWindow, true)
	tr.fire()
	run()
	if ov.at() != pillAt(d, topDisplay) {
		t.Fatalf("pill at %v after the watcher fired, want %v", ov.at(), pillAt(d, topDisplay))
	}
	// A dictation that starts asks again.
	tr.set(bottomWindow, true)
	setState(d, dictation.StateRecording)
	run()
	if ov.at() != pillAt(d, bottomDisplay) {
		t.Fatalf("pill at %v after the dictation started, want %v", ov.at(), pillAt(d, bottomDisplay))
	}
	d.stopFocus()
	d.stopFocus() // again: nothing
	tr.mu.Lock()
	stopped := tr.stopped
	tr.mu.Unlock()
	if !stopped {
		t.Fatal("stopFocus left the watcher running")
	}
	d.requestFocus() // after stop: nothing, and no panic
}

// A burst of focus events is answered with one question.
func TestFocusLoopCoalescesABurst(t *testing.T) {
	tr := &fakeTracker{}
	req, quit := make(chan struct{}, 1), make(chan struct{})
	seen := make(chan struct{}, 16)
	go focusLoop(tr, req, quit, func(platform.FocusFrame, bool) { seen <- struct{}{} })
	defer close(quit)
	for range 20 {
		select {
		case req <- struct{}{}:
		default:
		}
	}
	select {
	case <-seen:
	case <-time.After(2 * time.Second):
		t.Fatal("no answer")
	}
	time.Sleep(4 * focusSettle)
	tr.mu.Lock()
	asked := tr.asked
	tr.mu.Unlock()
	if asked > 2 {
		t.Fatalf("asked %d times for one burst", asked)
	}
}
