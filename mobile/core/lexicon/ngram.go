package lexicon

// Context is the two words before the next one, resolved once so that many
// next words can be scored against it. The zero value is no context.
type Context struct {
	W1, W2       WordID
	biLo, biHi   uint32 // bigram row of W2
	triLo, triHi uint32 // trigram row of (W1, W2)
	bow2, bow3   float32
}

// NewContext resolves the context (w1, w2). Either may be NoWord; BOS is the
// sentence start.
func (m *Model) NewContext(w1, w2 WordID) Context {
	c := Context{W1: w1, W2: w2}
	if w2 == NoWord || w2 > m.n {
		c.W1, c.W2 = NoWord, NoWord
		return c
	}
	c.biLo, c.biHi = m.biRow[w2], m.biRow[w2+1]
	c.bow2 = float32(m.words[w2].Bow) * m.bowStep
	if w1 == NoWord || w1 > m.n {
		c.W1 = NoWord
		return c
	}
	bi, ok := m.bigramIndex(w1, w2)
	if !ok {
		return c
	}
	lo, hi := 0, len(m.triCtx)-1
	for lo < hi {
		mid := (lo + hi) / 2
		if m.triCtx[mid].Bigram < bi {
			lo = mid + 1
		} else {
			hi = mid
		}
	}
	if lo < len(m.triCtx)-1 && m.triCtx[lo].Bigram == bi {
		c.triLo, c.triHi = m.triCtx[lo].RowOff, m.triCtx[lo+1].RowOff
		c.bow3 = float32(m.triCtx[lo].Bow) * m.bowStep
	}
	return c
}

func (m *Model) bigramIndex(w1, w2 WordID) (uint32, bool) {
	return search(m.biNext, m.biRow[w1], m.biRow[w1+1], w2)
}

func search(s []uint32, lo, hi uint32, x uint32) (uint32, bool) {
	for lo < hi {
		mid := (lo + hi) / 2
		if s[mid] < x {
			lo = mid + 1
		} else {
			hi = mid
		}
	}
	return lo, lo < uint32(len(s)) && s[lo] == x
}

// Cost is -ln p(w | context), with backoff to lower orders.
func (m *Model) Cost(c *Context, w WordID) float32 {
	if c.W2 == NoWord {
		return m.Unigram(w)
	}
	if c.triHi > c.triLo {
		if i, ok := search(m.triNext, c.triLo, c.triHi, w); ok && i < c.triHi {
			return float32(m.triCost[i]) * m.costStep
		}
	}
	if i, ok := search(m.biNext, c.biLo, c.biHi, w); ok && i < c.biHi {
		return c.bow3 + float32(m.biCost[i])*m.costStep
	}
	return c.bow3 + c.bow2 + m.Unigram(w)
}

// Order is the n-gram order that scored w in the context: 3, 2, or 1.
func (m *Model) Order(c *Context, w WordID) int {
	if c.W2 == NoWord {
		return 1
	}
	if i, ok := search(m.triNext, c.triLo, c.triHi, w); ok && i < c.triHi {
		return 3
	}
	if i, ok := search(m.biNext, c.biLo, c.biHi, w); ok && i < c.biHi {
		return 2
	}
	return 1
}

// Successors calls fn for each word the context has an explicit trigram or
// bigram for, with its cost; a word in both rows comes twice, trigram first.
// fn returns false to stop.
func (m *Model) Successors(c *Context, fn func(w WordID, cost float32) bool) {
	if c.W2 == NoWord {
		return
	}
	for i := c.triLo; i < c.triHi; i++ {
		if !fn(m.triNext[i], float32(m.triCost[i])*m.costStep) {
			return
		}
	}
	for i := c.biLo; i < c.biHi; i++ {
		if !fn(m.biNext[i], c.bow3+float32(m.biCost[i])*m.costStep) {
			return
		}
	}
}
