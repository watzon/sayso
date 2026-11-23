// Package decoder finds the word being typed: a preallocated beam search
// over the model's key trie, scored by where each tap landed (touch) and by
// the language model (lexicon), with edits, missed spaces, and contractions.
// It decides whether committing a word should autocorrect it, and gives
// suggestions and next-word predictions.
package decoder

import (
	"math"

	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/touch"
)

// MaxTaps is the longest word the decoder searches; longer words are kept
// as typed.
const MaxTaps = lexicon.MaxKeyLen

// MaxBeam is the largest beam width.
const MaxBeam = 64

// Params are the decoder's weights, in nats.
type Params struct {
	BeamWidth  int
	EditThresh float32 // a letter whose spatial cost is above this is an edit
	SubCost    float32 // cost of a substituted letter one key from the tap
	SubSlope   float32 // added cost of a substitution per key size farther
	InsCost    float32 // cost of an extra tap
	DelCost    float32 // cost of a missed letter
	TransCost  float32 // cost of two swapped letters
	SplitCost  float32 // cost of a missed space
	LookWeight float32 // weight of the best unigram under a node while searching
	LMWeight   float32 // weight of the language model in a candidate's score
	OOVCost    float32 // language-model cost of a word the model lacks
	// MarginKnown is how much better a candidate must score than what was
	// typed, when that is a word, to replace it.
	MarginKnown float32
	// MarginGap is how much better the best candidate must score than the
	// second, when what was typed is not a word, to replace it.
	MarginGap float32
	// ExtraMargin is one more bar every correction must clear, on top of
	// the margins above (the user's strict setting).
	ExtraMargin float32
	// MaxCandCost: a candidate that scores worse than this is never put in.
	MaxCandCost float32
	// LongWord is the key length from which two edits are allowed.
	LongWord int
}

// DefaultParams are the tuned defaults (see the replay benchmark).
var DefaultParams = Params{
	BeamWidth:   48,
	EditThresh:  5.5,
	SubCost:     5.5,
	SubSlope:    1.5,
	InsCost:     7,
	DelCost:     7,
	TransCost:   5,
	SplitCost:   5,
	LookWeight:  0.6,
	LMWeight:    1,
	OOVCost:     15,
	MarginKnown: 4,
	MarginGap:   1.0,
	MaxCandCost: 40,
	LongWord:    6,
}

// hyp is one path through a trie: the node reached, the spatial and edit
// cost so far, and the edits used.
type hyp struct {
	node  uint32
	cost  float32
	rank  float32 // cost plus look-ahead, for pruning
	edits uint8
	src   uint8 // 0 the main model, 1 the personal lexicon
	depth uint8 // letters of the node's key
}

// Source of a word: the main model or the personal lexicon.
const (
	SrcMain     = 0
	SrcPersonal = 1
)

const dedupeSize = 4096 // power of two, larger than any step's candidates

// Decoder holds one word being typed. It is not safe for concurrent use and
// does not allocate after New, except in Suggestions' optional string form.
type Decoder struct {
	P      Params
	models [2]*lexicon.Model
	tm     *touch.Model

	taps  [MaxTaps]touch.Dist
	lit   [MaxTaps]byte // the letters typed, lower case
	ntaps int
	over  bool   // more taps than MaxTaps: keep what was typed
	apos  uint64 // bit i: an apostrophe was typed after letter i (bit 0: before the first)

	beam, next []hyp
	dedupe     [dedupeSize]struct {
		key uint64
		gen uint32
		idx int32
	}
	gen uint32

	ctx       lexicon.Context
	ctxValid  bool
	rootPrior [26]float32 // log P(first letter | context)

	cands  []Cand
	scored bool
}

// New makes a decoder over a model and a touch model.
func New(m *lexicon.Model, tm *touch.Model, p Params) *Decoder {
	if p.BeamWidth <= 0 || p.BeamWidth > MaxBeam {
		p.BeamWidth = MaxBeam
	}
	d := &Decoder{P: p, tm: tm}
	d.models[SrcMain] = m
	d.beam = make([]hyp, 0, MaxBeam*2)
	d.next = make([]hyp, 0, dedupeSize)
	d.cands = make([]Cand, 0, maxCands)
	d.SetContext(lexicon.NoWord, lexicon.NoWord)
	d.Reset()
	return d
}

// Model is the main model.
func (d *Decoder) Model() *lexicon.Model { return d.models[SrcMain] }

