package touch

import (
	"math"
)

// Params tune the touch model. Fractions are of the key's size or of the
// layout's pitch.
type Params struct {
	// SigmaFrac is the spread of taps around a key's centre, as a fraction
	// of its width (x) and height (y).
	SigmaFrac float32
	// CoreFrac is the central part of each key's width and height that
	// always types that key, whatever the language model expects.
	CoreFrac float32
	// MaxBorrow is the most a likely key may claim beyond its own edge, as a
	// fraction of the pitch.
	MaxBorrow float32
	// Tau is how many nats more likely than the touched key a neighbour
	// must be to claim the whole MaxBorrow; less claims proportionally less.
	Tau float32
}

// DefaultParams are the tuned defaults (see the replay benchmark).
var DefaultParams = Params{SigmaFrac: 0.28, CoreFrac: 0.5, MaxBorrow: 0.22, Tau: 3}

// NoPrior marks a key the language model has no opinion on.
var NoPrior = float32(math.Inf(-1))

// Prior is the language model's view of the next key: log probabilities of
// each letter and of ending the word (the space bar). Valid false turns
// expansion off.
type Prior struct {
	Letter [26]float32
	Space  float32
	Valid  bool
}

// Model is the touch model of a layout. Offsets are the learned shifts of
// each key's target. It is not safe for concurrent use.
type Model struct {
	L       *Layout
	P       Params
	off     [MaxKeys][2]float32
	inv     [MaxKeys][3]float32 // 1/σx², 1/σy², ρ/(σxσy), each over (1-ρ²)
	logNorm [MaxKeys]float32
}

// NewModel makes the model of a layout with no learned offsets.
func NewModel(l *Layout, p Params) *Model {
	m := &Model{L: l, P: p}
	for i, k := range l.Keys {
		m.setShape(i, p.SigmaFrac*k.W, p.SigmaFrac*k.H, 0)
	}
	return m
}

func (m *Model) setShape(i int, sx, sy, rho float32) {
	q := 1 - rho*rho
	m.inv[i] = [3]float32{1 / (sx * sx * q), 1 / (sy * sy * q), rho / (sx * sy * q)}
	m.logNorm[i] = -float32(math.Log(2 * math.Pi * float64(sx*sy) * math.Sqrt(float64(q))))
}

// SetShape sets key i's spread (σx, σy) and correlation ρ, |ρ| < 1.
func (m *Model) SetShape(i int, sx, sy, rho float32) {
	if sx > 0 && sy > 0 && rho > -0.95 && rho < 0.95 {
		m.setShape(i, sx, sy, rho)
	}
}

// SetOffset shifts key i's target by (dx, dy).
func (m *Model) SetOffset(i int, dx, dy float32) { m.off[i] = [2]float32{dx, dy} }

// Offset is key i's learned shift.
func (m *Model) Offset(i int) (float32, float32) { return m.off[i][0], m.off[i][1] }

// LogLik is the log density of a tap at (x, y) for key i.
func (m *Model) LogLik(i int, x, y float32) float32 {
	cx, cy := m.L.Center(i)
	zx := x - cx - m.off[i][0]
	zy := y - cy - m.off[i][1]
	v := &m.inv[i]
	return m.logNorm[i] - 0.5*(zx*zx*v[0]+zy*zy*v[1]-2*zx*zy*v[2])
}

// MaxSpatialCost caps the spatial cost of a letter, so a far letter is
// unlikely but still reachable as an edit.
const MaxSpatialCost = 14

// Dist is one tap seen by the decoder: the cost of each letter and of the
// space bar, -ln P(key | tap), normalized over letters and space.
type Dist struct {
	Cost [26]float32
	// Far is how far each letter's key is from the tap, in key sizes (x
	// over the key's width, y over its height): about 1 for a neighbour.
	// Unlike Cost it is not capped, so the decoder can price a slip onto
	// a key by how far away that key is.
	Far   [26]float32
	Space float32
	X, Y  float32
	Key   int16 // the key the tap typed
}

// Spatial fills d for a tap at (x, y). It does not allocate.
func (m *Model) Spatial(x, y float32, d *Dist) {
	var ll [27]float32
	best := float32(math.Inf(-1))
	for c := 0; c < 26; c++ {
		ll[c] = float32(math.Inf(-1))
		d.Far[c] = farAbsent
		if k := m.L.letter[c]; k >= 0 {
			ll[c] = m.LogLik(int(k), x, y)
			d.Far[c] = m.far(int(k), x, y)
		}
		best = max(best, ll[c])
	}
	ll[26] = float32(math.Inf(-1))
	if m.L.space >= 0 {
		ll[26] = m.LogLik(int(m.L.space), x, y)
		best = max(best, ll[26])
	}
	var sum float64
	for _, v := range ll {
		sum += math.Exp(float64(v - best))
	}
	lse := best + float32(math.Log(sum))
	for c := 0; c < 26; c++ {
		d.Cost[c] = min(lse-ll[c], MaxSpatialCost)
	}
	d.Space = min(lse-ll[26], MaxSpatialCost)
	d.X, d.Y = x, y
}

