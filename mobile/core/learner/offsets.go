package learner

import "math"

// Offset adaptation: where this user's taps land relative to each key's
// centre, estimated at three levels (the keyboard, each row, each key) and
// blended so a key with few taps leans on its row and the keyboard. Offsets
// are in key pitches, so they survive a change of keyboard size.

const (
	// MaxDrift is the largest learned offset on each axis, in pitches.
	MaxDrift = 0.3
	// MaxStep is the most one tap may move an estimate on each axis.
	MaxStep = 0.05
	// outlier: taps farther than this from the centre teach nothing.
	outlier = 0.8
	// shrink is how many taps a level needs to count half.
	shrink = 20
	// rate is the slowest learning rate (a running mean of this many taps).
	rate = 60
	// MaxOffsetKeys is the most keys and rows the learner tracks.
	MaxOffsetKeys = 96
)

// estimate is a running mean of tap offsets.
type estimate struct {
	X, Y float64
	N    float64
}

func (e *estimate) add(dx, dy float64) {
	e.N++
	k := 1 / math.Min(e.N, rate)
	e.X += clamp((dx-e.X)*k, MaxStep)
	e.Y += clamp((dy-e.Y)*k, MaxStep)
	e.X = clamp(e.X, MaxDrift)
	e.Y = clamp(e.Y, MaxDrift)
}

func clamp(v, lim float64) float64 { return math.Max(-lim, math.Min(lim, v)) }

// Offsets is the three-level estimate.
type Offsets struct {
	Board estimate
	Rows  [MaxOffsetKeys]estimate
	Keys  [MaxOffsetKeys]estimate
}

// Sample is one confirmed tap: the key it was meant for, that key's row,
// and where it landed relative to the key's centre, in pitches.
type Sample struct {
	Key, Row int
	DX, DY   float32
}

func (o *Offsets) add(s Sample) {
	if s.Key < 0 || s.Key >= MaxOffsetKeys || s.Row < 0 || s.Row >= MaxOffsetKeys {
		return
	}
	dx, dy := float64(s.DX), float64(s.DY)
	if math.Abs(dx) > outlier || math.Abs(dy) > outlier || math.IsNaN(dx) || math.IsNaN(dy) {
		return
	}
	o.Board.add(dx, dy)
	o.Rows[s.Row].add(dx, dy)
	o.Keys[s.Key].add(dx, dy)
}

// Of is the blended offset of a key in a row, in pitches; it is within
// MaxDrift on each axis.
func (o *Offsets) Of(key, row int) (float32, float32) {
	if key < 0 || key >= MaxOffsetKeys || row < 0 || row >= MaxOffsetKeys {
		return 0, 0
	}
	w := func(n float64) float64 { return n / (n + shrink) }
	b, r, k := &o.Board, &o.Rows[row], &o.Keys[key]
	wb, wr, wk := w(b.N), w(r.N), w(k.N)
	x := wb * b.X
	y := wb * b.Y
	x += wr * (r.X - x)
	y += wr * (r.Y - y)
	x += wk * (k.X - x)
	y += wk * (k.Y - y)
	return float32(clamp(x, MaxDrift)), float32(clamp(y, MaxDrift))
}

// Samples is how many taps the key has taught.
func (o *Offsets) Samples(key int) int {
	if key < 0 || key >= MaxOffsetKeys {
		return 0
	}
	return int(o.Keys[key].N)
}
