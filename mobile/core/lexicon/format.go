// Package lexicon reads and writes .sslm language models (FORMAT.md): a
// sorted word list with a key trie, quantized unigram, bigram, and pruned
// trigram costs. The reader maps the file and looks words up with no heap
// allocation.
package lexicon

import "math"

// Magic and Version open every .sslm file.
const (
	Magic   = "SSLM"
	Version = 1
)

const (
	headerSize   = 64
	sectionCount = 10
	tableSize    = sectionCount * 8
	dataStart    = headerSize + tableSize // 144, a multiple of 8
)

// Sections, in file order.
const (
	secWords = iota
	secText
	secNodes
	secBigramRow
	secBigramNext
	secBigramCost
	secTriCtx
	secTriNext
	secTriCost
	secReserved
)

// Word flags.
const (
	// FlagProper: the spelling has a capital that stays mid-sentence ("London", "I").
	FlagProper uint8 = 1 << iota
	// FlagContraction: the spelling has an apostrophe ("don't").
	FlagContraction
	// FlagAllCaps: the spelling is all capitals ("NASA").
	FlagAllCaps
	// FlagNoAutocorrect: the word is offered as a suggestion but never put
	// in place of what was typed (rare words with no corpus evidence).
	FlagNoAutocorrect
	// FlagUnseen: the word came from a word list, not from the corpus.
	FlagUnseen
	// FlagNoSuggest: never shown (the sentence-start token).
	FlagNoSuggest
)

// Default quantization steps: a stored cost byte q means q*CostStep nats,
// a backoff byte b means b*BowStep nats.
const (
	DefaultCostStep = 0.08
	DefaultBowStep  = 0.04
)

// WordID is a word's index in the sorted word list. BOS, one past the last
// word, is the sentence start; NoWord is no word.
type WordID = uint32

// NoWord is the absent word.
const NoWord WordID = math.MaxUint32

// NodeID is a trie node's index; the root is 0.
type NodeID = uint32

// wordRec is one word: its spelling in the text section, flags, and costs.
type wordRec struct {
	TextOff uint32
	TextLen uint8
	Flags   uint8
	Uni     uint8 // unigram cost
	Bow     uint8 // backoff cost of the word as a bigram context
}

// Node is one trie node. Children are contiguous and sorted by Char. The
// words under the node are contiguous from WordLo; the first TermCount of
// them have exactly this node's key, most probable first.
type Node struct {
	FirstChild uint32
	WordLo     uint32
	Char       uint8
	ChildCount uint8
	TermCount  uint8
	BestCost   uint8 // the lowest unigram cost of the words under the node
}

// triCtx is a trigram context: the bigram (w1, w2) by its index in the
// bigram table, and where its row of third words starts.
type triCtx struct {
	Bigram uint32
	RowOff uint32
	Bow    uint8
	_      [3]uint8
}

func quantize(nats, step float64) uint8 {
	q := math.Round(nats / step)
	if q < 0 || math.IsNaN(q) {
		return 0
	}
	if q > 255 {
		return 255
	}
	return uint8(q)
}