// farAbsent is Dist.Far of a letter the layout lacks: finite, so that
// weighting it by zero stays zero.
const farAbsent = 100

// far is the distance from a tap to key i's target, in key sizes.
func (m *Model) far(i int, x, y float32) float32 {
	k := &m.L.Keys[i]
	cx, cy := m.L.Center(i)
	zx := (x - cx - m.off[i][0]) / k.W
	zy := (y - cy - m.off[i][1]) / k.H
	return float32(math.Sqrt(float64(zx*zx + zy*zy)))
}

// gap is how far the tap is outside key i's rectangle shifted by its
// offset, as (dx, dy) components; 0, 0 inside.
func (m *Model) gap(i int, x, y float32) (float32, float32) {
	k := &m.L.Keys[i]
	px, py := x-m.off[i][0], y-m.off[i][1]
	dx := max(0, max(k.X-px, px-(k.X+k.W)))
	dy := max(0, max(k.Y-py, py-(k.Y+k.H)))
	return dx, dy
}

func (m *Model) dist(i int, x, y float32) float32 {
	dx, dy := m.gap(i, x, y)
	return float32(math.Sqrt(float64(dx*dx + dy*dy)))
}

// owner is the key under the tap, offsets applied; nearest when the tap is
// on no key, highest density when shifted keys overlap.
func (m *Model) owner(x, y float32) int {
	best, bestD, bestLL := 0, float32(math.Inf(1)), float32(math.Inf(-1))
	for i := range m.L.Keys {
		d := m.dist(i, x, y)
		if d < bestD || (d == bestD && d == 0 && m.LogLik(i, x, y) > bestLL) {
			best, bestD = i, d
			bestLL = m.LogLik(i, x, y)
		}
	}
	return best
}

func (m *Model) inCore(i int, x, y float32) bool {
	k := &m.L.Keys[i]
	cx, cy := m.L.Center(i)
	px, py := x-m.off[i][0], y-m.off[i][1]
	return abs(px-cx) <= m.P.CoreFrac*k.W/2 && abs(py-cy) <= m.P.CoreFrac*k.H/2
}

func abs(f float32) float32 {
	if f < 0 {
		return -f
	}
	return f
}

// prior is the log probability the language model gives key i.
func (m *Model) prior(p *Prior, i int) float32 {
	if p == nil || !p.Valid {
		return NoPrior
	}
	switch m.L.kinds[i] {
	case KindLetter:
		return p.Letter[m.L.Keys[i].Code-'a']
	case KindSpace:
		return p.Space
	}
	return NoPrior
}

// share is the fraction of MaxBorrow key k may claim from key o.
func (m *Model) share(p *Prior, k, o int) float32 {
	pk, po := m.prior(p, k), m.prior(p, o)
	if math.IsInf(float64(pk), -1) || math.IsInf(float64(po), -1) {
		return 0
	}
	return min(1, max(0, (pk-po)/m.P.Tau))
}

// Resolution is the key a tap typed and why.
type Resolution struct {
	Key   int  // the key typed
	Owner int  // the key under the tap
	Core  bool // the tap was in the owner's core
}

// Borrowed reports whether a neighbour took the tap from its owner.
func (r Resolution) Borrowed() bool { return r.Key != r.Owner }

// Resolve picks the key for a tap. A tap in a key's core types that key. A
// tap in its border types a neighbour instead when the language model
// expects the neighbour enough for its expanded edge to reach the tap.
func (m *Model) Resolve(x, y float32, p *Prior) Resolution {
	o := m.owner(x, y)
	r := Resolution{Key: o, Owner: o}
	if m.inCore(o, x, y) {
		r.Core = true
		return r
	}
	if m.L.kinds[o] != KindLetter && m.L.kinds[o] != KindSpace {
		return r
	}
	reach := m.P.MaxBorrow * m.L.pitch
	bestMargin := float32(0)
	for _, k := range m.L.Neighbours(o) {
		s := m.share(p, int(k), o)
		if s == 0 {
			continue
		}
		b := s * reach
		if margin := b - m.dist(int(k), x, y); margin >= 0 && margin >= bestMargin {
			r.Key, bestMargin = int(k), margin
		}
	}
	return r
}

// Expansion fills out[i] with how far key i reaches past its edge into its
// neighbours under prior p (the largest claim over its neighbours), in
// layout units. out must have a length of at least the number of keys.
func (m *Model) Expansion(p *Prior, out []float32) {
	reach := m.P.MaxBorrow * m.L.pitch
	for i := range m.L.Keys {
		var e float32
		for _, n := range m.L.Neighbours(i) {
			e = max(e, m.share(p, i, int(n))*reach)
		}
		out[i] = e
	}
}
