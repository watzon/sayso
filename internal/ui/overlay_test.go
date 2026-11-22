package ui

import (
	"strings"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/i18n"
)

// longSpeech is a dictation of a minute or so: far more than the slip holds.
var longSpeech = strings.Repeat("okay we are so close in terms of the design but the overlay does need a bit of help ", 6)

// recordingWithPreview is an app recording a long dictation, with the live
// preview on, at an overlay size and style.
func recordingWithPreview(t *testing.T, size config.OverlaySize, style config.OverlayStyle) *App {
	t.Helper()
	a := newEnv(t).a
	a.cfg.Overlay.Size, a.cfg.Overlay.Style, a.cfg.Overlay.ShowPreview = size, style, true
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	a.Preview(strings.TrimSpace(longSpeech), "and these are the latest words")
	return a
}

// overlayTester runs the overlay in a window of w by h, and lets the pen
// write all of the preview.
func overlayTester(a *App, w, h int) *mu.Tester {
	tt := mu.NewTester(a.OverlayView, w, h)
	for range 60 {
		tt.Advance(100 * time.Millisecond)
		tt.Frame()
	}
	return tt
}

func accessNamed(t *testing.T, tt *mu.Tester, name string) mu.Rect {
	t.Helper()
	r, ok := controlBounds(tt, name)
	if !ok {
		for _, a := range tt.AccessAll() {
			if a.Name == name {
				return a.Bounds
			}
		}
		t.Fatalf("nothing named %q: %q", name, tt.Texts())
	}
	return r
}

// previewBounds is the box of the slip of the live preview: the status
// that shows the words.
func previewBounds(t *testing.T, tt *mu.Tester) mu.Rect {
	t.Helper()
	for _, a := range tt.AccessAll() {
		if a.Role == mu.RoleStatus && strings.Contains(a.Name, "latest") {
			return a.Bounds
		}
	}
	t.Fatalf("no preview slip: %q", tt.Texts())
	return mu.Rect{}
}

// The live preview of a long dictation stays above the pill at its size: it
// grows up, the window holds all of it and the pill, and the latest words
// show (Rust overlay preview_slip). Chris's overlay: pill, Small.
func TestOverlayLongPreviewFitsTheWindow(t *testing.T) {
	for _, size := range []config.OverlaySize{config.OverlaySmall, config.OverlayMedium, config.OverlayLarge} {
		t.Run(string(size), func(t *testing.T) {
			a := recordingWithPreview(t, size, config.OverlayPill)
			w, h := OverlayWindowSize(a.cfg.Overlay)
			tt := overlayTester(a, w, h)
			tall := overlayTester(a, w, 4*h)
			stop := i18n.T("overlay.a11y.stop")
			pill, pillFree := accessNamed(t, tt, stop), accessNamed(t, tall, stop)
			s, sFree := previewBounds(t, tt), previewBounds(t, tall)
			if pill.H < pillFree.H-0.5 || pill.Y+pill.H > float32(h) {
				t.Errorf("the window (%dx%d) cuts the pill: %+v, unclipped %+v", w, h, pill, pillFree)
			}
			if s.H < sFree.H-0.5 || s.Y < 0 {
				t.Errorf("the window (%dx%d) cuts the slip: %+v, unclipped %+v", w, h, s, sFree)
			}
			// At most three lines: 14 + 3×24 + 14 at Large, shapes scaled by k.
			k := a.cfg.Overlay.Size.Scale()
			if maxH := 28*k + 72*size.TextScale(); s.H > maxH+0.5 {
				t.Errorf("the slip is %v high, more than three lines (%v)", s.H, maxH)
			}
			if s.Y+s.H > pill.Y {
				t.Errorf("the slip %+v is not above the pill %+v", s, pill)
			}
			if !tt.HasText("latest words") {
				t.Errorf("the latest words do not show: %q", tt.Texts())
			}
		})
	}
}

// regionShell records the part of the overlay window that takes the mouse.
type regionShell struct {
	*fakeShell
	region mu.Rect
	calls  int
}

func (s *regionShell) SetOverlayInputRegion(r mu.Rect) { s.region, s.calls = r, s.calls+1 }

// Only the visible card takes the mouse: the rest of the 520×300 window
// lets it through to the app under it (Rust overlay).
func TestOverlayOnlyTheCardTakesTheMouse(t *testing.T) {
	e := newEnv(t)
	a := e.a
	shell := &regionShell{fakeShell: e.shell}
	a.d.Shell = shell
	a.cfg.Overlay.IdlePill, a.cfg.Overlay.Size = true, config.OverlaySmall
	a.cfg.Hotkeys.Toggle = ptr(config.MustHotkey("opt+space"))
	w, h := OverlayWindowSize(a.cfg.Overlay)
	tt := overlayTester(a, w, h)
	start := accessNamed(t, tt, i18n.T("overlay.a11y.start"))
	near := func(r, want mu.Rect) bool {
		d := func(x, y float32) bool { return x-y < 1 && y-x < 1 }
		return d(r.X, want.X) && d(r.Y, want.Y) && d(r.W, want.W) && d(r.H, want.H)
	}
	if !near(shell.region, start) {
		t.Fatalf("region %+v, the idle pill %+v", shell.region, start)
	}
	// Hovered, the pill opens; the region follows it.
	// The view draws only the frames it asks for, as a window does.
	tt.Move(start.X+start.W/2, start.Y+start.H/2)
	for range 10 {
		tt.Advance(50 * time.Millisecond)
		if !tt.FramePending() {
			break
		}
		tt.Frame()
	}
	if open := accessNamed(t, tt, i18n.T("overlay.a11y.start")); !near(shell.region, open) || open.W <= start.W {
		t.Fatalf("region %+v, the open pill %+v (closed %+v)", shell.region, open, start)
	}
	if r := shell.region; r.X < 0 || r.Y < 0 || r.X+r.W > float32(w) || r.Y+r.H > float32(h) || r.W*r.H > float32(w*h)/4 {
		t.Errorf("region %+v in a %dx%d window", r, w, h)
	}
}
