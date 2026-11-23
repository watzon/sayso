package lexicon

import (
	"bytes"
	"cmp"
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"io"
	"math"
	"slices"
	"unicode/utf8"
)

// Entry is one word given to a Builder.
type Entry struct {
	Display string  // the spelling to write
	Flags   uint8   // Flag* bits
	Cost    float64 // unigram cost, -ln p
	Bow     float64 // backoff cost as a bigram context, -ln bow
}

// Bigram and Trigram name words by their index in Builder.Entries; BOSIndex
// is the sentence start.
type Bigram struct {
	W1, W2 int
	Cost   float64 // -ln p(w2 | w1)
}

type Trigram struct {
	W1, W2, W3 int
	Cost       float64 // -ln p(w3 | w1 w2)
}

// BOSIndex names the sentence start in n-grams.
const BOSIndex = -1

// Builder collects a model and writes it as .sslm.
type Builder struct {
	Locale   string
	Entries  []Entry
	BOSBow   float64 // backoff cost of the sentence start as a context
	Bigrams  []Bigram
	Trigrams []Trigram
	// TriBow is the backoff cost of each trigram context (w1, w2).
	TriBow map[[2]int]float64
	// CorpusTokens is informational.
	CorpusTokens uint32
	CostStep     float64 // 0 means DefaultCostStep
	BowStep      float64 // 0 means DefaultBowStep
}

type sortedWord struct {
	key, display string
	entry        int
	uni          uint8
}

// Write lays the model out and writes it. Words that have no key or a key
// longer than MaxKeyLen are an error, as are two entries with one spelling.
func (b *Builder) Write(w io.Writer) error {
	costStep, bowStep := b.CostStep, b.BowStep
	if costStep == 0 {
		costStep = DefaultCostStep
	}
	if bowStep == 0 {
		bowStep = DefaultBowStep
	}
	n := len(b.Entries)
	if n == 0 || n >= 1<<24 {
		return fmt.Errorf("lexicon: %d words", n)
	}
	words := make([]sortedWord, n)
	seen := map[string]bool{}
	for i, e := range b.Entries {
		k, ok := KeyOf(e.Display)
		if !ok || len(k) > MaxKeyLen || len(e.Display) > 255 || !utf8.ValidString(e.Display) {
			return fmt.Errorf("lexicon: bad word %q", e.Display)
		}
		if seen[e.Display] {
			return fmt.Errorf("lexicon: duplicate word %q", e.Display)
		}
		seen[e.Display] = true
		words[i] = sortedWord{key: k, display: e.Display, entry: i, uni: quantize(e.Cost, costStep)}
	}
	slices.SortFunc(words, func(x, y sortedWord) int {
		return cmp.Or(cmp.Compare(x.key, y.key), cmp.Compare(x.uni, y.uni), cmp.Compare(x.display, y.display))
	})
	idOf := make([]uint32, n) // entry index → word id
	for id, sw := range words {
		idOf[sw.entry] = uint32(id)
	}
	bos := uint32(n)
	id := func(i int) uint32 {
		if i == BOSIndex {
			return bos
		}
		return idOf[i]
	}

	// Words and text.
	var text bytes.Buffer
	recs := make([]wordRec, n+1)
	for wid, sw := range words {
		e := b.Entries[sw.entry]
		recs[wid] = wordRec{TextOff: uint32(text.Len()), TextLen: uint8(len(e.Display)), Flags: e.Flags, Uni: sw.uni, Bow: quantize(e.Bow, bowStep)}
		text.WriteString(e.Display)
	}
	recs[n] = wordRec{TextOff: uint32(text.Len()), TextLen: 3, Flags: FlagNoSuggest, Uni: 255, Bow: quantize(b.BOSBow, bowStep)}
	text.WriteString("<s>")

	keys := make([]string, n)
	for i, sw := range words {
		keys[i] = sw.key
	}
	nodes := buildTrie(keys, recs)

	// Bigrams, rows by w1 and sorted by w2.
	type bi struct {
		w1, w2 uint32
		cost   uint8
	}
	bis := make([]bi, len(b.Bigrams))
	for i, g := range b.Bigrams {
		bis[i] = bi{id(g.W1), id(g.W2), quantize(g.Cost, costStep)}
	}
	slices.SortFunc(bis, func(x, y bi) int { return cmp.Or(cmp.Compare(x.w1, y.w1), cmp.Compare(x.w2, y.w2)) })
	for i := 1; i < len(bis); i++ {
		if bis[i].w1 == bis[i-1].w1 && bis[i].w2 == bis[i-1].w2 {
			return errors.New("lexicon: duplicate bigram")
		}
	}
	rowOff := make([]uint32, n+2)
	biNext := make([]uint32, len(bis))
	biCost := make([]uint8, len(bis))
	for i, g := range bis {
		rowOff[g.w1+1]++
		biNext[i] = g.w2
		biCost[i] = g.cost
	}
	for i := 1; i < len(rowOff); i++ {
		rowOff[i] += rowOff[i-1]
	}
	bigramIndex := func(w1, w2 uint32) (uint32, bool) {
		lo, hi := rowOff[w1], rowOff[w1+1]
		j, ok := slices.BinarySearch(biNext[lo:hi], w2)
		return lo + uint32(j), ok
	}

	// Trigrams, grouped by the index of their (w1, w2) bigram.
	type tri struct {
		ctx, w3 uint32
		cost    uint8
	}
	tris := make([]tri, 0, len(b.Trigrams))
	for _, g := range b.Trigrams {
		ci, ok := bigramIndex(id(g.W1), id(g.W2))
		if !ok {
			return fmt.Errorf("lexicon: trigram context %d %d has no bigram", g.W1, g.W2)
		}
		tris = append(tris, tri{ci, id(g.W3), quantize(g.Cost, costStep)})
	}
	slices.SortFunc(tris, func(x, y tri) int { return cmp.Or(cmp.Compare(x.ctx, y.ctx), cmp.Compare(x.w3, y.w3)) })
	var ctxs []triCtx
	triNext := make([]uint32, len(tris))
	triCost := make([]uint8, len(tris))
	for i, g := range tris {
		if i > 0 && g.ctx == tris[i-1].ctx && g.w3 == tris[i-1].w3 {
			return errors.New("lexicon: duplicate trigram")
		}
		if i == 0 || g.ctx != tris[i-1].ctx {
			ctxs = append(ctxs, triCtx{Bigram: g.ctx, RowOff: uint32(i)})
		}
		triNext[i] = g.w3
		triCost[i] = g.cost
	}
	for k, bow := range b.TriBow {
		ci, ok := bigramIndex(id(k[0]), id(k[1]))
		if !ok {
			continue
		}
		if j, found := slices.BinarySearchFunc(ctxs, ci, func(c triCtx, t uint32) int { return cmp.Compare(c.Bigram, t) }); found {
			ctxs[j].Bow = quantize(bow, bowStep)
		}
	}
	ctxs = append(ctxs, triCtx{Bigram: ^uint32(0), RowOff: uint32(len(tris))}) // sentinel

	// Lay out.
	var sections [sectionCount][]byte
	sections[secWords] = encode(recs)
	sections[secText] = text.Bytes()
	sections[secNodes] = encode(nodes)
	sections[secBigramRow] = encode(rowOff)
	sections[secBigramNext] = encode(biNext)
	sections[secBigramCost] = biCost
	sections[secTriCtx] = encode(ctxs)
	sections[secTriNext] = encode(triNext)
	sections[secTriCost] = triCost
	sections[secReserved] = nil

	body := make([]byte, tableSize)
	off := uint32(dataStart)
	for i, s := range sections {
		for off%8 != 0 {
			body = append(body, 0)
			off++
		}
		binary.LittleEndian.PutUint32(body[i*8:], off)
		binary.LittleEndian.PutUint32(body[i*8+4:], uint32(len(s)))
		body = append(body, s...)
		off += uint32(len(s))
	}
	for len(body)%8 != 0 {
		body = append(body, 0)
	}

	hdr := make([]byte, headerSize)
	copy(hdr, Magic)
	binary.LittleEndian.PutUint16(hdr[4:], Version)
	binary.LittleEndian.PutUint32(hdr[8:], uint32(n))
	binary.LittleEndian.PutUint32(hdr[12:], uint32(len(nodes)))
	binary.LittleEndian.PutUint32(hdr[16:], uint32(len(bis)))
	binary.LittleEndian.PutUint32(hdr[20:], uint32(len(ctxs)-1))
	binary.LittleEndian.PutUint32(hdr[24:], uint32(len(tris)))
	binary.LittleEndian.PutUint32(hdr[28:], sectionCount)
	copy(hdr[32:40], b.Locale)
	binary.LittleEndian.PutUint32(hdr[40:], mathFloat32bits(costStep))
	binary.LittleEndian.PutUint32(hdr[44:], mathFloat32bits(bowStep))
	binary.LittleEndian.PutUint32(hdr[48:], crc32.Checksum(body, castagnoli))
	binary.LittleEndian.PutUint32(hdr[52:], uint32(headerSize+len(body)))
	binary.LittleEndian.PutUint32(hdr[56:], b.CorpusTokens)
	if _, err := w.Write(hdr); err != nil {
		return err
	}
	_, err := w.Write(body)
	return err
}

