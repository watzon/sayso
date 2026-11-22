package ui

import (
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
)

// countedOverlay runs the overlay in a window of its size and counts the
// frames it builds.
func countedOverlay(a *App) (*mu.Tester, *int) {
	n := new(int)
	w, h := OverlayWindowSize(a.cfg.Overlay)
	tt := mu.NewTester(func(c *mu.Context) { *n++; a.OverlayView(c) }, w, h)
	return tt, n
}

// regionEnv is an app whose shell records the overlay's input region.
func regionEnv(t *testing.T) (*App, *regionShell) {
	e := newEnv(t)
	shell := &regionShell{fakeShell: e.shell}
	e.a.d.Shell = shell
	return e.a, shell
}

// sample advances the tester frame by frame for d and returns get after each.
func sample(tt *mu.Tester, d time.Duration, get func() float32) []float32 {
	var out []float32
	for end := tt.Now().Add(d); tt.Now().Before(end); {
		tt.Advance(frame60)
		out = append(out, get())
	}
	return out
}

// strictlyBetween counts the frames whose value is between from and to,
// away from both: the value moved there over frames, not in one jump.
func strictlyBetween(v []float32, from, to float32) int {
	lo, hi := min(from, to), max(from, to)
	margin := (hi - lo) / 50
	n := 0
	for _, x := range v {
		if x > lo+margin && x < hi-margin {
			n++
		}
	}
	return n
}

// While recording, the overlay draws a frame each display refresh (60 Hz
// in the tester), so the waveform moves at the display's rate. At rest it
// asks for no frame at all.
func TestOverlayFrameCadence(t *testing.T) {
	a, _ := regionEnv(t)
	a.cfg.Overlay.IdlePill = true
	tt, builds := countedOverlay(a)
	tt.Advance(2 * time.Second)
	if tt.FramePending() {
		t.Fatal("the idle overlay at rest asks for frames")
	}
	*builds = 0
	tt.Advance(time.Second)
	if *builds != 0 {
		t.Errorf("idle: %d frames in a second", *builds)
	}

	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	tt.Frame()
	*builds = 0
	for i := range 60 {
		a.Level(float32(i%5) / 5) // 20 ms levels come in between frames
		tt.Advance(time.Second / 60)
	}
	if *builds < 55 {
		t.Errorf("recording: %d frames in a second, want 55 or more", *builds)
	}

	a.StateChanged(dictation.State{Kind: dictation.StateIdle})
	tt.Advance(2 * time.Second)
	*builds = 0
	tt.Advance(time.Second)
	if *builds != 0 || tt.FramePending() {
		t.Errorf("after the recording: %d frames in a second", *builds)
	}
}

// A level redraws no window: the hub has nothing to show for it, and the
// overlay draws its own frames while recording.
func TestLevelRedrawsNoWindow(t *testing.T) {
	e := newEnv(t)
	e.a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	before := e.shell.redraws.Load()
	for range 50 {
		e.a.Level(0.5)
	}
	if n := e.shell.redraws.Load() - before; n != 0 {
		t.Errorf("50 levels redrew the windows %d times", n)
	}
}

// With no idle pill, the card grows in from a dot when the recording
// starts and shrinks back out when it ends; the window hides only then.
// The input region follows the card in every frame.
func TestOverlayPillOpensAndClosesOverFrames(t *testing.T) {
	a, shell := regionEnv(t)
	a.cfg.Overlay.IdlePill = false
	tt, _ := countedOverlay(a)
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	tt.Frame()
	open := sample(tt, time.Second, func() float32 { return shell.region.W })
	full := open[len(open)-1]
	if full < 100 {
		t.Fatalf("open card %v wide", full)
	}
	if n := strictlyBetween(open, 0, full); n < 5 {
		t.Errorf("the card opened in %d frames: %v", n, open[:12])
	}
	a.StateChanged(dictation.State{Kind: dictation.StateIdle})
	tt.Frame()
	if !shell.overlay {
		t.Fatal("the window hid before the card closed")
	}
	shut := sample(tt, time.Second, func() float32 { return shell.region.W })
	if n := strictlyBetween(shut, 0, full); n < 5 {
		t.Errorf("the card closed in %d frames: %v", n, shut[:12])
	}
	if shell.overlay || shell.region.W != 0 {
		t.Errorf("after the close: shown %v, region %+v", shell.overlay, shell.region)
	}
}

