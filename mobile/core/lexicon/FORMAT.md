# The .sslm format, version 1

An `.sslm` file is a Sayso typing language model: a word list with a key trie,
quantized unigram costs, a quantized bigram table, and a pruned quantized
trigram table. The keyboard maps the file read-only and looks words up with no
heap allocation (`lexicon.Model`). `tools/sslmgen` writes it.

All integers are little-endian. Every section starts at a multiple of 8 bytes
from the start of the file, so a page-aligned mapping can be read as arrays of
the records below.

## Costs

A cost is `-ln p` in nats. A cost byte `q` means `q × costStep` nats; a backoff
byte `b` means `b × bowStep` nats. The steps are in the header (default 0.08 and
0.04), so a byte covers 0 to 20.4 nats of cost and 0 to 10.2 nats of backoff.

## Keys

A word's key is the letters it is typed with: lower-case `a`–`z`, accents folded
(`café` → `cafe`), apostrophes dropped (`don't` → `dont`). Keys are at most 32
letters. The trie is built over keys; the spelling is stored apart, so
`don't` and a typed `dont` meet at one trie node, and key-mates such as `its`
and `it's` sit side by side.

## Layout

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `SSLM` |
| 4 | 2 | version, 1 |
| 6 | 2 | flags, 0 |
| 8 | 4 | `N`, the number of words (the sentence start is not counted) |
| 12 | 4 | trie node count |
| 16 | 4 | bigram count |
| 20 | 4 | trigram context count (the sentinel is not counted) |
| 24 | 4 | trigram count |
| 28 | 4 | section count, 10 |
| 32 | 8 | locale tag, NUL-padded ASCII (`en-US`) |
| 40 | 4 | `costStep`, float32 |
| 44 | 4 | `bowStep`, float32 |
| 48 | 4 | CRC-32C (Castagnoli) of bytes 64 to the end of the file |
| 52 | 4 | total file length |
| 56 | 4 | corpus tokens (information only) |
| 60 | 4 | reserved, 0 |
| 64 | 80 | section table: 10 × (offset u32, length u32) |

### Sections

| # | Name | Record | Count |
|---|---|---|---|
| 0 | words | 8 bytes: text offset u32, text length u8, flags u8, unigram cost u8, bigram backoff u8 | `N + 1` |
| 1 | text | the UTF-8 spellings, back to back | bytes |
| 2 | nodes | 12 bytes: first child u32, first word u32, letter u8, child count u8, term count u8, best cost u8 | nodes |
| 3 | bigram rows | u32 start of each word's row | `N + 2` |
| 4 | bigram next | u32 second word | bigrams |
| 5 | bigram cost | u8 cost of `p(w2 | w1)` | bigrams |
| 6 | trigram contexts | 12 bytes: bigram index u32, row start u32, backoff u8, 3 bytes padding | contexts + 1 |
| 7 | trigram next | u32 third word | trigrams |
| 8 | trigram cost | u8 cost of `p(w3 | w1 w2)` | trigrams |
| 9 | reserved | empty | 0 |

**Words.** A word ID is its index in section 0. Words are sorted by key, then
by unigram cost (most probable first), then by spelling. Record `N` is the
sentence start, spelled `<s>`; it is never typed or suggested, and appears only
as a context. Flags:

| Bit | Name | Meaning |
|---|---|---|
| 0 | proper | the spelling keeps a capital mid-sentence (`London`, `I`) |
| 1 | contraction | the spelling has an apostrophe |
| 2 | all caps | the spelling is all capitals (`NASA`) |
| 3 | no autocorrect | suggest it, but never put it in place of what was typed |
| 4 | unseen | from the word list only; the corpus never had it |
| 5 | no suggest | never shown (the sentence start) |

**Trie.** Node 0 is the root (the empty key). Nodes are in breadth-first order,
so the children of a node are the `child count` nodes from `first child`,
sorted by letter. The words whose keys start with a node's key are contiguous
from `first word`; the first `term count` of them have exactly the node's key.
`best cost` is the lowest unigram cost of any word under the node; the decoder
uses it as an admissible look-ahead, and following children with the same best
cost finds the most probable completion.

**Bigrams.** The row of word `w1` is entries `rows[w1]` to `rows[w1+1]` of
sections 4 and 5, sorted by second word. A bigram's index is its position in
those sections.

**Trigrams.** A context `(w1, w2)` is named by the index of the bigram
`(w1, w2)`, which must exist. Contexts are sorted by that index; context `i`
owns trigram entries `ctx[i].rowStart` to `ctx[i+1].rowStart`, sorted by third
word. A final sentinel context (bigram index `0xFFFFFFFF`) closes the last row.

### Scoring (backoff, ARPA semantics)

```
cost(w | w1 w2) = tri(w1 w2 w)                       if stored
                = bow3(w1 w2) + bi(w2 w)             else, if stored
                = bow3(w1 w2) + bow2(w2) + uni(w)    otherwise
```

`bow3` is 0 when `(w1, w2)` has no trigram row. Stored probabilities are
interpolated absolute-discounting estimates; the backoff weights make each
context's distribution sum to one over the stored entries plus the backed-off
rest.

## Validation

A reader rejects a file whose magic, version, section count, length, or
checksum is wrong, whose sections overlap the header, are misaligned, or have
a size that does not match their count, or whose indexes point outside their
tables. After those checks, no lookup can go out of bounds.

## Compatibility

A change to any record, section, or meaning bumps the version. Readers refuse
other versions; the app ships the model it was built with.
