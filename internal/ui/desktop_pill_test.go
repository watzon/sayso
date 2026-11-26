package ui

import (
	"os"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo"
	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
)

// pillDesk is a desktop on the fake layer whose overlay shows the idle
// pill at its default place on the left display, and the overlay's view.
type pillDesk struct {
	e  *testEnv
	d  *Desktop
	l  *fakeLayer
	tt *mu.Tester
	// x, y is the middle of the pill in the window.
	x, y float32
}

func newPillDesk(t *testing.T, e *testEnv) *pillDesk {
	t.Helper()
	l := newFakeLayer()
	d := &Desktop{a: e.a, layer: l}
	e.a.d.Shell = d
	e.a.cfg.Overlay.IdlePill, e.a.cfg.Overlay.Size = true, config.OverlaySmall
	w, h := OverlayWindowSize(e.a.cfg.Overlay)
	l.screens = sideBySide
	l.ov = &fakeOverlay{w: w, h: h}
	d.ShowOverlay(true)
	tt := overlayTester(e.a, w, h)
	pill := accessNamed(t, tt, i18n.T("overlay.a11y.start"))
	return &pillDesk{e: e, d: d, l: l, tt: tt, x: pill.X + pill.W/2, y: pill.Y + pill.H/2}
}

// frames lets the view draw what it asks for.
func (p *pillDesk) frames() {
	for range 20 {
		p.tt.Advance(16 * time.Millisecond)
		if !p.tt.FramePending() {
			return
		}
		p.tt.Frame()
	}
}

// drag presses the pill and moves the pointer by dx, dy on the screen in
// steps. The window follows the pointer, so in the window the pointer stays
// on the pill.
func (p *pillDesk) drag(dx, dy int) {
	at := p.l.ov.at()
	p.l.cursor = mygo.Point{X: at.X + int(p.x), Y: at.Y + int(p.y)}
	p.tt.Move(p.x, p.y)
	p.frames()
	p.tt.Press(p.x, p.y)
	p.tt.Frame()
	const steps = 5
	for i := 1; i <= steps; i++ {
		p.l.cursor.X += dx / steps
		p.l.cursor.Y += dy / steps
		p.tt.Move(p.x+float32(i%2), p.y)
		p.tt.Advance(16 * time.Millisecond)
		p.tt.Frame()
	}
}

func (p *pillDesk) release() {
	p.tt.Release(p.x, p.y)
	p.frames()
}

// Dragging the idle pill moves its window and starts no dictation; a click
// that does not move still starts one (issue 86: the pill could not be
// dragged).
func TestDraggingThePillMovesItAndStartsNoDictation(t *testing.T) {
	p := newPillDesk(t, newEnv(t))
	start := p.l.ov.at()
	p.drag(-300, -200)
	if got, want := p.l.ov.at(), (mygo.Point{X: start.X - 300, Y: start.Y - 200}); got != want {
		t.Fatalf("while dragged the overlay is at %+v, want %+v (from %+v)", got, want, start)
	}
	p.release()
	if n := p.e.dict.count("toggle"); n != 0 {
		t.Fatalf("the drag toggled dictation %d times", n)
	}
	if got, want := p.l.ov.at(), (mygo.Point{X: start.X - 300, Y: start.Y - 200}); got != want {
		t.Fatalf("dropped, the overlay is at %+v, want %+v", got, want)
	}
	moved := p.l.ov.at()
	p.tt.ClickAt(p.x, p.y)
	p.frames()
	if n := p.e.dict.count("toggle"); n != 1 {
		t.Fatalf("a click toggled dictation %d times, want once", n)
	}
	if p.l.ov.at() != moved {
		t.Fatalf("a click moved the overlay from %+v to %+v", moved, p.l.ov.at())
	}
}

// A drag goes on while the pointer runs ahead of the window, off the pill,
// and its release there and back on the pill clicks nothing.
func TestAPillDragOffThePillStaysADrag(t *testing.T) {
	p := newPillDesk(t, newEnv(t))
	start := p.l.ov.at()
	p.drag(-300, -200)
	p.l.cursor.X -= 100
	p.tt.Move(p.x-100, p.y)
	p.tt.Advance(16 * time.Millisecond)
	p.tt.Frame()
	if got, want := p.l.ov.at(), (mygo.Point{X: start.X - 400, Y: start.Y - 200}); got != want {
		t.Fatalf("off the pill the overlay is at %+v, want %+v", got, want)
	}
	p.tt.Move(p.x, p.y)
	p.tt.Frame()
	p.release()
	if n := p.e.dict.count("toggle"); n != 0 {
		t.Fatalf("the drag toggled dictation %d times", n)
	}
}

