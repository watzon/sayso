package main

import (
	"cmp"
	"math"
	"slices"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/lexicon"
)

// word is one vocabulary entry before it is written.
type word struct {
	lower, display string
	level          int // SCOWL level, 99 when not in SCOWL
	count          float64
	p              float64
	flags          uint8
}

// levelPrior is the pseudo-count a word gets for its SCOWL level, so words
// the corpus never saw still rank by how common SCOWL says they are.
func levelPrior(level int) float64 {
	switch {
	case level <= 10:
		return 20
	case level <= 20:
		return 8
	case level <= 35:
		return 3
	case level <= 40:
		return 1.5
	case level <= 50:
		return 0.8
	case level <= 55:
		return 0.5
	case level <= 60:
		return 0.35
	case level <= 70:
		return 0.2
	}
	return 0.1
}

type vocabOptions struct {
	maxLevel      int // SCOWL words up to this level always count
	extendedLevel int // words up to this level count when the corpus has them
	corpusOnlyMin int // corpus words not in SCOWL need this many occurrences
	maxWords      int
	properRank    int // proper nouns are capped at the count of this rank
}

// chooseVocab picks the words and their spellings, counts, and flags.
func chooseVocab(c *corpus, scowl scowlWords, extra []string, o vocabOptions) []word {
	groups := map[string][]string{} // lower → SCOWL spellings
	for w := range scowl {
		lw := strings.ToLower(w)
		groups[lw] = append(groups[lw], w)
	}
	for _, g := range groups {
		slices.Sort(g)
	}
	isExtra := map[string]bool{}
	for _, w := range extra {
		isExtra[strings.ToLower(w)] = true
		if _, ok := groups[strings.ToLower(w)]; !ok {
			groups[strings.ToLower(w)] = []string{w}
		}
	}
	count := func(lower string) int {
		if id, ok := c.index[lower]; ok {
			return c.forms[id].count
		}
		return 0
	}
	var out []word
	consider := func(lower string, spellings []string) {
		level := 99
		for _, s := range spellings {
			if l, ok := scowl[s]; ok && l < level {
				level = l
			}
		}
		if isExtra[lower] {
			level = min(level, 35)
		}
		n := count(lower)
		switch {
		case level <= o.maxLevel:
		case level <= o.extendedLevel && n >= 2:
		case level == 99 && n >= o.corpusOnlyMin:
		default:
			return
		}
		if strings.HasSuffix(lower, "'s") && n < 2 {
			return // possessives only when the corpus uses them
		}
		k, ok := lexicon.KeyOf(lower)
		if !ok || len(k) > lexicon.MaxKeyLen {
			return
		}
		display := spellingOf(c, lower, spellings)
		if display == "" {
			return
		}
		out = append(out, word{lower: lower, display: display, level: level, count: float64(n)})
	}
	for lower, sp := range groups {
		consider(lower, sp)
	}
	for id, lower := range c.lower {
		if _, ok := groups[lower]; ok {
			continue
		}
		if f := c.forms[id]; f.count >= o.corpusOnlyMin {
			consider(lower, nil)
		}
	}

	// Fixed order, so float sums and the output do not depend on map order.
	slices.SortFunc(out, func(a, b word) int { return cmp.Compare(a.display, b.display) })

	// Cap proper nouns: Tatoeba's "Tom" and "Mary" would otherwise rank with "the".
	counts := make([]float64, len(out))
	for i, w := range out {
		counts[i] = w.count
	}
	slices.SortFunc(counts, func(a, b float64) int { return cmp.Compare(b, a) })
	capCount := counts[min(o.properRank, len(counts)-1)]
	for i := range out {
		if isProper(out[i].display) && out[i].count > capCount {
			out[i].count = capCount
		}
	}

	var total float64
	for i := range out {
		total += out[i].count + levelPrior(out[i].level)
	}
	for i := range out {
		w := &out[i]
		w.p = (w.count + levelPrior(w.level)) / total
	}
	slices.SortFunc(out, func(a, b word) int { return cmp.Or(cmp.Compare(b.p, a.p), cmp.Compare(a.display, b.display)) })
	if len(out) > o.maxWords {
		out = out[:o.maxWords]
	}
	// Renormalize after the cut.
	total = 0
	for _, w := range out {
		total += w.p
	}
	for i := range out {
		w := &out[i]
		w.p /= total
		w.flags = flagsOf(w)
	}
	return out
}

