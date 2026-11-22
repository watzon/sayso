package ui

import (
	"testing"
	"time"
)

const frame60 = time.Second / 60

func runSpring(s *spring, seconds float32) {
	for t := float32(0); t < seconds; t += 1.0 / 60 {
		s.step(1.0 / 60)
	}
}

// The springs of Rust overlay/motion.rs, with its tests.
func TestSpringComesToRestOnItsTarget(t *testing.T) {
	s := springAt(52)
	s.target = 320
	runSpring(&s, 1.5)
	if s.moving() || s.value != 320 {
		t.Fatalf("value %v moving %v", s.value, s.moving())
	}
}

func TestSpringMovesMostOfTheWayInItsResponseTime(t *testing.T) {
	s := springAt(0)
	s.target = 100
	runSpring(&s, 0.25)
	if s.value <= 80 {
		t.Fatal(s.value)
	}
}

func TestSpringNewTargetKeepsTheMotionSmooth(t *testing.T) {
	s := springAt(0)
	s.target = 100
	runSpring(&s, 0.1)
	before := s.value
	s.target = 0
	s.step(1.0 / 60)
	// It goes on from where it was: no jump to the new target, and the
	// motion carries it on for a moment before it turns.
	if s.value < before || s.value-before >= 10 {
		t.Fatalf("%v -> %v", before, s.value)
	}
}

func TestSpringLongPauseDoesNotJump(t *testing.T) {
	s := springAt(0)
	s.target = 100
	s.step(5)
	if s.value >= 100 {
		t.Fatal(s.value)
	}
}

// frames runs update at 60 Hz from at for d, and returns each frame's
// width, height and opacity.
func morphFrames(m *morph, at *time.Time, d time.Duration, present bool) [][3]float32 {
	var out [][3]float32
	for end := at.Add(d); at.Before(end); *at = at.Add(frame60) {
		m.update(*at, present, false)
		w, h := m.size()
		out = append(out, [3]float32{w, h, m.opacity()})
	}
	return out
}

// between is true when some frame is strictly between from and to in
// field i: the value moved there over frames, not in one jump.
func between(frames [][3]float32, i int, from, to float32) int {
	lo, hi := min(from, to), max(from, to)
	margin := (hi - lo) / 100
	n := 0
	for _, f := range frames {
		if f[i] > lo+margin && f[i] < hi-margin {
			n++
		}
	}
	return n
}

// A card grows in from a dot (a little lower than the card, and faded) to
// its size, and shrinks back out to the dot as it fades: over frames.
func TestMorphCardGrowsInAndShrinksOut(t *testing.T) {
	m := newMorph(exitShrink)
	at := time.Unix(1000, 0)
	// First frame: no measure yet, nothing shows; the next frame opens.
	if !m.update(at, true, false) {
		t.Fatal("a card without a measure asks for no frame")
	}
	m.measure(320, 56)
	open := morphFrames(&m, &at, time.Second, true)
	if w, h, o := open[0][0], open[0][1], open[0][2]; w > 0.7*56+5 || h > 0.7*56+5 || o > 0.3 {
		t.Errorf("first frame %v %v %v: the card should start as a faint dot", w, h, o)
	}
	if n := between(open, 0, 0.7*56, 320); n < 5 {
		t.Errorf("width took %d frames between dot and card: %v", n, open[:10])
	}
	if n := between(open, 2, 0, 1); n < 3 {
		t.Errorf("opacity took %d frames to fade in", n)
	}
	if last := open[len(open)-1]; last != [3]float32{320, 56, 1} || m.moving() {
		t.Errorf("settled at %v", last)
	}
	shut := morphFrames(&m, &at, time.Second, false)
	if n := between(shut, 0, 0.7*56, 320); n < 5 {
		t.Errorf("width took %d frames to shrink: %v", n, shut[:10])
	}
	if !m.gone() {
		t.Errorf("not gone after a second: %v", shut[len(shut)-1])
	}
}

// A card that changes state eases from one size to the next.
func TestMorphEasesBetweenSizes(t *testing.T) {
	m := newMorph(exitShrink)
	at := time.Unix(1000, 0)
	m.update(at, true, false)
	m.measure(320, 56)
	morphFrames(&m, &at, time.Second, true)
	m.measure(220, 56) // recording -> processing
	f := morphFrames(&m, &at, time.Second, true)
	if n := between(f, 0, 220, 320); n < 5 {
		t.Errorf("width jumped: %v", f[:8])
	}
	if f[len(f)-1][0] != 220 {
		t.Errorf("settled at %v", f[len(f)-1])
	}
}