// Touch is the touch model.
func (d *Decoder) Touch() *touch.Model { return d.tm }

// SetTouch replaces the touch model (a new layout).
func (d *Decoder) SetTouch(tm *touch.Model) { d.tm = tm; d.rebuild() }

// SetPersonal sets the personal lexicon, a model built from the user's
// words (nil for none).
func (d *Decoder) SetPersonal(m *lexicon.Model) { d.models[SrcPersonal] = m; d.rebuild() }

// Personal is the personal lexicon, or nil.
func (d *Decoder) Personal() *lexicon.Model { return d.models[SrcPersonal] }

// SetContext sets the two words before the word being typed, as main-model
// word IDs (BOS for a sentence start, NoWord for none or unknown).
func (d *Decoder) SetContext(w1, w2 lexicon.WordID) {
	m := d.models[SrcMain]
	d.ctx = m.NewContext(w1, w2)
	d.ctxValid = w2 != lexicon.NoWord
	d.computeRootPrior()
	d.scored = false
}

// Context is the language-model context of the word being typed.
func (d *Decoder) Context() *lexicon.Context { return &d.ctx }

// Reset starts a new, empty word.
func (d *Decoder) Reset() {
	d.ntaps = 0
	d.over = false
	d.apos = 0
	d.beam = d.beam[:0]
	for src, m := range d.models {
		if m != nil {
			d.beam = append(d.beam, hyp{node: 0, src: uint8(src)})
		}
	}
	d.scored = false
}

// Len is the number of taps in the word.
func (d *Decoder) Len() int { return d.ntaps }

// Literal appends the letters typed, lower case, to dst.
func (d *Decoder) Literal(dst []byte) []byte { return append(dst, d.lit[:d.ntaps]...) }

// Tap returns tap i of the word.
func (d *Decoder) Tap(i int) *touch.Dist { return &d.taps[i] }

// Overflow reports whether the word has more taps than the decoder searches.
func (d *Decoder) Overflow() bool { return d.over }

// AddTap adds a tap that typed letter c ('a' to 'z') with spatial costs t.
// It does not allocate.
func (d *Decoder) AddTap(t *touch.Dist, c byte) {
	d.scored = false
	if d.ntaps == MaxTaps {
		d.over = true
		return
	}
	d.taps[d.ntaps] = *t
	d.lit[d.ntaps] = c
	d.ntaps++
	d.step(&d.taps[d.ntaps-1])
}

// AddApostrophe records an apostrophe typed at this point of the word. It
// does not move through the trie (keys have no apostrophes) but decides
// whether what was typed is itself a word ("it's" against "its").
func (d *Decoder) AddApostrophe() {
	if d.ntaps < 64 {
		d.apos |= 1 << uint(d.ntaps)
		d.scored = false
	}
}

// RemoveLast drops the last thing typed, an apostrophe or a tap, and
// searches the rest again.
func (d *Decoder) RemoveLast() {
	d.scored = false
	if d.over {
		d.over = false
		return
	}
	if d.ntaps < 64 && d.apos&(1<<uint(d.ntaps)) != 0 {
		d.apos &^= 1 << uint(d.ntaps)
		return
	}
	if d.ntaps == 0 {
		return
	}
	d.ntaps--
	d.rebuild()
}

// Typed appends the word as typed, lower case with its apostrophes, to dst.
func (d *Decoder) Typed(dst []byte) []byte {
	for i := 0; i <= d.ntaps; i++ {
		if i < 64 && d.apos&(1<<uint(i)) != 0 {
			dst = append(dst, '\'')
		}
		if i < d.ntaps {
			dst = append(dst, d.lit[i])
		}
	}
	return dst
}

func (d *Decoder) rebuild() {
	n, over, apos := d.ntaps, d.over, d.apos
	d.Reset()
	for i := 0; i < n; i++ {
		d.ntaps = i + 1
		d.step(&d.taps[i])
	}
	d.ntaps, d.over, d.apos = n, over, apos
	d.scored = false
}

// maxEdits is the most edits a word with a key of n letters may need.
func (d *Decoder) maxEdits(n int) uint8 {
	switch {
	case n >= d.P.LongWord:
		return 2
	case n >= 3:
		return 1
	}
	return 0
}

// lookahead is the weighted best unigram cost under a node.
func (d *Decoder) lookahead(src uint8, node uint32) float32 {
	m := d.models[src]
	return d.P.LookWeight * float32(m.Node(node).BestCost) * m.CostStep()
}