// A press that moves less than the drag threshold is a click.
func TestAShakyClickOnThePillIsAClick(t *testing.T) {
	p := newPillDesk(t, newEnv(t))
	start := p.l.ov.at()
	p.drag(2, 1)
	p.release()
	if n := p.e.dict.count("toggle"); n != 1 {
		t.Fatalf("toggled %d times, want once", n)
	}
	if p.l.ov.at() != start {
		t.Fatalf("the overlay moved from %+v to %+v", start, p.l.ov.at())
	}
}

// The pill keeps the place it was dropped at across launches, per display,
// until Reset position.
func TestThePillKeepsItsPlace(t *testing.T) {
	e := newEnv(t)
	p := newPillDesk(t, e)
	start := p.l.ov.at()
	p.drag(-300, -200)
	p.release()
	dropped := p.l.ov.at()

	again := newPillDesk(t, e)
	if got := again.l.ov.at(); got != dropped {
		t.Fatalf("after a restart the pill is at %+v, want %+v where it was dropped", got, dropped)
	}
	if err := e.a.ResetPillPosition(); err != nil {
		t.Fatal(err)
	}
	if got := newPillDesk(t, e).l.ov.at(); got != start {
		t.Fatalf("after Reset position the pill starts at %+v, want %+v", got, start)
	}
}

// A drop near the bottom center snaps back to it.
func TestADropNearTheBottomCenterSnaps(t *testing.T) {
	p := newPillDesk(t, newEnv(t))
	start := p.l.ov.at()
	p.drag(30, -20)
	p.release()
	if got := p.l.ov.at(); got != start {
		t.Fatalf("dropped near its default place, the pill is at %+v, want %+v", got, start)
	}
}

// The pill's menu puts it back at the bottom center at once.
func TestThePillMenuResetsItsPosition(t *testing.T) {
	p := newPillDesk(t, newEnv(t))
	start := p.l.ov.at()
	p.drag(-300, -200)
	p.release()
	if err := p.tt.RightClick(i18n.T("overlay.a11y.start")); err != nil {
		t.Fatal(err)
	}
	if err := p.tt.ChooseMenuItem(i18n.T("settings.overlay.reset_position")); err != nil {
		t.Fatalf("%v; menu %q", err, p.tt.Menu())
	}
	p.frames()
	if got := p.l.ov.at(); got != start {
		t.Fatalf("after Reset position the pill is at %+v, want %+v", got, start)
	}
}

// Settings › Overlay › Reset position forgets the saved place and says so;
// the other keys of state.json stay.
func TestSettingsResetPosition(t *testing.T) {
	e := newEnv(t)
	path := e.paths.StateFile()
	if err := os.WriteFile(path, []byte(`{"whats_new_seen": "0.16.0", "pill": {"11": {"dx": -300, "bottom": 216}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	e.a.OpenSettings(SecOverlay)
	// Tall enough to show the row without scrolling.
	tt := mu.NewTester(e.a.HubView, HubWidth, 2*HubHeight)
	press(t, tt, i18n.T("settings.overlay.reset_position"))
	tt.Frame()
	if !tt.HasText(i18n.T("settings.overlay.reset_done")) {
		t.Errorf("no note after the reset: %q", tt.Texts())
	}
	if spots := loadPillSpots(path); len(spots) != 0 {
		t.Errorf("spots after the reset: %+v", spots)
	}
	if data, _ := os.ReadFile(path); !strings.Contains(string(data), "whats_new_seen") {
		t.Errorf("the reset dropped the other keys: %s", data)
	}
}

// A spot saved by Sayso 0.16, before it kept a top gap, is still read: one
// high on the display goes to the top half.
func TestPillSpotOfAnOlderVersion(t *testing.T) {
	wa := mygo.Rectangle{Y: 30, Width: 1440, Height: 870}
	f, above := pillSpotFrame(wa, pillSpot{Bottom: 814}, 390, 225)
	if !above || f.Y != 30+pillEdge-overlayMargin {
		t.Fatalf("frame %+v above %v, want the card %d below the top", f, above, pillEdge)
	}
	if _, above := pillSpotFrame(wa, pillSpot{Bottom: 16}, 390, 225); above {
		t.Fatal("a spot at the bottom went to the top half")
	}
}