var castagnoli = crc32.MakeTable(crc32.Castagnoli)

// buildTrie makes the key trie of sorted keys in breadth-first order, so the
// children of a node are contiguous.
func buildTrie(keys []string, recs []wordRec) []Node {
	type span struct {
		depth  int
		lo, hi int // words under the node
	}
	nodes := []Node{{}}
	spans := []span{{0, 0, len(keys)}}
	for q := 0; q < len(nodes); q++ {
		s := spans[q]
		nd := &nodes[q]
		nd.WordLo = uint32(s.lo)
		best := s.lo
		for i := s.lo; i < s.hi; i++ {
			if recs[i].Uni < recs[best].Uni {
				best = i
			}
		}
		nd.BestCost = recs[best].Uni
		i := s.lo
		for i < s.hi && len(keys[i]) == s.depth {
			i++
		}
		nd.TermCount = uint8(min(i-s.lo, 255))
		nd.FirstChild = uint32(len(nodes))
		for i < s.hi {
			c := keys[i][s.depth]
			j := i
			for j < s.hi && keys[j][s.depth] == c {
				j++
			}
			nodes = append(nodes, Node{Char: c})
			spans = append(spans, span{s.depth + 1, i, j})
			nodes[q].ChildCount++
			i = j
		}
	}
	return nodes
}

func encode(v any) []byte {
	b, err := binary.Append(nil, binary.LittleEndian, v)
	if err != nil {
		panic(err) // only fixed-size types reach here
	}
	return b
}

func mathFloat32bits(x float64) uint32 { return math.Float32bits(float32(x)) }
