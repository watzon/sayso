package ui

import (
	"log"
	"time"

	"github.com/egoist/mygo"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/platform"
)

// The overlay's display follows the keyboard focus, never the pointer:
// moving the mouse to another display leaves it where it is, and focusing a
// window or an input on another display moves it there. When the focus is
// nowhere a dictation could type (the desktop, Sayso's tray popover or its
// overlay), it stays on the last display that had the focus. The primary
// display holds it only before any focus is known, when that display is
// gone, or with Follow display off.

// overlayWindow is what the overlay's placement asks of its window;
// *mygo.Window has it.
type overlayWindow interface {
	IsVisible() bool
	ShowInactive()
	Hide()
	// SetSize and SetPosition are in DIPs of the screen, where y grows down
	// from the top of the primary display.
	SetSize(w, h int)
	SetPosition(x, y int)
	Position() (x, y int)
	SetIgnoreMouseEvents(ignore bool)
	SetInputRegion(rects []mygo.Rectangle)
}

// overlayKey is what the overlay's frame depends on besides the display:
// a change of a setting in it places the overlay again.
type overlayKey struct {
	follow bool
	style  config.OverlayStyle
	size   config.OverlaySize
}

// overlayPlaced is where placeOverlay last put the overlay.
type overlayPlaced struct {
	ok      bool
	display int64
	frame   mygo.Rectangle
	key     overlayKey
	// above: the card is in the top half of the display, at the top of
	// the window, and the preview opens below it.
	above bool
}

// focusState is the display the keyboard focus was last seen on.
type focusState struct {
	known   bool
	display int64
	// The tracker's loop: req asks it to look again; stop ends it.
	req  chan struct{}
	stop func()
}

// focusSettle lets a burst of focus events (an app switch fires several)
// end before the tracker is asked where the focus is.
const focusSettle = 30 * time.Millisecond

// focusDisplay is the display that holds the focus frame f: the one it
// names, else the one with the most of its rectangle, else the nearest to
// its center. ok is false without displays or a frame.
func focusDisplay(f platform.FocusFrame, displays []mygo.Display) (int64, bool) {
	if f.Display != 0 {
		for _, d := range displays {
			if d.ID == f.Display {
				return d.ID, true
			}
		}
	}
	if f.Rect.Empty() || len(displays) == 0 {
		return 0, false
	}
	best, bestArea := -1, 0
	for i, d := range displays {
		r := toDIPs(f, d)
		if a := overlapArea(r, d.Bounds); a > bestArea {
			best, bestArea = i, a
		}
	}
	if best >= 0 {
		return displays[best].ID, true
	}
	// Off every display: the nearest to its center. Pixels need a scale,
	// so each display is measured in its own.
	bestDist := -1
	for i, d := range displays {
		r := toDIPs(f, d)
		c := mygo.Point{X: r.X + r.Width/2, Y: r.Y + r.Height/2}
		b := d.Bounds
		dx := max(b.X-c.X, 0, c.X-(b.X+b.Width-1))
		dy := max(b.Y-c.Y, 0, c.Y-(b.Y+b.Height-1))
		if dist := dx*dx + dy*dy; bestDist < 0 || dist < bestDist {
			best, bestDist = i, dist
		}
	}
	return displays[best].ID, true
}

// toDIPs is the frame's rectangle in the space of d.
func toDIPs(f platform.FocusFrame, d mygo.Display) mygo.Rectangle {
	r := mygo.Rectangle{X: f.Rect.X, Y: f.Rect.Y, Width: f.Rect.Width, Height: f.Rect.Height}
	if !f.Pixels || d.ScaleFactor <= 0 || d.ScaleFactor == 1 {
		return r
	}
	s := d.ScaleFactor
	return mygo.Rectangle{X: int(float64(r.X) / s), Y: int(float64(r.Y) / s), Width: int(float64(r.Width) / s), Height: int(float64(r.Height) / s)}
}

func overlapArea(a, b mygo.Rectangle) int {
	w := min(a.X+a.Width, b.X+b.Width) - max(a.X, b.X)
	h := min(a.Y+a.Height, b.Y+b.Height) - max(a.Y, b.Y)
	if w <= 0 || h <= 0 {
		return 0
	}
	return w * h
}

