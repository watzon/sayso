package ui

import (
	"math"
	"sync"

	mu "github.com/egoist/mygo/ui"
)

// Ink drawn with paths (Rust sayso-ui components/waveform.rs): the live
// waveform and the line of the processing stage, and the Undo countdown.

// inkLayers are three layers of ink, from the wet edge to the dense core:
// the height of each, and its opacity. The notch's pale ink is denser.
var (
	inkLayers   = [3][2]float32{{0.95, 0.08}, {0.70, 0.18}, {0.45, 1.0}}
	notchLayers = [3][2]float32{{0.95, 0.14}, {0.70, 0.28}, {0.45, 1.0}}
)

// inkWaveform is the live waveform: a brush stroke of ink. levels are recent
// input levels from 0 to 1, oldest first. The shape tapers at both ends;
// phase (seconds) moves the ripple so the ink looks alive between levels.
func inkWaveform(c *mu.Context, levels []float32, phase float32, col mu.Color) mu.Element {
	// The newest level is at the end of the line.
	return mu.Box(c).Draw(mirrored(rtl(c), func(pt *mu.Painter, r mu.Rect) {
		paintInkLayers(pt, r, levels, phase, col, inkLayers)
		// Two spatters of ink, where the Paper design has them.
		pt.Fill(mu.Rect{X: r.X + r.W*0.935 - 1, Y: r.Y + r.H*0.28 - 1, W: 2, H: 2}, col.Alpha(0.4), 1)
		pt.Fill(mu.Rect{X: r.X + r.W*0.17 - 0.8, Y: r.Y + r.H*0.75 - 0.8, W: 1.6, H: 1.6}, col.Alpha(0.3), 0.8)
	}))
}

func sampleLevel(levels []float32, x float32) float32 {
	if len(levels) == 0 {
		return 0
	}
	pos := x * float32(len(levels)-1)
	i := int(pos)
	j := min(i+1, len(levels)-1)
	t := pos - float32(i)
	return levels[i]*(1-t) + levels[j]*t
}

// inkPaths are paths for the ink, reused: a painter flattens a path when it
// fills it and keeps nothing of it, and the waveform paints in every frame.
var inkPaths = sync.Pool{New: func() any { return new(mu.Path) }}

func paintInkLayers(pt *mu.Painter, r mu.Rect, levels []float32, phase float32, col mu.Color, layers [3][2]float32) {
	const n = 28
	mid, half := r.Y+r.H/2, r.H/2
	// The shape of the stroke before each layer's scale: the same for the
	// three layers.
	var shape [n]float32
	for i := range shape {
		x := float32(i) / (n - 1)
		// Brush taper: thin at both ends, full in the middle.
		taper := float32(math.Pow(math.Max(math.Sin(float64(x)*math.Pi), 0), 0.8))
		level := max(0, min(sampleLevel(levels, x), 1))
		ripple := 0.82 + 0.18*float32(math.Sin(float64(x*13+phase*6))*math.Cos(float64(x*5-phase*2)))
		// A small floor so silence still shows a hairline of ink.
		shape[i] = (0.04 + level*0.96) * taper * ripple
	}
	path := inkPaths.Get().(*mu.Path)
	defer inkPaths.Put(path)
	var top, bottom [n][2]float32
	for _, layer := range layers {
		scale, alpha := layer[0], layer[1]
		for i := range top {
			x := float32(i) / (n - 1)
			amp := shape[i] * scale
			top[i] = [2]float32{r.X + r.W*x, mid - half*amp}
		}
		for i, p := range top {
			bottom[n-1-i] = [2]float32{p[0], mid + (mid - p[1])}
		}
		path.Reset()
		path.MoveTo(top[0][0], top[0][1])
		smoothThrough(path, top[:])
		path.LineTo(bottom[0][0], bottom[0][1])
		smoothThrough(path, bottom[:])
		path.Close()
		pt.FillPath(path, col.Alpha(alpha))
	}
}

