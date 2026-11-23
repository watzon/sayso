package touch

import (
	"math"
	"math/rand/v2"
	"testing"
)

const kw, kh = 36, 54

func qwerty(t testing.TB) (*Layout, *Model) {
	t.Helper()
	l, err := NewLayout(QWERTY(kw, kh))
	if err != nil {
		t.Fatal(err)
	}
	return l, NewModel(l, DefaultParams)
}

func center(l *Layout, c rune) (float32, float32) {
	if c == ' ' {
		return l.Center(l.Space())
	}
	return l.Center(l.LetterKey(byte(c)))
}

func flatPrior() *Prior {
	p := &Prior{Valid: true, Space: -10}
	for i := range p.Letter {
		p.Letter[i] = -10
	}
	return p
}

func TestLayout(t *testing.T) {
	l, _ := qwerty(t)
	if l.Pitch() != kw || l.RowCount() != 4 {
		t.Fatal(l.Pitch(), l.RowCount())
	}
	if l.Row(l.LetterKey('q')) != 0 || l.Row(l.LetterKey('a')) != 1 || l.Row(l.Space()) != 3 {
		t.Fatal("rows")
	}
	g := l.LetterKey('g')
	want := map[byte]bool{'f': true, 'h': true, 't': true, 'y': true, 'v': true, 'b': true}
	got := map[byte]bool{}
	for _, n := range l.Neighbours(g) {
		if l.Kind(int(n)) == KindLetter {
			got[byte(l.Keys[n].Code)] = true
		}
	}
	for c := range want {
		if !got[c] {
			t.Errorf("g lacks neighbour %c (has %v)", c, got)
		}
	}
	if _, err := NewLayout([]Key{{Code: 'a', W: 0, H: 1}}); err == nil {
		t.Error("zero-width key accepted")
	}
	if _, err := NewLayout([]Key{{Code: 'a', W: 1, H: 1}, {Code: 'A', X: 2, W: 1, H: 1}}); err == nil {
		t.Error("duplicate letter accepted")
	}
}

func TestSpatial(t *testing.T) {
	l, m := qwerty(t)
	var d Dist
	x, y := center(l, 'g')
	m.Spatial(x, y, &d)
	var sum float64
	best := 0
	for c := range d.Cost {
		sum += math.Exp(-float64(d.Cost[c]))
		if d.Cost[c] < d.Cost[best] {
			best = c
		}
	}
	sum += math.Exp(-float64(d.Space))
	if math.Abs(sum-1) > 0.01 || best != 'g'-'a' {
		t.Fatalf("sum %v, best %c", sum, best+'a')
	}
	if d.Cost['f'-'a'] >= d.Cost['p'-'a'] {
		t.Fatal("a neighbour should cost less than a far key")
	}
	// Far is in key sizes and, unlike Cost, keeps growing past the cap.
	near := func(c byte, want float64) {
		t.Helper()
		if got := float64(d.Far[c-'a']); math.Abs(got-want) > 0.01 {
			t.Errorf("Far[%c] = %.3f, want %.3f", c, got, want)
		}
	}
	near('g', 0)
	near('f', 1)
	near('h', 1)
	near('t', math.Hypot(0.5, 1)) // the row above, half a key over
	near('a', 4)
	near('p', math.Hypot(4.5, 1))
	if d.Cost['a'-'a'] != d.Cost['p'-'a'] {
		t.Fatal("expected far keys to share the capped cost; Far is what tells them apart")
	}
}

// Every key keeps its core: no prior, however extreme, moves a tap there.
func TestCoreIsGuaranteed(t *testing.T) {
	l, m := qwerty(t)
	rng := rand.New(rand.NewPCG(1, 2))
	for range 20000 {
		p := &Prior{Valid: true, Space: -rng.Float32() * 30}
		for i := range p.Letter {
			p.Letter[i] = -rng.Float32() * 30
		}
		i := rng.IntN(l.Len())
		k := l.Keys[i]
		f := DefaultParams.CoreFrac * 0.999
		x := k.X + k.W/2 + (rng.Float32()-0.5)*f*k.W
		y := k.Y + k.H/2 + (rng.Float32()-0.5)*f*k.H
		if r := m.Resolve(x, y, p); r.Key != i || !r.Core {
			t.Fatalf("tap in the core of %q typed %q", k.Code, l.Keys[r.Key].Code)
		}
	}
}