// overlayDisplay is the display the overlay goes on: the focused one when
// follow is on and it is still connected, else the primary one. ok is
// false without displays.
func overlayDisplay(displays []mygo.Display, follow bool, focus focusState) (mygo.Display, bool) {
	if len(displays) == 0 {
		return mygo.Display{}, false
	}
	if follow && focus.known {
		for _, d := range displays {
			if d.ID == focus.display {
				return d, true
			}
		}
	}
	for _, d := range displays {
		if d.Primary {
			return d, true
		}
	}
	return displays[0], true
}

// overlayFrame is the overlay window's frame on disp: the notch at the top
// center of the display, the pill at the bottom center of its work area.
// The card's bottom edge is overlayMargin inside the window, and sits that
// far above the bottom of the work area (Rust placement DEFAULT_BOTTOM).
func overlayFrame(disp mygo.Display, style config.OverlayStyle, w, h int) mygo.Rectangle {
	if style == config.OverlayNotch {
		b := disp.Bounds
		return mygo.Rectangle{X: b.X + (b.Width-w)/2, Y: b.Y, Width: w, Height: h}
	}
	wa := disp.WorkArea
	if wa.Width <= 0 || wa.Height <= 0 {
		wa = disp.Bounds
	}
	return mygo.Rectangle{X: wa.X + (wa.Width-w)/2, Y: wa.Y + wa.Height - h, Width: w, Height: h}
}

func (d *Desktop) overlayKey() overlayKey {
	o := d.a.cfg.Overlay
	return overlayKey{follow: o.FollowDisplay, style: o.Style, size: o.Size}
}

// placeOverlay puts the overlay on its display (overlayDisplay). It moves
// the window only when the display, the frame or the size changed, or when
// the window is no longer on that display (the user dragged it there): a
// pill dragged along its own display stays where the user put it.
func (d *Desktop) placeOverlay() {
	ov := d.layer.overlay()
	if ov == nil {
		return
	}
	displays := d.layer.displays()
	disp, ok := overlayDisplay(displays, d.a.cfg.Overlay.FollowDisplay, d.focus)
	if !ok {
		return
	}
	w, h := d.overlaySize()
	f := overlayFrame(disp, d.a.cfg.Overlay.Style, w, h)
	above := f.Y < disp.Bounds.Y+disp.Bounds.Height/2
	if spot, ok := d.pillSpots()[disp.ID]; ok && d.a.cfg.Overlay.Style != config.OverlayNotch {
		f, above = pillSpotFrame(workArea(disp), spot, w, h)
	}
	key := d.overlayKey()
	p := d.placed
	if p.ok && p.display == disp.ID && p.frame == f && d.overlayOn(ov, disp.ID, w, h, displays) {
		d.placed.key = key
		return
	}
	if !p.ok || p.frame.Width != w || p.frame.Height != h {
		ov.SetSize(w, h)
	}
	ov.SetPosition(f.X, f.Y)
	d.placed = overlayPlaced{ok: true, display: disp.ID, frame: f, key: key, above: above}
	d.a.SetPreviewBelow(above)
}

// overlayOn reports whether the overlay window, w by h, is on the display id.
func (d *Desktop) overlayOn(ov overlayWindow, id int64, w, h int, displays []mygo.Display) bool {
	x, y := ov.Position()
	at, ok := displayAt(mygo.Point{X: x + w/2, Y: y + h/2}, displays)
	return ok && at.ID == id
}

// notePillHalf tells the pill which half of its display it is in, for the
// preview to open toward the middle.
func (d *Desktop) notePillHalf() {
	ov := d.layer.overlay()
	if ov == nil || d.pill.pressing {
		return
	}
	x, y := ov.Position()
	if p := d.placed; p.ok && x == p.frame.X && y == p.frame.Y {
		d.a.SetPreviewBelow(p.above)
		return
	}
	disp, ok := displayAt(mygo.Point{X: x, Y: y}, d.layer.displays())
	if !ok {
		return
	}
	d.a.SetPreviewBelow(y < disp.Bounds.Y+disp.Bounds.Height/2)
}