// smoothThrough draws quadratic curves through the midpoints of pts: a
// smooth line.
func smoothThrough(path *mu.Path, pts [][2]float32) {
	for i := 1; i < len(pts)-1; i++ {
		m := [2]float32{(pts[i][0] + pts[i+1][0]) / 2, (pts[i][1] + pts[i+1][1]) / 2}
		path.QuadTo(pts[i][0], pts[i][1], m[0], m[1])
	}
	last := pts[len(pts)-1]
	path.LineTo(last[0], last[1])
}

// smoothedLevels are the last n levels, oldest first, each a 1-2-1 average
// of its neighbors (Rust LiveAudio::smoothed).
func smoothedLevels(ordered []float32, n int) []float32 {
	raw := make([]float32, n+2)
	copy(raw[max(0, n+2-len(ordered)):], ordered[max(0, len(ordered)-(n+2)):])
	out := make([]float32, n)
	for i := range out {
		out[i] = (raw[i] + raw[i+1]*2 + raw[i+2]) / 4
	}
	return out
}

// inkLine is the processing stage: a line of ink drawn across to progress
// (0 to 1), with a wet nib, and a dotted pencil remainder.
func inkLine(c *mu.Context, progress float32, ink, pencil mu.Color) mu.Element {
	return mu.Box(c).Draw(mirrored(rtl(c), func(pt *mu.Painter, r mu.Rect) {
		y := r.Y + r.H/2
		x0, x1 := r.X+2, r.X+r.W-2
		nib := x0 + (x1-x0)*max(0, min(progress, 1))
		var path mu.Path
		path.MoveTo(x0, y).QuadTo((x0+nib)/2, y+0.6, nib, y)
		pt.StrokePath(&path, 2.4, ink)
		for x := nib + 8; x < x1; x += 5 {
			pt.Fill(mu.Rect{X: x, Y: y - 0.7, W: 1.4, H: 1.4}, pencil, 0.7)
		}
		// The nib and its wet halo.
		const rad = 3.2
		pt.Fill(mu.Rect{X: nib - rad*2, Y: y - rad*2, W: rad * 4, H: rad * 4}, ink.Alpha(0.08), rad*2)
		pt.Fill(mu.Rect{X: nib - rad, Y: y - rad, W: rad * 2, H: rad * 2}, ink, rad)
	}))
}

// processingProgress is how far the ink line of the processing stage has
// drawn, seconds into the stage.
func processingProgress(seconds float32, reduce bool) float32 {
	if reduce {
		return 0.6
	}
	f := seconds * 0.55
	f -= float32(math.Floor(float64(f)))
	return min(f*0.9+0.08, 0.98)
}

// countdownRing is the Undo countdown: a ring that empties as the time to
// undo runs out. left is from 1 (all of it) to 0.
func countdownRing(c *mu.Context, left float32, col mu.Color) mu.Element {
	return mu.Box(c).Draw(func(pt *mu.Painter, r mu.Rect) {
		cx, cy, rad := r.X+r.W/2, r.Y+r.H/2, r.W/2-1.5
		var track mu.Path
		track.Circle(cx, cy, rad)
		pt.StrokePath(&track, 1.6, col.Alpha(0.25))
		left = max(0, min(left, 1))
		if left <= 0.001 {
			return
		}
		// Clockwise from the top.
		const steps = 48
		a0 := -math.Pi / 2
		var arc mu.Path
		for i := 0; i <= steps; i++ {
			a := a0 + float64(left)*2*math.Pi*float64(i)/steps
			x, y := cx+rad*float32(math.Cos(a)), cy+rad*float32(math.Sin(a))
			if i == 0 {
				arc.MoveTo(x, y)
			} else {
				arc.LineTo(x, y)
			}
		}
		pt.StrokePath(&arc, 1.6, col)
	})
}
