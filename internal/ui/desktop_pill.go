package ui

import (
	"log"
	"math"

	"github.com/egoist/mygo"

	"github.com/watzon/sayso/internal/config"
)

// The pill is a button, so MyGo gives a press on it no window drag
// (DragWindow): Sayso moves the window itself, by the pointer's position
// on the screen, which moving the window does not change.

// pillDragStart is how far, in DIPs, a press of the pill moves before it
// drags the pill: a shorter move is a click.
const pillDragStart = 4

// pillDrag is a press of the pill: where the pointer and the window were as
// it began, and whether it moved the window.
type pillDrag struct {
	pressing, moved bool
	cursor, window  mygo.Point
}

// DragPill follows a press of the pill, which pressed says is down: once
// the pointer moved pillDragStart, the overlay window follows it, and when
// the press ends the pill snaps and its place is saved. It reports whether
// the press moved the window, also in the call that sees it end, so that
// the release is no click.
func (d *Desktop) DragPill(pressed bool) bool {
	g := &d.pill
	ov := d.layer.overlay()
	if ov == nil || d.a.cfg.Overlay.Style == config.OverlayNotch {
		*g = pillDrag{}
		return false
	}
	at := d.layer.pointer()
	switch {
	case pressed && !g.pressing:
		x, y := ov.Position()
		*g = pillDrag{pressing: true, cursor: at, window: mygo.Point{X: x, Y: y}}
	case pressed:
		dx, dy := at.X-g.cursor.X, at.Y-g.cursor.Y
		if !g.moved && dx*dx+dy*dy < pillDragStart*pillDragStart {
			return false
		}
		g.moved = true
		ov.SetPosition(g.window.X+dx, g.window.Y+dy)
	case g.pressing:
		moved := g.moved
		*g = pillDrag{}
		if moved {
			d.dropPill()
		}
		return moved
	}
	return g.moved
}

// dropPill snaps the pill where the drag left it, on the display under its
// card, keeps the card where it is when it goes to the other half of the
// display, and saves the place.
func (d *Desktop) dropPill() {
	ov := d.layer.overlay()
	if ov == nil {
		return
	}
	x, y := ov.Position()
	w, h := d.overlaySize()
	cardH := int(math.Round(float64(d.region.H)))
	cardTop := y + h - overlayMargin - cardH
	if d.a.ov.previewBelow {
		cardTop = y + overlayMargin
	}
	cardBottom := cardTop + cardH
	cx, mid := x+w/2, (cardTop+cardBottom)/2
	disp, ok := displayAt(mygo.Point{X: cx, Y: mid}, d.layer.displays())
	if !ok {
		return
	}
	wa := workArea(disp)
	spot := snapPillSpot(wa, mid < wa.Y+wa.Height/2, cx, cardTop, cardBottom)
	f, above := pillSpotFrame(wa, spot, w, h)
	ov.SetPosition(f.X, f.Y)
	d.placed = overlayPlaced{ok: true, display: disp.ID, frame: f, key: d.overlayKey(), above: above}
	d.a.SetPreviewBelow(above)
	d.pillSpots()[disp.ID] = spot
	if path := d.stateFile(); path != "" {
		if err := savePillSpot(path, disp.ID, spot); err != nil {
			log.Printf("overlay: save the pill's place: %v", err)
		}
	}
}

// pillSpots are the saved places of the pill, read once.
func (d *Desktop) pillSpots() map[int64]pillSpot {
	if d.spots == nil {
		d.spots = map[int64]pillSpot{}
		if path := d.stateFile(); path != "" {
			d.spots = loadPillSpots(path)
		}
	}
	return d.spots
}

func (d *Desktop) stateFile() string {
	if d.a.d.Paths.DataDir == "" {
		return ""
	}
	return d.a.d.Paths.StateFile()
}

// ResetPill forgets the saved places of the pill. With now it goes back to
// the bottom center at once; else it keeps its place until Sayso starts
// again, as Settings says.
func (d *Desktop) ResetPill(now bool) error {
	var err error
	if path := d.stateFile(); path != "" {
		err = forgetPillSpots(path)
	}
	if now {
		d.spots = map[int64]pillSpot{}
		d.placed = overlayPlaced{}
		d.placeOverlay()
	}
	return err
}