// spellingOf picks how a word is written: the corpus's mid-sentence
// spelling when it is capitalized most of the time ("I", "Monday", "Tom"),
// else lower case when SCOWL or the corpus has it, else SCOWL's spelling.
func spellingOf(c *corpus, lower string, spellings []string) string {
	if id, ok := c.index[lower]; ok {
		f := c.forms[id]
		if f.mid >= 10 && float64(f.midCap) > 0.8*float64(f.mid) {
			best, bestN := "", 0
			for s, n := range f.spellings {
				r, _ := utf8.DecodeRuneInString(s)
				if unicode.IsUpper(r) && (n > bestN || (n == bestN && s < best)) {
					best, bestN = s, n
				}
			}
			if best != "" {
				return best
			}
		}
		if f.mid >= 10 {
			return lower
		}
	}
	for _, s := range spellings {
		if s == lower {
			return lower
		}
	}
	if len(spellings) > 0 {
		return spellings[0]
	}
	return lower
}

func isProper(display string) bool {
	r, _ := utf8.DecodeRuneInString(display)
	return unicode.IsUpper(r) && display != "I" && !strings.HasPrefix(display, "I'")
}

func flagsOf(w *word) uint8 {
	var f uint8
	r, _ := utf8.DecodeRuneInString(w.display)
	if unicode.IsUpper(r) {
		f |= lexicon.FlagProper
	}
	if strings.ContainsRune(w.display, '\'') {
		f |= lexicon.FlagContraction
	}
	if utf8.RuneCountInString(w.display) >= 2 && w.display == strings.ToUpper(w.display) {
		f |= lexicon.FlagAllCaps
	}
	if w.count == 0 {
		f |= lexicon.FlagUnseen
	}
	if (w.count < 3 && w.level > 50) || (len(w.lower) == 1 && w.lower != "a" && w.lower != "i") {
		f |= lexicon.FlagNoAutocorrect
	}
	return f
}

// ---- n-grams ----

const (
	idBits = 21
	idMask = 1<<idBits - 1
	bos    = idMask // the sentence start in packed keys
)

func pack2(a, b int) uint64    { return uint64(a)<<idBits | uint64(b) }
func pack3(a, b, c int) uint64 { return uint64(a)<<(2*idBits) | uint64(b)<<idBits | uint64(c) }

type ngramOptions struct {
	minBigram, minTrigram   int
	maxBigrams, maxTrigrams int
	pruneNats               float64 // drop trigrams this close to their backoff
}

type ngrams struct {
	bigrams  []lexicon.Bigram
	trigrams []lexicon.Trigram
	bow2     []float64 // per word; the last is the sentence start
	triBow   map[[2]int]float64
}

