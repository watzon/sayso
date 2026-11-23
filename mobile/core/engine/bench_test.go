package engine

import (
	"slices"
	"testing"
	"time"
)

// tapLoop types an 8-letter word then a space, over and over.
type tapLoop struct {
	xs, ys [8]float32
	f      *field
}

func newTapLoop(t testing.TB) *tapLoop {
	f := newField(t, "")
	f.typeText("I want to ")
	l := f.e.layout
	tl := &tapLoop{f: f}
	for i, c := range []byte("understa") {
		tl.xs[i], tl.ys[i] = l.Center(l.LetterKey(c))
		tl.xs[i] += float32(i%3-1) * 6
	}
	return tl
}

// BenchmarkOnTouch is the work behind sayso_on_touch, without the cgo call.
func BenchmarkOnTouch(b *testing.B) {
	tl := newTapLoop(b)
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		j := i % 8
		tl.f.e.OnTouch(tl.xs[j], tl.ys[j], false)
		if j == 7 {
			b.StopTimer()
			tl.f.e.TypeChar(' ')
			b.StartTimer()
		}
	}
}

// raceEnabled is set under -race, whose instrumentation slows every call.
var raceEnabled bool

// TestOnTouchLatency times taps one by one and holds p99 under 50 µs.
func TestOnTouchLatency(t *testing.T) {
	if testing.Short() || raceEnabled {
		t.Skip("timing is meaningless in short or race runs")
	}
	tl := newTapLoop(t)
	const n = 100000
	d := make([]time.Duration, 0, n)
	for i := 0; i < n; i++ {
		j := i % 8
		t0 := time.Now()
		tl.f.e.OnTouch(tl.xs[j], tl.ys[j], false)
		d = append(d, time.Since(t0))
		if j == 7 {
			tl.f.e.TypeChar(' ')
		}
	}
	slices.Sort(d)
	p50, p99, p999 := d[n/2], d[n*99/100], d[n*999/1000]
	t.Logf("on_touch: p50 %v, p99 %v, p99.9 %v, max %v over %d taps", p50, p99, p999, d[n-1], n)
	if p99 > 50*time.Microsecond {
		t.Fatalf("p99 %v over 50µs", p99)
	}
}