func TestBorrowing(t *testing.T) {
	l, m := qwerty(t)
	w := l.Keys[l.LetterKey('w')]
	x, y := w.X+4, w.Y+w.H/2 // w's left border, next to q
	if r := m.Resolve(x, y, nil); r.Key != l.LetterKey('w') {
		t.Fatal("no prior: the key under the tap")
	}
	p := flatPrior()
	p.Letter['q'-'a'] = -0.1 // "q" is all but certain
	if r := m.Resolve(x, y, p); r.Key != l.LetterKey('q') || !r.Borrowed() {
		t.Fatal("a certain q should claim w's border")
	}
	p.Letter['q'-'a'] = -9 // a third of the reach (2.6) does not get to the tap (4)
	if r := m.Resolve(x, y, p); r.Key != l.LetterKey('w') {
		t.Fatal("a weak lead should not claim the tap")
	}
	// Past MaxBorrow, even a certain neighbour cannot claim it.
	p.Letter['q'-'a'] = -0.1
	far := w.X + DefaultParams.MaxBorrow*kw + 1
	if far < w.X+(1-DefaultParams.CoreFrac)/2*w.W {
		if r := m.Resolve(far, y, p); r.Key != l.LetterKey('w') {
			t.Fatal("claim reached past MaxBorrow")
		}
	}
	var exp [MaxKeys]float32
	m.Expansion(p, exp[:])
	if exp[l.LetterKey('q')] != DefaultParams.MaxBorrow*kw || exp[l.LetterKey('w')] != 0 {
		t.Fatal("expansion", exp[l.LetterKey('q')], exp[l.LetterKey('w')])
	}
}

func TestSpaceBarAndBottomRow(t *testing.T) {
	l, m := qwerty(t)
	b := l.Keys[l.LetterKey('b')]
	sp := l.Keys[l.Space()]
	low := b.Y + b.H - 2 // bottom border of b
	high := sp.Y + 2     // top border of the space bar, under b
	x := b.X + b.W/2

	endsWord := flatPrior()
	endsWord.Space = -0.05
	if r := m.Resolve(x, low, endsWord); r.Key != l.Space() {
		t.Fatal("a finished word should let the space bar claim b's lower border")
	}
	if r := m.Resolve(x, low, flatPrior()); r.Key != l.LetterKey('b') {
		t.Fatal("without a reason the tap stays on b")
	}
	wantsB := flatPrior()
	wantsB.Letter['b'-'a'] = -0.05
	if r := m.Resolve(x, high, wantsB); r.Key != l.LetterKey('b') {
		t.Fatal("an expected b should claim the top of the space bar")
	}
	if r := m.Resolve(x, high, endsWord); r.Key != l.Space() {
		t.Fatal("space stays space")
	}
	// Function keys never lose taps: shift's border stays shift.
	sh := l.Keys[l.Len()-6]
	if sh.Code != CodeShift {
		t.Fatal("layout order")
	}
	wantsZ := flatPrior()
	wantsZ.Letter['z'-'a'] = 0
	if r := m.Resolve(sh.X+sh.W-1, sh.Y+sh.H/2, wantsZ); r.Key != l.Len()-6 {
		t.Fatal("a letter took a tap from shift")
	}
}

func TestOffsetsMoveTargets(t *testing.T) {
	l, m := qwerty(t)
	d := l.LetterKey('d')
	x, y := center(l, 'd')
	y += kh * 0.55 // just inside c
	if r := m.Resolve(x, y, nil); r.Key == d {
		t.Fatal("setup: the tap should be off d")
	}
	m.SetOffset(d, 0, kh*0.3)
	if r := m.Resolve(x, y, nil); r.Key != d {
		t.Fatal("a learned downward offset should bring the tap back to d")
	}
}

func TestNoAllocations(t *testing.T) {
	l, m := qwerty(t)
	p := flatPrior()
	x, y := center(l, 'h')
	var d Dist
	if n := testing.AllocsPerRun(1000, func() { m.Resolve(x+15, y+20, p); m.Spatial(x, y, &d) }); n != 0 {
		t.Fatalf("%v allocs", n)
	}
}
