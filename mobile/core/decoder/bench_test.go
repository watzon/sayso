package decoder

import (
	"testing"

	"github.com/watzon/sayso/mobile/core/touch"
)

// BenchmarkTap is one tap of a word: the prior, hit-target resolution, the
// spatial costs, and the beam step (the work behind on_touch).
func BenchmarkTap(b *testing.B) {
	d := newDecoder(b)
	setContext(b, d, "I", "want")
	l := d.tm.L
	word := []byte("understanding")
	var xs, ys [16]float32
	for i, c := range word {
		xs[i], ys[i] = l.Center(l.LetterKey(c))
		xs[i] += float32(i%3-1) * 6
	}
	var dist touch.Dist
	var p touch.Prior
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		j := i % len(word)
		if j == 0 {
			d.Reset()
		}
		d.NextPrior(&p)
		r := d.tm.Resolve(xs[j], ys[j], &p)
		d.tm.Spatial(xs[j], ys[j], &dist)
		d.AddTap(&dist, byte(l.Keys[r.Key].Code))
	}
}
