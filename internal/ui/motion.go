package ui

import (
	"math"
	"time"

	mu "github.com/egoist/mygo/ui"
)

// Motion for the overlay: the card eases to each new size instead of a
// jump (Rust overlay/motion.rs, with its constants).
//
// A spring moves one value to its target. A morph is a box whose size and
// opacity follow the natural size of its content: the content is measured
// as it lays out, and the next frames move the box to that size. The box
// clips the content, so a card that gets smaller cuts its old edges and a
// card that gets larger opens around its content.

const (
	// springResponse is the time a spring needs for most of a move, in
	// seconds. Shorter is stiffer.
	springResponse = 0.32
	// springDamping below 1 goes a little past the target, and comes back.
	springDamping = 0.82
	// springRest: a spring at rest is this near its target and this slow.
	springRest = 0.25
	// springMaxStep is the longest step: a frame after a long pause does
	// not jump.
	springMaxStep = 1.0 / 30
	// springSubstep is the step of the integration, for a stable spring at
	// a low frame rate.
	springSubstep = 1.0 / 240
	// fadeIn is the time that the content of a new state takes to fade in.
	fadeIn = 0.16
	// seedHeight is the scale of a card that grows in or shrinks out: a
	// small round dot, a little lower than the card.
	seedHeight = 0.7
)

// spring is one value that moves to its target like a damped spring.
type spring struct {
	value, velocity, target float32
	// rest is the distance and speed under which the spring is at rest.
	rest float32
}

func springAt(v float32) spring { return spring{value: v, target: v, rest: springRest} }

// opacitySpring moves from 0 to 1, so it rests at a finer threshold.
func opacitySpring() spring { return spring{rest: springRest * 0.004} }

// snap goes to the target at once, with no motion.
func (s *spring) snap() { s.value, s.velocity = s.target, 0 }

func (s *spring) moving() bool {
	return abs32(s.value-s.target) > s.rest || abs32(s.velocity) > s.rest*4
}

// step moves the spring on by dt seconds.
func (s *spring) step(dt float32) {
	if !s.moving() {
		s.snap()
		return
	}
	omega := float32(2*math.Pi) / springResponse
	k, damp := omega*omega, 2*springDamping*omega
	for left := max(0, min(dt, springMaxStep)); left > 0; left -= springSubstep {
		h := min(left, springSubstep)
		accel := -k*(s.value-s.target) - damp*s.velocity
		s.velocity += accel * h
		s.value += s.velocity * h
	}
	if !s.moving() {
		s.snap()
	}
}

// move steps the spring, or snaps it with reduce motion.
func (s *spring) move(dt float32, reduce bool) {
	if reduce {
		s.snap()
		return
	}
	s.step(dt)
}

func abs32(v float32) float32 {
	if v < 0 {
		return -v
	}
	return v
}

// motionClock is the time between two frames of a motion. It reads the
// frame clock, so a view built twice in one frame steps by zero.
type motionClock struct{ last time.Time }

// tick is the seconds since the last tick, or 0 at the first.
func (k *motionClock) tick(now time.Time) float32 {
	var dt float32
	if !k.last.IsZero() {
		dt = max(0, float32(now.Sub(k.last).Seconds()))
	}
	k.last = now
	return dt
}

// exitKind is how a morph goes away, and how it comes back.
type exitKind uint8

const (
	// exitShrink shrinks to a small dot and fades: a card.
	exitShrink exitKind = iota
	// exitFold keeps the width and folds to no height: a slip that feeds
	// back in.
	exitFold
	// exitPoint shrinks to a point and fades: a slip that goes back into
	// the pill.
	exitPoint
)

// morph is a box that eases to the size of its content.
type morph struct {
	w, h, o spring
	// natW and natH are the natural size of the content in the last frame;
	// measured is false before the first.
	natW, natH float32
	measured   bool
	present    bool
	exit       exitKind
	clock      motionClock
	// content is the content's box, which says its natural size.
	content mu.Handle
}

func newMorph(exit exitKind) morph {
	return morph{w: springAt(0), h: springAt(0), o: opacitySpring(), exit: exit}
}

// measure takes the natural size of the content. An empty content (a card
// that goes away) keeps the last size, to shrink from. It is true when the
// size changed.
func (m *morph) measure(w, h float32) bool {
	if w <= 0 || h <= 0 {
		return false
	}
	if m.measured && abs32(m.natW-w) <= 0.5 && abs32(m.natH-h) <= 0.5 {
		return false
	}
	m.natW, m.natH, m.measured = w, h, true
	return true
}

// seed is the size that the box grows in from and goes out to.
func (m *morph) seed() (w, h float32) {
	switch m.exit {
	case exitFold:
		return m.natW, 0
	case exitPoint:
		return 0, 0
	}
	return m.natH * seedHeight, m.natH * seedHeight
}

