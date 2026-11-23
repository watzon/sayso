package decoder

import (
	"math"

	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/touch"
)

const priorFloor = 1e-4

// computeRootPrior is P(first letter | context): the explicit next words of
// the context, plus the backed-off rest spread by the root's children.
func (d *Decoder) computeRootPrior() {
	m := d.models[SrcMain]
	var mass [26]float64
	root := m.Node(0)
	step := float64(m.CostStep())
	for c := root.FirstChild; c < root.FirstChild+uint32(root.ChildCount); c++ {
		nd := m.Node(c)
		mass[nd.Char-'a'] += math.Exp(-float64(nd.BestCost) * step)
	}
	if d.ctxValid {
		// Scale the unigram view down, then add the context's own next words.
		var total float64
		for _, v := range mass {
			total += v
		}
		for i := range mass {
			mass[i] = 0.3 * mass[i] / total
		}
		var ctxMass [26]float64
		var ctxTotal float64
		m.Successors(&d.ctx, func(w lexicon.WordID, cost float32) bool {
			if c := firstLetter(m.Word(w)); c != 0 {
				p := math.Exp(-float64(cost))
				ctxMass[c-'a'] += p
				ctxTotal += p
			}
			return true
		})
		if ctxTotal > 0 {
			for i := range mass {
				mass[i] += 0.7 * ctxMass[i] / ctxTotal
			}
		}
	}
	normalizeLog(&mass, 0, &d.rootPrior, nil)
}

func firstLetter(w []byte) byte {
	var buf [8]byte
	k, ok := lexicon.AppendKey(buf[:0], w[:min(len(w), 4)])
	if !ok || len(k) == 0 {
		// The first four bytes may cut a rune; fold the whole word.
		var big [lexicon.MaxKeyLen * 2]byte
		if k, ok = lexicon.AppendKey(big[:0], w); !ok || len(k) == 0 {
			return 0
		}
	}
	return k[0]
}

// normalizeLog turns masses into log probabilities with a floor; space, if
// not nil, gets the log probability of spaceMass.
func normalizeLog(mass *[26]float64, spaceMass float64, out *[26]float32, space *float32) {
	total := spaceMass
	for _, v := range mass {
		total += v
	}
	if total <= 0 {
		total = 1
	}
	for i, v := range mass {
		out[i] = float32(math.Log(v/total + priorFloor))
	}
	if space != nil {
		*space = float32(math.Log(spaceMass/total + priorFloor))
	}
}

// NextPrior fills p with what the language model expects the next tap to
// type: each letter, or the space bar ending the word. The touch model uses
// it for hit-target expansion. It does not allocate.
func (d *Decoder) NextPrior(p *touch.Prior) {
	p.Valid = true
	if d.ntaps == 0 {
		p.Letter = d.rootPrior
		p.Space = float32(math.Log(priorFloor)) // a space now types no word
		return
	}
	if d.over || len(d.beam) == 0 {
		p.Valid = false
		return
	}
	var mass [26]float64
	var end float64
	best := d.beam[0].rank
	for i, h := range d.beam {
		if i == 16 {
			break
		}
		w := math.Exp(-float64(h.rank - best))
		if w < 1e-4 {
			break
		}
		m := d.models[h.src]
		step := float64(m.CostStep())
		nd := m.Node(h.node)
		if nd.TermCount > 0 {
			// Key-mates are sorted by cost, so the first is the best.
			end += w * math.Exp(-float64(m.Unigram(nd.WordLo)-float32(nd.BestCost)*m.CostStep()))
		}
		for c := nd.FirstChild; c < nd.FirstChild+uint32(nd.ChildCount); c++ {
			ch := m.Node(c)
			mass[ch.Char-'a'] += w * math.Exp(-float64(int(ch.BestCost)-int(nd.BestCost))*step)
		}
	}
	normalizeLog(&mass, end, &p.Letter, &p.Space)
}
