package lexicon

import (
	"encoding/binary"
	"errors"
	"fmt"
	"hash/crc32"
	"math"
	"unsafe"
)

// Model is an open .sslm file. Its methods do not allocate and are safe for
// concurrent use; Close must not run at the same time as them.
type Model struct {
	data     []byte
	release  func() error
	n        uint32 // words, BOS excluded
	words    []wordRec
	text     []byte
	nodes    []Node
	biRow    []uint32
	biNext   []uint32
	biCost   []uint8
	triCtx   []triCtx // with a sentinel at the end
	triNext  []uint32
	triCost  []uint8
	costStep float32
	bowStep  float32
	locale   [8]byte
	tokens   uint32
}

// ErrFormat wraps every reason a file is not a valid model.
var ErrFormat = errors.New("lexicon: not a valid .sslm model")

func formatErr(format string, args ...any) error {
	return fmt.Errorf("%w: %s", ErrFormat, fmt.Sprintf(format, args...))
}

// FromBytes reads a model from b, which must stay unchanged while the model
// is in use. It checks the checksum and every index, so later lookups cannot
// go out of bounds.
func FromBytes(b []byte) (*Model, error) {
	if !littleEndian() {
		return nil, errors.New("lexicon: big-endian hosts are not supported")
	}
	if len(b) < dataStart || string(b[:4]) != Magic {
		return nil, formatErr("bad magic")
	}
	le := binary.LittleEndian
	if v := le.Uint16(b[4:]); v != Version {
		return nil, formatErr("version %d, want %d", v, Version)
	}
	if le.Uint32(b[28:]) != sectionCount {
		return nil, formatErr("section count")
	}
	if total := le.Uint32(b[52:]); uint64(total) != uint64(len(b)) {
		return nil, formatErr("length %d, header says %d", len(b), total)
	}
	if crc32.Checksum(b[headerSize:], castagnoli) != le.Uint32(b[48:]) {
		return nil, formatErr("checksum mismatch")
	}
	if uintptr(unsafe.Pointer(&b[0]))%8 != 0 {
		// Mapped files are page aligned; a heap copy keeps the casts below safe.
		c := make([]uint64, (len(b)+7)/8)
		cb := unsafe.Slice((*byte)(unsafe.Pointer(&c[0])), len(b))
		copy(cb, b)
		b = cb
	}
	m := &Model{data: b}
	m.n = le.Uint32(b[8:])
	nodeCount := le.Uint32(b[12:])
	biCount := le.Uint32(b[16:])
	ctxCount := le.Uint32(b[20:])
	triCount := le.Uint32(b[24:])
	copy(m.locale[:], b[32:40])
	m.costStep = math.Float32frombits(le.Uint32(b[40:]))
	m.bowStep = math.Float32frombits(le.Uint32(b[44:]))
	m.tokens = le.Uint32(b[56:])
	if !(m.costStep > 0 && m.costStep < 1) || !(m.bowStep > 0 && m.bowStep < 1) || m.n == 0 || m.n >= 1<<24 {
		return nil, formatErr("header values")
	}

	var secs [sectionCount][]byte
	for i := range secs {
		off, ln := le.Uint32(b[headerSize+i*8:]), le.Uint32(b[headerSize+i*8+4:])
		if off < dataStart || off%8 != 0 || uint64(off)+uint64(ln) > uint64(len(b)) {
			return nil, formatErr("section %d bounds", i)
		}
		secs[i] = b[off : off+ln]
	}
	var err error
	if m.words, err = view[wordRec](secs[secWords], uint64(m.n)+1); err != nil {
		return nil, err
	}
	m.text = secs[secText]
	if m.nodes, err = view[Node](secs[secNodes], uint64(nodeCount)); err != nil {
		return nil, err
	}
	if m.biRow, err = view[uint32](secs[secBigramRow], uint64(m.n)+2); err != nil {
		return nil, err
	}
	if m.biNext, err = view[uint32](secs[secBigramNext], uint64(biCount)); err != nil {
		return nil, err
	}
	if m.biCost, err = view[uint8](secs[secBigramCost], uint64(biCount)); err != nil {
		return nil, err
	}
	if m.triCtx, err = view[triCtx](secs[secTriCtx], uint64(ctxCount)+1); err != nil {
		return nil, err
	}
	if m.triNext, err = view[uint32](secs[secTriNext], uint64(triCount)); err != nil {
		return nil, err
	}
	if m.triCost, err = view[uint8](secs[secTriCost], uint64(triCount)); err != nil {
		return nil, err
	}
	if err := m.validate(); err != nil {
		return nil, err
	}
	return m, nil
}

