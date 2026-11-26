package ui

import (
	"encoding/json"
	"errors"
	"io/fs"
	"math"
	"os"
	"strconv"

	"github.com/egoist/mygo"
)

// The place the user dropped the pill at, per display, in state.json under
// "pill", in the shape Sayso 0.16 kept it.

// pillSpot is where the pill rests on a display, in DIPs: the card's center
// from the center of the work area (DX), and the card's gap to the bottom
// of the work area or, for a pill in the top half, to its top. Bottom stays
// in the file for a pill in the top half, for a version without Top.
type pillSpot struct {
	DX     float64  `json:"dx"`
	Bottom float64  `json:"bottom"`
	Top    *float64 `json:"top,omitempty"`
}

const (
	// pillSnap is how near, in DIPs, a drop must be to snap to the center
	// or to an edge.
	pillSnap = 48
	// pillEdge is the gap the pill keeps to the sides and the top of the
	// work area; at the bottom it keeps overlayMargin.
	pillEdge = 16
	// pillHalfCard is half the idle card's width, for the side edges.
	pillHalfCard = 30
	// pillIdleCard is the idle card's height, for a spot without Top.
	pillIdleCard = 14
)

// snapToEdge is gap, or rest when gap is within pillSnap of it.
func snapToEdge(gap, rest float64) float64 {
	if gap < rest+pillSnap {
		return rest
	}
	return gap
}

// topGap is the spot's gap to the top of the work area wa, for a pill in
// the top half; ok is false for one in the bottom half. A spot of a
// version without Top gets one.
func (s pillSpot) topGap(wa mygo.Rectangle) (float64, bool) {
	if s.Top != nil {
		return *s.Top, true
	}
	if s.Bottom > float64(wa.Height)/2 {
		return snapToEdge(float64(wa.Height)-s.Bottom-pillIdleCard, pillEdge), true
	}
	return 0, false
}

// snapPillSpot is the spot of a card dropped on the work area wa with its
// center at x and its top and bottom edges at top and bottom, in the top
// half when above, with the snaps to the center and the edges.
func snapPillSpot(wa mygo.Rectangle, above bool, x, top, bottom int) pillSpot {
	half := float64(wa.Width) / 2
	dx := float64(x) - (float64(wa.X) + half)
	if math.Abs(dx) < pillSnap {
		dx = 0
	}
	left, right := -half+pillHalfCard+pillEdge, half-pillHalfCard-pillEdge
	if dx < left+pillSnap {
		dx = left
	}
	if dx > right-pillSnap {
		dx = right
	}
	gapBelow := float64(wa.Y + wa.Height - bottom)
	if !above {
		return pillSpot{DX: dx, Bottom: snapToEdge(gapBelow, overlayMargin)}
	}
	t := snapToEdge(float64(top-wa.Y), pillEdge)
	return pillSpot{DX: dx, Bottom: gapBelow, Top: &t}
}

// pillSpotFrame is the frame of the overlay window, w by h, that puts its
// card at s on the work area wa, and whether the card is in the top half,
// at the top of the window.
func pillSpotFrame(wa mygo.Rectangle, s pillSpot, w, h int) (mygo.Rectangle, bool) {
	x := int(math.Round(float64(wa.X)+float64(wa.Width)/2+s.DX)) - w/2
	if t, ok := s.topGap(wa); ok {
		return mygo.Rectangle{X: x, Y: wa.Y + int(math.Round(t)) - overlayMargin, Width: w, Height: h}, true
	}
	y := wa.Y + wa.Height - int(math.Round(s.Bottom)) + overlayMargin - h
	return mygo.Rectangle{X: x, Y: y, Width: w, Height: h}, false
}

// workArea is the display's work area, or its bounds when it has none.
func workArea(d mygo.Display) mygo.Rectangle {
	if d.WorkArea.Width <= 0 || d.WorkArea.Height <= 0 {
		return d.Bounds
	}
	return d.WorkArea
}

// readState is state.json; a missing or broken file is empty.
func readState(path string) map[string]json.RawMessage {
	state := map[string]json.RawMessage{}
	if data, err := os.ReadFile(path); err == nil {
		if json.Unmarshal(data, &state) != nil || state == nil {
			state = map[string]json.RawMessage{}
		}
	}
	return state
}

func writeState(path string, state map[string]json.RawMessage) error {
	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, append(data, '\n'), 0o644)
}

// loadPillSpots reads the saved spots, by display ID.
func loadPillSpots(path string) map[int64]pillSpot {
	out := map[int64]pillSpot{}
	var spots map[string]pillSpot
	if json.Unmarshal(readState(path)["pill"], &spots) != nil {
		return out
	}
	for k, s := range spots {
		if id, err := strconv.ParseInt(k, 10, 64); err == nil {
			out[id] = s
		}
	}
	return out
}

// savePillSpot saves the spot of one display, keeping the other keys.
func savePillSpot(path string, display int64, s pillSpot) error {
	state := readState(path)
	var spots map[string]pillSpot
	if json.Unmarshal(state["pill"], &spots) != nil || spots == nil {
		spots = map[string]pillSpot{}
	}
	spots[strconv.FormatInt(display, 10)] = s
	raw, err := json.Marshal(spots)
	if err != nil {
		return err
	}
	state["pill"] = raw
	return writeState(path, state)
}

// forgetPillSpots removes every saved spot, keeping the other keys.
func forgetPillSpots(path string) error {
	if _, err := os.Stat(path); errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	state := readState(path)
	if _, ok := state["pill"]; !ok {
		return nil
	}
	delete(state, "pill")
	return writeState(path, state)
}