// The notch's slip folds: it keeps its width and its height goes to none.
func TestMorphFoldKeepsTheWidth(t *testing.T) {
	m := newMorph(exitFold)
	at := time.Unix(1000, 0)
	m.update(at, true, false)
	m.measure(317, 60)
	in := morphFrames(&m, &at, time.Second, true)
	if in[0][0] != 317 {
		t.Errorf("a folding slip feeds out at its width, got %v", in[0])
	}
	if n := between(in, 1, 0, 60); n < 5 {
		t.Errorf("height jumped: %v", in[:8])
	}
	out := morphFrames(&m, &at, time.Second, false)
	if n := between(out, 1, 0, 60); n < 5 || out[3][0] != 317 {
		t.Errorf("fold: %v", out[:8])
	}
}

// The pill's slip goes back to a point.
func TestMorphPointShrinksToNothing(t *testing.T) {
	m := newMorph(exitPoint)
	at := time.Unix(1000, 0)
	m.update(at, true, false)
	m.measure(420, 100)
	morphFrames(&m, &at, time.Second, true)
	out := morphFrames(&m, &at, time.Second, false)
	if n := between(out, 0, 0, 420); n < 5 {
		t.Errorf("width jumped: %v", out[:8])
	}
	if w, h := m.size(); w != 0 || h != 0 || !m.gone() {
		t.Errorf("left at %v×%v", w, h)
	}
}

// With reduce motion, every change is at once.
func TestMorphReduceMotionSnaps(t *testing.T) {
	m := newMorph(exitShrink)
	at := time.Unix(1000, 0)
	m.update(at, true, true)
	m.measure(320, 56)
	if m.update(at.Add(frame60), true, true) {
		t.Error("a snapped card asks for frames")
	}
	if w, h := m.size(); w != 320 || h != 56 || m.opacity() != 1 {
		t.Errorf("%v×%v at %v", w, h, m.opacity())
	}
}

// The notch's wings open from the camera and close into it, over frames.
func TestNotchMotionOpensAndCloses(t *testing.T) {
	n := newNotchMotion()
	at := time.Unix(1000, 0)
	var open, shut []float32
	for range 60 {
		n.update(at, true, 76, false)
		open = append(open, n.width())
		at = at.Add(frame60)
	}
	for range 60 {
		n.update(at, false, 76, false)
		shut = append(shut, n.width())
		at = at.Add(frame60)
	}
	mid := func(v []float32) int {
		c := 0
		for _, x := range v {
			if x > 1 && x < 75 {
				c++
			}
		}
		return c
	}
	if mid(open) < 5 || open[len(open)-1] != 76 {
		t.Errorf("open: %v", open[:10])
	}
	if mid(shut) < 5 || !n.gone() {
		t.Errorf("close: %v gone %v", shut[:10], n.gone())
	}
}

// A new state's content fades in over FADE_IN.
func TestFadeIn(t *testing.T) {
	if fade(0, false) != 0 || fade(0.08, false) != 0.5 || fade(0.2, false) != 1 || fade(0, true) != 1 {
		t.Error(fade(0, false), fade(0.08, false), fade(0.2, false), fade(0, true))
	}
}

// The ink flows: with a level each 20 ms (here in bursts of five every
// 100 ms, as a system that hands over several audio frames at once does),
// the playhead moves on each 60 Hz frame by a part of a level, never
// standing still for a frame and never jumping.
func TestWavePlayheadFlowsBetweenLevels(t *testing.T) {
	var w liveWave
	at := time.Unix(1000, 0)
	pushed := 0
	var steps []float64
	last := -1.0
	for i := range 180 { // three seconds
		at = at.Add(frame60)
		for want := (i + 1) * 50 / 60 / 5 * 5; pushed < want; pushed++ {
			w.push(float32(pushed%9) / 9)
		}
		w.advance(at)
		if last >= 0 && i > 60 {
			steps = append(steps, w.head-last)
		}
		last = w.head
	}
	for i, s := range steps {
		if s <= 0 || s > 2 {
			t.Fatalf("frame %d: the playhead moved %.2f levels (all: %.2f)", i, s, steps)
		}
	}
	if lag := float64(w.count) - w.head; lag < 1 || lag > waveMaxLag {
		t.Errorf("playhead %.1f behind the newest level", lag)
	}
}

// Points between two levels read between them.
func TestWavePointsInterpolate(t *testing.T) {
	var w liveWave
	for range 40 {
		w.push(0)
	}
	for range 40 {
		w.push(1)
	}
	w.head = 39.5
	p := w.points(28)
	if v := p[len(p)-1]; v <= 0.2 || v >= 0.8 {
		t.Errorf("the newest point is %v between 0 and 1", v)
	}
}