func view[T any](b []byte, count uint64) ([]T, error) {
	var zero T
	size := uint64(unsafe.Sizeof(zero))
	if uint64(len(b)) != count*size {
		return nil, formatErr("section holds %d bytes, want %d", len(b), count*size)
	}
	if count == 0 {
		return []T{}, nil
	}
	return unsafe.Slice((*T)(unsafe.Pointer(&b[0])), count), nil
}

func (m *Model) validate() error {
	words := uint32(len(m.words))
	for i, w := range m.words {
		if uint64(w.TextOff)+uint64(w.TextLen) > uint64(len(m.text)) || w.TextLen == 0 {
			return formatErr("word %d text", i)
		}
	}
	nodes := uint32(len(m.nodes))
	if nodes == 0 {
		return formatErr("no trie")
	}
	for i, nd := range m.nodes {
		if uint64(nd.FirstChild)+uint64(nd.ChildCount) > uint64(nodes) || (nd.ChildCount > 0 && nd.FirstChild <= uint32(i)) ||
			uint64(nd.WordLo)+uint64(nd.TermCount) > uint64(m.n) {
			return formatErr("node %d", i)
		}
	}
	if m.biRow[0] != 0 || m.biRow[len(m.biRow)-1] != uint32(len(m.biNext)) {
		return formatErr("bigram rows")
	}
	for i := 1; i < len(m.biRow); i++ {
		if m.biRow[i] < m.biRow[i-1] {
			return formatErr("bigram row %d", i)
		}
	}
	for _, w := range m.biNext {
		if w >= words {
			return formatErr("bigram word")
		}
	}
	last := m.triCtx[len(m.triCtx)-1]
	if last.RowOff != uint32(len(m.triNext)) || (len(m.triCtx) > 1 && m.triCtx[0].RowOff != 0) {
		return formatErr("trigram rows")
	}
	for i := 0; i+1 < len(m.triCtx); i++ {
		c := m.triCtx[i]
		if c.Bigram >= uint32(len(m.biNext)) || m.triCtx[i+1].RowOff < c.RowOff || (i > 0 && c.Bigram <= m.triCtx[i-1].Bigram) {
			return formatErr("trigram context %d", i)
		}
	}
	for _, w := range m.triNext {
		if w >= words {
			return formatErr("trigram word")
		}
	}
	return nil
}

func littleEndian() bool {
	x := uint16(1)
	return *(*byte)(unsafe.Pointer(&x)) == 1
}

// Close releases the mapping. The model must not be used after it.
func (m *Model) Close() error {
	if m.release == nil {
		return nil
	}
	err := m.release()
	m.release = nil
	return err
}

// Size is the model's size in bytes.
func (m *Model) Size() int { return len(m.data) }

// Locale is the model's locale tag, such as "en-US".
func (m *Model) Locale() string {
	n := 0
	for n < len(m.locale) && m.locale[n] != 0 {
		n++
	}
	return string(m.locale[:n])
}

// WordCount is the number of words, the sentence start excluded.
func (m *Model) WordCount() int { return int(m.n) }