// update shows the content or eases the box out, at the frame time now.
// With reduce, every change is at once. It is true while the box moves or
// waits for its first measure, so the caller asks for another frame.
func (m *morph) update(now time.Time, present, reduce bool) bool {
	dt := m.clock.tick(now)
	// Grow in from the exit size: the first time, and after an exit that
	// ended. An exit that did not end turns back from where it is.
	if present && m.measured && m.o.target == 0 && m.o.value < 0.05 {
		sw, sh := m.seed()
		m.w, m.h = springAt(sw), springAt(sh)
	}
	m.present = present
	var tw, th, to float32
	switch {
	case present && m.measured:
		tw, th, to = m.natW, m.natH, 1
	case present:
		// No measure yet: the first frame lays the content out, the next opens.
	default:
		tw, th = m.seed()
	}
	m.w.target, m.h.target, m.o.target = tw, th, to
	m.w.move(dt, reduce)
	m.h.move(dt, reduce)
	m.o.move(dt, reduce)
	return m.moving() || (present && !m.measured)
}

// moving is true while the box moves to its size, or fades.
func (m *morph) moving() bool { return m.w.moving() || m.h.moving() || m.o.moving() }

// gone is true when the box is away and at rest.
func (m *morph) gone() bool { return !m.present && !m.o.moving() && m.o.value <= 0.01 }

// opacity is the box's opacity in this frame.
func (m *morph) opacity() float32 { return max(0, min(m.o.value, 1)) }

// size is the box's size in this frame.
func (m *morph) size() (w, h float32) { return max(0, m.w.value), max(0, m.h.value) }

// morphAlign is where the content sits in a morph smaller than it.
type morphAlign uint8

const (
	// alignCenter opens the box from its center.
	alignCenter morphAlign = iota
	// alignBottom keeps the content on the box's bottom edge, so it slides
	// out downward.
	alignBottom
)

// morphLayerW and morphLayerH are the room the content lays out in: the
// overlay window, so the content wraps as the window would let it.
const (
	morphLayerW = PillWindowW
	morphLayerH = PillWindowH
)

// morphBox is m's box at this frame's size, with content laid out at its
// natural size inside it and measured. content can be nil: an empty box
// that shrinks away. The caller gives the box its look (fill, corners,
// shadow) on the element returned. It asks for frames while the box moves.
func (a *App) morphBox(c *mu.Context, key string, m *morph, present bool, align morphAlign, content func()) mu.Element {
	if r := m.content.Bounds(c); content != nil && m.measure(r.W, r.H) {
		c.AnimationFrame()
	}
	if m.update(c.Now(), present, stillMotion(c)) {
		c.AnimationFrame()
	}
	w, h := m.size()
	box := mu.Box(a.k(c, key)).Size(w, h).Shrink(0).Clip().Opacity(m.opacity())
	box.Children(func() {
		if content == nil {
			return
		}
		top := (h - morphLayerH) / 2
		if align == alignBottom {
			top = h - morphLayerH
		}
		layer := mu.Column(c).Absolute().Left((w-morphLayerW)/2).Top(top).Size(morphLayerW, morphLayerH).AlignItems(mu.Center).Justify(mu.Center)
		if align == alignBottom {
			layer.Justify(mu.End)
		}
		layer.Children(func() {
			mu.Row(c).Bind(&m.content).Shrink(0).AlignItems(mu.Center).Children(content)
		})
	})
	return box
}

// fade is the opacity of a new state's content, since seconds ago it began.
func fade(since float32, reduce bool) float32 {
	if reduce {
		return 1
	}
	return max(0, min(since/fadeIn, 1))
}

// notchMotion opens the notch's wings from the camera and closes them
// into it (Rust overlay/notch.rs Motion).
type notchMotion struct {
	wing, o spring
	clock   motionClock
}

func newNotchMotion() notchMotion { return notchMotion{wing: springAt(0), o: opacitySpring()} }

// update opens the wings to wing DIPs, or closes them. It is true while
// they move.
func (n *notchMotion) update(now time.Time, open bool, wing float32, reduce bool) bool {
	dt := n.clock.tick(now)
	n.wing.target, n.o.target = 0, 0
	if open {
		n.wing.target, n.o.target = wing, 1
	}
	n.wing.move(dt, reduce)
	n.o.move(dt, reduce)
	return n.wing.moving() || n.o.moving()
}

// gone is true when the notch is closed and at rest.
func (n *notchMotion) gone() bool { return n.o.target == 0 && !n.o.moving() && n.o.value <= 0.01 }

func (n *notchMotion) width() float32   { return max(0, n.wing.value) }
func (n *notchMotion) opacity() float32 { return max(0, min(n.o.value, 1)) }
