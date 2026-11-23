package decoder

import (
	"github.com/watzon/sayso/mobile/core/lexicon"
)

// Kind is how a candidate explains the taps.
type Kind uint8

const (
	KindWord       Kind = iota // a word whose key the taps spell, perhaps with edits
	KindCompletion             // a longer word the taps start
	KindSplit                  // two words with a missed space between them
	KindPrediction             // a next word, before any tap
)

// Cand is one candidate for the word being typed.
type Cand struct {
	Word, Word2 lexicon.WordID // Word2 is set for a split
	Src         uint8          // SrcMain or SrcPersonal (of Word)
	Kind        Kind
	Edits       uint8
	Spatial     float32 // spatial and edit cost
	LM          float32 // language-model cost
	Total       float32 // Spatial + LMWeight × LM (+ Bonus)
}

const maxCands = 128

// Bonus lets the learner adjust a word's language-model cost, for example
// from the user's own n-grams. Negative is better. Nil for none.
type Bonus interface {
	WordBonus(src uint8, w lexicon.WordID) float32
}

// Literal describes what was typed, as the user sees it.
type Literal struct {
	IsWord  bool           // the typed spelling is a word (main or personal)
	Word    lexicon.WordID // that word
	Src     uint8
	Spatial float32 // spatial cost of the letters as typed
	Total   float32 // Spatial plus its language-model cost (OOVCost if not a word)
}

// lmCost is the language-model cost of a word in the current context.
func (d *Decoder) lmCost(src uint8, w lexicon.WordID, b Bonus) float32 {
	var c float32
	if src == SrcMain {
		c = d.models[SrcMain].Cost(&d.ctx, w)
	} else {
		c = d.models[SrcPersonal].Unigram(w)
	}
	if b != nil {
		c += b.WordBonus(src, w)
	}
	return c
}

func (d *Decoder) addCand(c Cand) {
	for i := range d.cands {
		x := &d.cands[i]
		if x.Word == c.Word && x.Word2 == c.Word2 && x.Src == c.Src {
			if c.Total < x.Total {
				*x = c
			}
			return
		}
	}
	if len(d.cands) < cap(d.cands) {
		d.cands = append(d.cands, c)
		return
	}
	// Full: replace the worst if this one is better.
	worst := 0
	for i := range d.cands {
		if d.cands[i].Total > d.cands[worst].Total {
			worst = i
		}
	}
	if c.Total < d.cands[worst].Total {
		d.cands[worst] = c
	}
}

// exactSpatial is the spatial cost of the letters typed, taps i to j.
func (d *Decoder) exactSpatial(i, j int) float32 {
	var s float32
	for k := i; k < j; k++ {
		s += d.taps[k].Cost[d.lit[k]-'a']
	}
	return s
}

// Score finds and ranks the candidates for the word typed so far (best
// first) and describes the literal. It does not allocate.
func (d *Decoder) Score(b Bonus) ([]Cand, Literal) {
	d.cands = d.cands[:0]
	lit := d.literal(b)
	if d.ntaps == 0 || d.over {
		d.scored = true
		return d.cands, lit
	}
	lm := d.P.LMWeight
	for hi, h := range d.beam {
		m := d.models[h.src]
		nd := m.Node(h.node)
		if h.edits <= d.maxEdits(int(h.depth)) {
			for i := uint32(0); i < uint32(nd.TermCount); i++ {
				w := nd.WordLo + i
				if m.Flags(w)&lexicon.FlagNoSuggest != 0 {
					continue
				}
				l := d.lmCost(h.src, w, b)
				d.addCand(Cand{Word: w, Word2: lexicon.NoWord, Src: h.src, Kind: KindWord, Edits: h.edits, Spatial: h.cost, LM: l, Total: h.cost + lm*l})
			}
		}
		// A missed last letter.
		if hi < 8 && h.edits+1 <= d.maxEdits(int(h.depth)+1) {
			for c := nd.FirstChild; c < nd.FirstChild+uint32(nd.ChildCount); c++ {
				cn := m.Node(c)
				for i := uint32(0); i < uint32(cn.TermCount); i++ {
					w := cn.WordLo + i
					l := d.lmCost(h.src, w, b)
					cost := h.cost + d.P.DelCost
					d.addCand(Cand{Word: w, Word2: lexicon.NoWord, Src: h.src, Kind: KindWord, Edits: h.edits + 1, Spatial: cost, LM: l, Total: cost + lm*l})
				}
			}
		}
	}
	d.transpositions(b)
	d.splits(b)
	sortCands(d.cands)
	d.scored = true
	return d.cands, lit
}