// NodeCount, BigramCount, and TrigramCount describe the model.
func (m *Model) NodeCount() int    { return len(m.nodes) }
func (m *Model) BigramCount() int  { return len(m.biNext) }
func (m *Model) TrigramCount() int { return len(m.triNext) }

// BOS is the sentence-start word.
func (m *Model) BOS() WordID { return m.n }

// Word is the spelling of a word. The slice points into the model.
func (m *Model) Word(id WordID) []byte {
	w := &m.words[id]
	return m.text[w.TextOff : w.TextOff+uint32(w.TextLen)]
}

// Flags is a word's Flag* bits.
func (m *Model) Flags(id WordID) uint8 { return m.words[id].Flags }

// Unigram is a word's cost, -ln p(w).
func (m *Model) Unigram(id WordID) float32 { return float32(m.words[id].Uni) * m.costStep }

// CostStep is the nats one stored cost unit is worth.
func (m *Model) CostStep() float32 { return m.costStep }

// Node is a trie node by id.
func (m *Model) Node(id NodeID) *Node { return &m.nodes[id] }

// Child is the child of a node for the letter c.
func (m *Model) Child(id NodeID, c byte) (NodeID, bool) {
	nd := &m.nodes[id]
	lo, hi := nd.FirstChild, nd.FirstChild+uint32(nd.ChildCount)
	for lo < hi {
		mid := (lo + hi) / 2
		switch ch := m.nodes[mid].Char; {
		case ch == c:
			return mid, true
		case ch < c:
			lo = mid + 1
		default:
			hi = mid
		}
	}
	return 0, false
}

// BestWord is the most probable word under a node: it follows the children
// whose BestCost matches until a word has it.
func (m *Model) BestWord(id NodeID) WordID {
	for {
		nd := &m.nodes[id]
		for i := uint32(0); i < uint32(nd.TermCount); i++ {
			if m.words[nd.WordLo+i].Uni == nd.BestCost {
				return nd.WordLo + i
			}
		}
		next, found := id, false
		for c := nd.FirstChild; c < nd.FirstChild+uint32(nd.ChildCount); c++ {
			if m.nodes[c].BestCost == nd.BestCost {
				next, found = c, true
				break
			}
		}
		if !found {
			return nd.WordLo // unreachable in a valid file
		}
		id = next
	}
}

// Walk follows key from the root.
func (m *Model) Walk(key []byte) (NodeID, bool) {
	var id NodeID
	for _, c := range key {
		var ok bool
		if id, ok = m.Child(id, c); !ok {
			return 0, false
		}
	}
	return id, true
}

// Lookup is the words whose key is key: [lo, lo+n), most probable first.
func (m *Model) Lookup(key []byte) (lo WordID, n int) {
	id, ok := m.Walk(key)
	if !ok {
		return 0, 0
	}
	nd := &m.nodes[id]
	return nd.WordLo, int(nd.TermCount)
}

// Find is the word spelled exactly as word, ignoring case when fold is set.
func (m *Model) Find(word []byte, fold bool) (WordID, bool) {
	var buf [MaxKeyLen * 2]byte
	key, ok := AppendKey(buf[:0], word)
	if !ok || len(key) > MaxKeyLen {
		return NoWord, false
	}
	lo, n := m.Lookup(key)
	for i := lo; i < lo+WordID(n); i++ {
		if w := m.Word(i); string(w) == string(word) || (fold && equalFold(w, word)) {
			return i, true
		}
	}
	return NoWord, false
}

func equalFold(a, b []byte) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		x, y := a[i], b[i]
		if x >= 'A' && x <= 'Z' {
			x += 'a' - 'A'
		}
		if y >= 'A' && y <= 'Z' {
			y += 'a' - 'A'
		}
		if x != y {
			return false
		}
	}
	return true
}

// AppendKeyOf appends the key of a word to dst by walking its spelling.
func (m *Model) AppendKeyOf(dst []byte, id WordID) []byte {
	k, _ := AppendKey(dst, m.Word(id))
	return k
}