// push adds a candidate hypothesis to next, keeping the cheaper of two
// paths to the same node.
func (d *Decoder) push(src uint8, node uint32, cost float32, edits, depth uint8) {
	key := uint64(src)<<32 | uint64(node)
	h := (key * 0x9E3779B97F4A7C15) >> (64 - 12)
	for {
		e := &d.dedupe[h]
		if e.gen != d.gen {
			if len(d.next) == cap(d.next) {
				return
			}
			e.gen, e.key, e.idx = d.gen, key, int32(len(d.next))
			d.next = append(d.next, hyp{node: node, cost: cost, rank: cost + d.lookahead(src, node), edits: edits, src: src, depth: depth})
			return
		}
		if e.key == key {
			x := &d.next[e.idx]
			if cost < x.cost {
				x.rank += cost - x.cost
				x.cost, x.edits = cost, edits
			}
			return
		}
		h = (h + 1) & (dedupeSize - 1)
	}
}

// step extends every hypothesis by one tap.
func (d *Decoder) step(t *touch.Dist) {
	d.gen++
	if d.gen == 0 { // wrapped: clear the table
		d.dedupe = [dedupeSize]struct {
			key uint64
			gen uint32
			idx int32
		}{}
		d.gen = 1
	}
	d.next = d.next[:0]
	maxE := uint8(2)
	// The letters this tap most likely meant, for missed-letter paths.
	var top [3]byte
	topLetters(t, &top)
	for bi, h := range d.beam {
		m := d.models[h.src]
		nd := m.Node(h.node)
		wide := bi < 16 && h.edits < maxE // edits only for the best paths
		for c := nd.FirstChild; c < nd.FirstChild+uint32(nd.ChildCount); c++ {
			ch := m.Node(c).Char
			sc := t.Cost[ch-'a']
			switch {
			case sc <= d.P.EditThresh:
				d.push(h.src, c, h.cost+sc, h.edits, h.depth+1)
			case wide:
				sub := d.P.SubCost + d.P.SubSlope*max(0, t.Far[ch-'a']-1)
				d.push(h.src, c, h.cost+sub, h.edits+1, h.depth+1)
			}
			if wide {
				// A missed letter ch, then this tap types a letter after it.
				for _, l := range top {
					if g, ok := m.Child(c, l); ok {
						d.push(h.src, g, h.cost+d.P.DelCost+t.Cost[l-'a'], h.edits+1, h.depth+2)
					}
				}
			}
		}
		if wide {
			d.push(h.src, h.node, h.cost+d.P.InsCost, h.edits+1, h.depth) // an extra tap
		}
	}
	d.beam = d.beam[:0]
	selectTop(d.next, d.P.BeamWidth)
	n := min(len(d.next), d.P.BeamWidth)
	d.beam = append(d.beam, d.next[:n]...)
	sortByRank(d.beam)
}

// topLetters is the three letters a tap most likely meant.
func topLetters(t *touch.Dist, out *[3]byte) {
	var best [3]float32
	for i := range best {
		best[i] = float32(math.Inf(1))
	}
	for c := 0; c < 26; c++ {
		v := t.Cost[c]
		switch {
		case v < best[0]:
			best[2], out[2] = best[1], out[1]
			best[1], out[1] = best[0], out[0]
			best[0], out[0] = v, byte('a'+c)
		case v < best[1]:
			best[2], out[2] = best[1], out[1]
			best[1], out[1] = v, byte('a'+c)
		case v < best[2]:
			best[2], out[2] = v, byte('a'+c)
		}
	}
}

// selectTop moves the k lowest-rank hypotheses to the front (quickselect).
func selectTop(h []hyp, k int) {
	lo, hi := 0, len(h)-1
	if k >= len(h) {
		return
	}
	for lo < hi {
		p := h[(lo+hi)/2].rank
		i, j := lo, hi
		for i <= j {
			for h[i].rank < p {
				i++
			}
			for h[j].rank > p {
				j--
			}
			if i <= j {
				h[i], h[j] = h[j], h[i]
				i++
				j--
			}
		}
		switch {
		case k-1 <= j:
			hi = j
		case k-1 >= i:
			lo = i
		default:
			return
		}
	}
}

func sortByRank(h []hyp) {
	for i := 1; i < len(h); i++ {
		for j := i; j > 0 && h[j].rank < h[j-1].rank; j-- {
			h[j], h[j-1] = h[j-1], h[j]
		}
	}
}