// literal describes the typed spelling.
func (d *Decoder) literal(b Bonus) Literal {
	lit := Literal{Word: lexicon.NoWord, Spatial: d.exactSpatial(0, d.ntaps)}
	lit.Total = lit.Spatial + d.P.LMWeight*d.P.OOVCost
	if d.ntaps == 0 || d.over {
		return lit
	}
	var buf [MaxTaps + 8]byte
	typed := d.Typed(buf[:0])
	for src, m := range d.models {
		if m == nil {
			continue
		}
		lo, n := m.Lookup(d.lit[:d.ntaps])
		for i := lo; i < lo+lexicon.WordID(n); i++ {
			if equalFoldASCII(m.Word(i), typed) {
				l := d.lmCost(uint8(src), i, b)
				t := lit.Spatial + d.P.LMWeight*l
				if !lit.IsWord || t < lit.Total {
					lit.IsWord, lit.Word, lit.Src, lit.Total = true, i, uint8(src), t
				}
			}
		}
	}
	return lit
}

// equalFoldASCII compares a stored spelling with typed lower-case letters
// and apostrophes; accented letters match their plain form.
func equalFoldASCII(word, typed []byte) bool {
	var buf [lexicon.MaxKeyLen * 2]byte
	j := 0
	for i := 0; i < len(word); {
		c := word[i]
		if c >= 0x80 {
			// Fold one accented rune through AppendKey.
			n := 2
			if c >= 0xE0 {
				n = 3
			}
			if i+n > len(word) {
				return false
			}
			k, ok := lexicon.AppendKey(buf[:0], word[i:i+n])
			if !ok || len(k) != 1 || j >= len(typed) || typed[j] != k[0] {
				return false
			}
			i += n
			j++
			continue
		}
		if c >= 'A' && c <= 'Z' {
			c += 'a' - 'A'
		}
		if j >= len(typed) || typed[j] != c {
			return false
		}
		i++
		j++
	}
	return j == len(typed)
}

// transpositions adds words that the typed letters spell with two
// neighbouring letters swapped ("teh" → "the").
func (d *Decoder) transpositions(b Bonus) {
	n := d.ntaps
	if n < 2 {
		return
	}
	var key [MaxTaps]byte
	base := d.exactSpatial(0, n) + d.P.TransCost
	for i := 0; i+1 < n; i++ {
		if d.lit[i] == d.lit[i+1] {
			continue
		}
		copy(key[:n], d.lit[:n])
		key[i], key[i+1] = key[i+1], key[i]
		for src, m := range d.models {
			if m == nil {
				continue
			}
			lo, cnt := m.Lookup(key[:n])
			for w := lo; w < lo+lexicon.WordID(cnt); w++ {
				l := d.lmCost(uint8(src), w, b)
				d.addCand(Cand{Word: w, Word2: lexicon.NoWord, Src: uint8(src), Kind: KindWord, Edits: 1, Spatial: base, LM: l, Total: base + d.P.LMWeight*l})
			}
		}
	}
}

