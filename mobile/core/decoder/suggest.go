package decoder

import (
	"github.com/watzon/sayso/mobile/core/lexicon"
)

// Suggestion flags.
const (
	SugLiteral     uint8 = 1 << iota // what was typed (Word is NoWord when it is not a word)
	SugAutocorrect                   // committing now would put this in
	SugCompletion                    // a longer word the taps start
	SugPrediction                    // a next word
	SugPersonal                      // from the personal lexicon
)

// Suggestion is one entry of the suggestion strip.
type Suggestion struct {
	Cand
	Flags uint8
}

// completionCost is added to a completion's score: it types letters the
// user has not typed yet.
const completionCost = 2.5

// Suggest fills out with suggestions, best first, and returns how many. The
// first is what committing now would write (the autocorrection, else what
// was typed). Before any tap, it predicts next words. It does not allocate.
func (d *Decoder) Suggest(b Bonus, out []Suggestion) int {
	if len(out) == 0 {
		return 0
	}
	if d.ntaps == 0 {
		return d.predict(b, out)
	}
	dec := d.Decide(b)
	n := 0
	add := func(s Suggestion) {
		if n == len(out) {
			return
		}
		for i := 0; i < n; i++ {
			if out[i].Word == s.Word && out[i].Word2 == s.Word2 && out[i].Src == s.Src && (s.Word != lexicon.NoWord || out[i].Flags&SugLiteral != 0) {
				out[i].Flags |= s.Flags
				return
			}
		}
		if s.Src == SrcPersonal {
			s.Flags |= SugPersonal
		}
		out[n] = s
		n++
	}
	litSug := Suggestion{Cand: Cand{Word: dec.Literal.Word, Word2: lexicon.NoWord, Src: dec.Literal.Src, Kind: KindWord, Total: dec.Literal.Total}, Flags: SugLiteral}
	if dec.Correct {
		add(Suggestion{Cand: dec.Cand, Flags: SugAutocorrect})
		add(litSug)
	} else {
		add(litSug)
	}
	// Candidates (Score already ran in Decide) and completions, by score.
	var comps [8]Cand
	nc := d.completions(b, comps[:])
	ci := 0
	for _, c := range d.cands {
		for ci < nc && comps[ci].Total < c.Total {
			add(Suggestion{Cand: comps[ci], Flags: SugCompletion})
			ci++
		}
		add(Suggestion{Cand: c})
		if n == len(out) {
			break
		}
	}
	for ; ci < nc; ci++ {
		add(Suggestion{Cand: comps[ci], Flags: SugCompletion})
	}
	return n
}

// completions finds likely longer words under the best paths, sorted.
func (d *Decoder) completions(b Bonus, out []Cand) int {
	n := 0
	put := func(c Cand) {
		for i := 0; i < n; i++ {
			if out[i].Word == c.Word && out[i].Src == c.Src {
				if c.Total < out[i].Total {
					out[i] = c
				}
				return
			}
		}
		if n < len(out) {
			out[n] = c
			n++
			return
		}
		worst := 0
		for i := 1; i < n; i++ {
			if out[i].Total > out[worst].Total {
				worst = i
			}
		}
		if c.Total < out[worst].Total {
			out[worst] = c
		}
	}
	for hi, h := range d.beam {
		if hi == 6 {
			break
		}
		if h.edits > 1 {
			continue
		}
		m := d.models[h.src]
		nd := m.Node(h.node)
		// The best word under each of the two best children.
		var kids [2]uint32
		nk := 0
		for c := nd.FirstChild; c < nd.FirstChild+uint32(nd.ChildCount); c++ {
			cn := m.Node(c)
			switch {
			case nk < 2:
				kids[nk] = c
				nk++
				if nk == 2 && m.Node(kids[1]).BestCost < m.Node(kids[0]).BestCost {
					kids[0], kids[1] = kids[1], kids[0]
				}
			case cn.BestCost < m.Node(kids[1]).BestCost:
				kids[1] = c
				if cn.BestCost < m.Node(kids[0]).BestCost {
					kids[0], kids[1] = kids[1], kids[0]
				}
			}
		}
		for k := 0; k < nk; k++ {
			w := m.BestWord(kids[k])
			if m.Flags(w)&lexicon.FlagNoSuggest != 0 {
				continue
			}
			l := d.lmCost(h.src, w, b)
			cost := h.cost + completionCost
			put(Cand{Word: w, Word2: lexicon.NoWord, Src: h.src, Kind: KindCompletion, Edits: h.edits, Spatial: cost, LM: l, Total: cost + d.P.LMWeight*l})
		}
	}
	sortCands(out[:n])
	return n
}

// predict fills out with the likeliest next words after the context.
func (d *Decoder) predict(b Bonus, out []Suggestion) int {
	m := d.models[SrcMain]
	ctx := &d.ctx
	if !d.ctxValid {
		c := m.NewContext(lexicon.NoWord, m.BOS())
		ctx = &c
	}
	n := 0
	m.Successors(ctx, func(w lexicon.WordID, cost float32) bool {
		if m.Flags(w)&lexicon.FlagNoSuggest != 0 {
			return true
		}
		if b != nil {
			cost += b.WordBonus(SrcMain, w)
		}
		for i := 0; i < n; i++ {
			if out[i].Word == w {
				if cost < out[i].Total {
					out[i].Total = cost
				}
				return true
			}
		}
		s := Suggestion{Cand: Cand{Word: w, Word2: lexicon.NoWord, Src: SrcMain, Kind: KindPrediction, LM: cost, Total: cost}, Flags: SugPrediction}
		if n < len(out) {
			out[n] = s
			n++
			return true
		}
		worst := 0
		for i := 1; i < n; i++ {
			if out[i].Total > out[worst].Total {
				worst = i
			}
		}
		if cost < out[worst].Total {
			out[worst] = s
		}
		return true
	})
	for i := 1; i < n; i++ {
		for j := i; j > 0 && out[j].Total < out[j-1].Total; j-- {
			out[j], out[j-1] = out[j-1], out[j]
		}
	}
	return n
}