// ShowOverlay shows the overlay without taking the keyboard, or hides it.
// It places the overlay as it appears, as a dictation starts (the focus
// may be on another display than when the idle pill showed), and after a
// change of a setting it depends on; and it asks the tracker again for the
// first two, in case a focus change was missed.
func (d *Desktop) ShowOverlay(show bool) {
	ov := d.layer.overlay()
	if ov == nil {
		return
	}
	recording := d.a.state.Kind == dictation.StateRecording
	started := recording && !d.wasRecording
	d.wasRecording = recording
	if !show {
		ov.Hide()
		return
	}
	appearing := !ov.IsVisible()
	if appearing || started || !d.placed.ok || d.placed.key != d.overlayKey() {
		d.placeOverlay()
	}
	if appearing || started {
		d.requestFocus()
	}
	ov.SetIgnoreMouseEvents(d.a.OverlayClickThrough())
	d.applyInputRegion()
	ov.ShowInactive()
}

// focusSeen takes where the tracker found the focus. ok false (the
// desktop, the popover, the overlay, nothing) keeps the last display. A
// visible overlay moves at once when the display changed; a hidden one
// only learns it, and opens there.
func (d *Desktop) focusSeen(f platform.FocusFrame, ok bool) {
	if !ok {
		return
	}
	id, ok := focusDisplay(f, d.layer.displays())
	if !ok {
		return
	}
	moved := !d.focus.known || d.focus.display != id
	d.focus.known, d.focus.display = true, id
	if !moved || !d.a.cfg.Overlay.FollowDisplay {
		return
	}
	if ov := d.layer.overlay(); ov != nil && ov.IsVisible() {
		d.placeOverlay()
	}
}

// displaysChanged forgets a focused display that was disconnected, and
// places a visible overlay again: on the primary display for a lost one,
// or for a work area that changed.
func (d *Desktop) displaysChanged() {
	if d.focus.known {
		found := false
		for _, disp := range d.layer.displays() {
			found = found || disp.ID == d.focus.display
		}
		if !found {
			d.focus.known = false
		}
	}
	if ov := d.layer.overlay(); ov != nil && ov.IsVisible() {
		d.placeOverlay()
	}
	d.requestFocus()
}

func (d *Desktop) tracker() platform.FocusTracker {
	if d.focusTracker != nil {
		return d.focusTracker
	}
	if d.a.d.Platform != nil {
		return d.a.d.Platform.Focus
	}
	return nil
}

// startFocus starts following the focus: a loop that asks the tracker
// where the focus is, off the main thread, whenever its watcher or the
// overlay asks, and hands the answer to focusSeen on the main thread.
func (d *Desktop) startFocus() {
	tr := d.tracker()
	if tr == nil || d.focus.req != nil {
		return
	}
	req, quit := make(chan struct{}, 1), make(chan struct{})
	d.focus.req = req
	go focusLoop(tr, req, quit, func(f platform.FocusFrame, ok bool) {
		d.a.post(func() { d.focusSeen(f, ok) })
	})
	stopWatch, err := tr.Watch(func() {
		select {
		case req <- struct{}{}:
		default:
		}
	})
	if err != nil {
		// The overlay still moves as it appears and as a dictation starts.
		log.Printf("focus: %v", err)
		stopWatch = func() {}
	}
	d.focus.stop = func() {
		stopWatch()
		close(quit)
	}
	d.requestFocus()
}

// stopFocus stops following the focus; it may be called again.
func (d *Desktop) stopFocus() {
	if d.focus.stop != nil {
		d.focus.stop()
		d.focus.stop = nil
	}
}

// requestFocus asks the focus loop to look again.
func (d *Desktop) requestFocus() {
	if d.focus.req == nil || d.focus.stop == nil {
		return
	}
	select {
	case d.focus.req <- struct{}{}:
	default:
	}
}

// focusLoop answers each request with where the focus is, after the burst
// of requests settles, until quit closes.
func focusLoop(tr platform.FocusTracker, req <-chan struct{}, quit <-chan struct{}, seen func(platform.FocusFrame, bool)) {
	settle := time.NewTimer(time.Hour)
	settle.Stop()
	pending := false
	for {
		select {
		case <-quit:
			settle.Stop()
			return
		case <-req:
			// The first request of a burst starts the wait; the rest join
			// it, so a steady stream of focus events still gets answers.
			if !pending {
				pending = true
				settle.Reset(focusSettle)
			}
			continue
		case <-settle.C:
			pending = false
		}
		f, ok := tr.Focused()
		select {
		case <-quit:
			return
		default:
		}
		seen(f, ok)
	}
}