// splits adds two-word readings: a space left out ("ofthe"), or a tap on a
// bottom-row letter that was meant for the space bar ("ofbthe").
func (d *Decoder) splits(b Bonus) {
	n := d.ntaps
	if n < 3 {
		return
	}
	m := d.models[SrcMain]
	try := func(leftEnd, rightStart int, spatial float32) {
		left, right := d.lit[:leftEnd], d.lit[rightStart:n]
		if !splitPart(left) || !splitPart(right) {
			return
		}
		lo1, n1 := m.Lookup(left)
		lo2, n2 := m.Lookup(right)
		if n1 == 0 || n2 == 0 {
			return
		}
		// The most probable spelling of each half.
		w1, w2 := lo1, lo2
		l1 := m.Cost(&d.ctx, w1)
		c2 := m.NewContext(d.ctx.W2, w1)
		l2 := m.Cost(&c2, w2)
		if b != nil {
			l1 += b.WordBonus(SrcMain, w1)
		}
		cost := spatial + d.P.SplitCost
		d.addCand(Cand{Word: w1, Word2: w2, Src: SrcMain, Kind: KindSplit, Edits: 1, Spatial: cost, LM: l1 + l2, Total: cost + d.P.LMWeight*(l1+l2)})
	}
	all := d.exactSpatial(0, n)
	for i := 1; i < n; i++ {
		try(i, i, all)
	}
	for j := 1; j+1 < n; j++ {
		if s := d.taps[j].Space; s < 6 {
			try(j, j+1, all-d.taps[j].Cost[d.lit[j]-'a']+s)
		}
	}
}

// splitPart: one-letter halves only as "a" or "i".
func splitPart(k []byte) bool {
	return len(k) > 1 || (len(k) == 1 && (k[0] == 'a' || k[0] == 'i'))
}

func sortCands(c []Cand) {
	for i := 1; i < len(c); i++ {
		for j := i; j > 0 && c[j].Total < c[j-1].Total; j-- {
			c[j], c[j-1] = c[j-1], c[j]
		}
	}
}

// Decision is what committing the word should do.
type Decision struct {
	Correct bool // replace the typed word with Cand
	Cand    Cand
	Literal Literal
	// Margin is how much better Cand scored than the bar it had to clear:
	// the literal when it is a word, else the second candidate. Negative
	// when it did not clear it.
	Margin float32
}

// Decide says whether to autocorrect the word typed so far. The engine
// applies the learner's vetoes on top. It does not allocate.
func (d *Decoder) Decide(b Bonus) Decision {
	cands, lit := d.Score(b)
	dec := Decision{Literal: lit}
	if len(cands) == 0 {
		return dec
	}
	var buf [MaxTaps + 8]byte
	typed := d.Typed(buf[:0])
	best := -1
	for i := range cands {
		if !d.sameAsTyped(&cands[i], typed) {
			best = i
			break
		}
		if i == 0 {
			// What was typed scores best: nothing to correct.
			dec.Cand, dec.Margin = cands[0], -1
			return dec
		}
	}
	if best < 0 {
		return dec
	}
	c := cands[best]
	dec.Cand = c
	if c.Total > d.P.MaxCandCost {
		dec.Margin = d.P.MaxCandCost - c.Total
		return dec
	}
	if c.Kind == KindWord && d.models[c.Src].Flags(c.Word)&lexicon.FlagNoAutocorrect != 0 {
		dec.Margin = -1
		return dec
	}
	if lit.IsWord {
		dec.Margin = lit.Total - c.Total - d.P.MarginKnown
	} else {
		second := float32(1e9)
		for i := best + 1; i < len(cands); i++ {
			if !sameWords(&cands[i], &c) {
				second = cands[i].Total
				break
			}
		}
		dec.Margin = min(second-c.Total-d.P.MarginGap, lit.Total-c.Total)
	}
	dec.Correct = dec.Margin >= 0
	return dec
}

func sameWords(a, b *Cand) bool { return a.Word == b.Word && a.Word2 == b.Word2 && a.Src == b.Src }

// sameAsTyped reports whether a candidate is spelled as typed (ignoring case).
func (d *Decoder) sameAsTyped(c *Cand, typed []byte) bool {
	return c.Kind == KindWord && equalFoldASCII(d.models[c.Src].Word(c.Word), typed)
}