// estimate counts n-grams over the corpus and turns them into interpolated
// absolute-discounting estimates in backoff form (ARPA semantics): a stored
// n-gram has its interpolated probability; an unstored one is the context's
// backoff weight times the lower order.
func estimate(c *corpus, vocab []word, o ngramOptions) ngrams {
	formToWord := make([]int32, len(c.forms))
	for i := range formToWord {
		formToWord[i] = -1
	}
	for i, w := range vocab {
		formToWord[c.index[w.lower]] = int32(i)
	}
	// Proper nouns had their counts capped; scale their n-grams the same way.
	scale := make([]float64, len(vocab))
	for i, w := range vocab {
		scale[i] = 1
		if raw := float64(c.forms[c.index[w.lower]].count); raw > 0 && w.count < raw {
			scale[i] = w.count / raw
		}
	}

	bi := map[uint64]float64{}
	tri := map[uint64]float64{}
	for _, s := range c.sentences {
		p2, p1 := -1, bos
		for _, f := range s {
			w := -1
			if f >= 0 {
				w = int(formToWord[f])
			}
			if w < 0 {
				p2, p1 = -1, -1
				continue
			}
			if p1 >= 0 {
				bi[pack2(p1, w)] += scale[w]
				if p2 >= 0 {
					tri[pack3(p2, p1, w)] += scale[w]
				}
			}
			p2, p1 = p1, w
		}
	}

	n := len(vocab)
	idx := func(id int) int { // packed id → Builder index
		if id == bos {
			return lexicon.BOSIndex
		}
		return id
	}
	p1 := func(w int) float64 { return vocab[w].p }

	// Bigrams.
	d2 := discount(bi)
	type stat struct{ total, distinct float64 }
	ctx2 := map[int]*stat{}
	biKeys := sortedKeys(bi)
	triKeys := sortedKeys(tri)
	for _, k := range biKeys {
		cnt := bi[k]
		v := int(k >> idBits)
		s := ctx2[v]
		if s == nil {
			s = &stat{}
			ctx2[v] = s
		}
		s.total += cnt
		s.distinct++
	}
	keep2 := topKeys(bi, float64(o.minBigram), o.maxBigrams)
	p2 := make(map[uint64]float64, len(keep2))
	sumP2 := map[int]float64{}
	sumP1 := map[int]float64{}
	for _, k := range keep2 {
		v, w := int(k>>idBits), int(k&idMask)
		s := ctx2[v]
		p := math.Max(bi[k]-d2, 0)/s.total + d2*s.distinct/s.total*p1(w)
		p2[k] = p
		sumP2[v] += p
		sumP1[v] += p1(w)
	}
	bow2 := make([]float64, n+1)
	bowOf := func(v int) *float64 {
		if v == bos {
			return &bow2[n]
		}
		return &bow2[v]
	}
	for v := range ctx2 {
		*bowOf(v) = backoff(sumP2[v], sumP1[v])
	}
	for v := range bow2 {
		if bow2[v] == 0 {
			bow2[v] = 1
		}
	}
	pbo2 := func(v, w int) float64 {
		if p, ok := p2[pack2(v, w)]; ok {
			return p
		}
		return *bowOf(v) * p1(w)
	}

	// Trigrams, only for stored bigram contexts.
	d3 := discount(tri)
	ctx3 := map[uint64]*stat{}
	for _, k := range triKeys {
		cnt := tri[k]
		uv := k >> idBits
		s := ctx3[uv]
		if s == nil {
			s = &stat{}
			ctx3[uv] = s
		}
		s.total += cnt
		s.distinct++
	}
	var cand3 []uint64
	for _, k := range triKeys {
		if tri[k] < float64(o.minTrigram) {
			continue
		}
		if _, ok := p2[k>>idBits]; !ok {
			continue
		}
		cand3 = append(cand3, k)
	}
	type tp struct {
		k uint64
		p float64
	}
	var kept3 []tp
	for _, k := range cand3 {
		uv := k >> idBits
		v, w := int(uv&idMask), int(k&idMask)
		s := ctx3[uv]
		lower := pbo2(v, w)
		p := math.Max(tri[k]-d3, 0)/s.total + d3*s.distinct/s.total*lower
		if math.Abs(math.Log(p)-math.Log(lower)) < o.pruneNats {
			continue
		}
		kept3 = append(kept3, tp{k, p})
	}
	slices.SortFunc(kept3, func(a, b tp) int { return cmp.Or(cmp.Compare(tri[b.k], tri[a.k]), cmp.Compare(a.k, b.k)) })
	if len(kept3) > o.maxTrigrams {
		kept3 = kept3[:o.maxTrigrams]
	}
	sumP3 := map[uint64]float64{}
	sumLow := map[uint64]float64{}
	out := ngrams{bow2: bow2, triBow: map[[2]int]float64{}}
	for _, t := range kept3 {
		uv := t.k >> idBits
		u, v, w := int(uv>>idBits), int(uv&idMask), int(t.k&idMask)
		sumP3[uv] += t.p
		sumLow[uv] += pbo2(v, w)
		out.trigrams = append(out.trigrams, lexicon.Trigram{W1: idx(u), W2: idx(v), W3: w, Cost: -math.Log(t.p)})
	}
	for uv, sp := range sumP3 {
		u, v := int(uv>>idBits), int(uv&idMask)
		out.triBow[[2]int{idx(u), idx(v)}] = -math.Log(backoff(sp, sumLow[uv]))
	}
	for _, k := range keep2 {
		v, w := int(k>>idBits), int(k&idMask)
		out.bigrams = append(out.bigrams, lexicon.Bigram{W1: idx(v), W2: w, Cost: -math.Log(p2[k])})
	}
	slices.SortFunc(out.trigrams, func(a, b lexicon.Trigram) int {
		return cmp.Or(cmp.Compare(a.W1, b.W1), cmp.Compare(a.W2, b.W2), cmp.Compare(a.W3, b.W3))
	})
	return out
}

// discount is n1 / (n1 + 2 n2), from the counts of n-grams seen once and twice.
func discount(m map[uint64]float64) float64 {
	var n1, n2 float64
	for _, c := range m {
		switch {
		case c > 0.5 && c < 1.5:
			n1++
		case c >= 1.5 && c < 2.5:
			n2++
		}
	}
	if n1+2*n2 == 0 {
		return 0.5
	}
	return n1 / (n1 + 2*n2)
}

// backoff is the weight that keeps a context's distribution summing to one:
// the probability left by stored n-grams over the lower-order mass of the
// rest. It is kept in (1e-6, 1].
func backoff(stored, lower float64) float64 {
	left, rest := 1-stored, 1-lower
	if left <= 0 || rest <= 1e-12 {
		return 1e-6
	}
	return math.Min(1, math.Max(left/rest, 1e-6))
}

// topKeys is the keys with at least min count, most frequent first, at most limit.
func topKeys(m map[uint64]float64, minCount float64, limit int) []uint64 {
	var keys []uint64
	for k, c := range m {
		if c >= minCount {
			keys = append(keys, k)
		}
	}
	slices.SortFunc(keys, func(a, b uint64) int { return cmp.Or(cmp.Compare(m[b], m[a]), cmp.Compare(a, b)) })
	if len(keys) > limit {
		keys = keys[:limit]
	}
	return keys
}

func sortedKeys(m map[uint64]float64) []uint64 {
	keys := make([]uint64, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	return keys
}
func ln(x float64) float64 { return math.Log(x) }