// From recording to processing the card eases to its new size, and the
// new content fades in.
func TestOverlayStateChangeEases(t *testing.T) {
	a, shell := regionEnv(t)
	a.cfg.Overlay.IdlePill = true
	tt, _ := countedOverlay(a)
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	tt.Frame()
	tt.Advance(time.Second)
	rec := shell.region.W
	a.StateChanged(dictation.State{Kind: dictation.StateProcessing})
	tt.Frame()
	var fades []float32
	widths := sample(tt, time.Second, func() float32 { fades = append(fades, a.ov.fade); return shell.region.W })
	proc := widths[len(widths)-1]
	if proc == rec {
		t.Fatalf("processing and recording cards are both %v wide", rec)
	}
	if n := strictlyBetween(widths, rec, proc); n < 4 {
		t.Errorf("recording %v to processing %v in %d frames: %v", rec, proc, n, widths[:12])
	}
	if n := strictlyBetween(fades, 0, 1); n < 3 {
		t.Errorf("the content faded in over %d frames: %v", n, fades[:12])
	}
}

// The live preview's slip grows out of the pill, and goes back into it
// with the last words when the recording stops.
func TestOverlaySlipComesInAndGoesOut(t *testing.T) {
	a, _ := regionEnv(t)
	a.cfg.Overlay.IdlePill, a.cfg.Overlay.ShowPreview = true, true
	tt, _ := countedOverlay(a)
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	a.Preview("hello there", "friend")
	tt.Frame()
	height := func() float32 { _, h := a.ov.slipM.size(); return h }
	in := sample(tt, time.Second, height)
	full := in[len(in)-1]
	if full < 20 {
		t.Fatalf("slip %v tall", full)
	}
	if n := strictlyBetween(in, 0, full); n < 5 {
		t.Errorf("the slip came in over %d frames: %v", n, in[:12])
	}
	a.StateChanged(dictation.State{Kind: dictation.StateProcessing})
	tt.Frame()
	if !tt.HasText("friend") {
		t.Errorf("the slip went out without its words: %q", tt.Texts())
	}
	out := sample(tt, time.Second, height)
	if n := strictlyBetween(out, 0, full); n < 5 || !a.ov.slipM.gone() {
		t.Errorf("the slip went out over %d frames: %v", n, out[:12])
	}
}

// The notch's wings open from the camera when the recording starts and
// close into it after; then nothing shows, and the window hides.
func TestOverlayNotchExpandsAndCollapses(t *testing.T) {
	a, shell := regionEnv(t)
	a.cfg.Overlay.Style = config.OverlayNotch
	tt, _ := countedOverlay(a)
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	tt.Frame()
	open := sample(tt, time.Second, func() float32 { return shell.region.W })
	full := open[len(open)-1]
	if full < notchCamera+2*notchWing {
		t.Fatalf("open notch %v wide", full)
	}
	if n := strictlyBetween(open, notchCamera+2*notchShoulder, full); n < 5 {
		t.Errorf("the notch opened in %d frames: %v", n, open[:12])
	}
	a.StateChanged(dictation.State{Kind: dictation.StateIdle})
	tt.Frame()
	shut := sample(tt, time.Second, func() float32 { return a.ov.notch.width() })
	if n := strictlyBetween(shut, 0, shut[0]); n < 5 {
		t.Errorf("the wings closed in %d frames: %v", n, shut[:12])
	}
	if shell.overlay || shell.region.W != 0 {
		t.Errorf("after the close: shown %v, region %+v", shell.overlay, shell.region)
	}
}
