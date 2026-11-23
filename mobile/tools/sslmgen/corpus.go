package main

import (
	"bufio"
	"compress/bzip2"
	"os"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"
)

// form is one lower-case spelling seen in the corpus and how it was written.
type form struct {
	count     int            // all occurrences
	mid       int            // occurrences not at a sentence start
	midCap    int            // of those, with a capital first letter
	midAll    int            // of those, all capitals (two or more letters)
	spellings map[string]int // exact spellings, mid-sentence
}

// corpus is the training text as form ids, sentence by sentence; -1 is a
// token that is not a word (a number, a symbol).
type corpus struct {
	forms     []*form
	lower     []string
	index     map[string]int32
	sentences [][]int32
	tokens    int
	heldOut   []string // held-out sentences, in ID order
	maxID     int
	// droppedDuplicates counts training sentences equal to a held-out one.
	droppedDuplicates int
}

// heldOutEvery: sentences whose Tatoeba ID is a multiple of it are kept out
// of the model for the replay benchmark.
const heldOutEvery = 50

// readTatoeba reads the bz2 TSV export (id, lang, text). Sentences above
// maxID are skipped.
func readTatoeba(path string, maxID int) (*corpus, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	c := &corpus{index: map[string]int32{}}
	var train []string
	sc := bufio.NewScanner(bzip2.NewReader(f))
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		parts := strings.SplitN(sc.Text(), "\t", 3)
		if len(parts) != 3 {
			continue
		}
		id, err := strconv.Atoi(parts[0])
		if err != nil || (maxID > 0 && id > maxID) {
			continue
		}
		c.maxID = max(c.maxID, id)
		if id%heldOutEvery == 0 {
			c.heldOut = append(c.heldOut, parts[2])
			continue
		}
		train = append(train, parts[2])
	}
	if err := sc.Err(); err != nil {
		return nil, err
	}
	// A training sentence with the same text as a held-out one is dropped
	// too, so the replay benchmark never types text the model has seen.
	held := make(map[string]bool, len(c.heldOut))
	for _, s := range c.heldOut {
		held[normalize(s)] = true
	}
	for _, s := range train {
		if !held[normalize(s)] {
			c.add(s)
		} else {
			c.droppedDuplicates++
		}
	}
	return c, nil
}

func normalize(s string) string { return strings.ToLower(strings.Join(strings.Fields(s), " ")) }

func (c *corpus) intern(lower string) int32 {
	if id, ok := c.index[lower]; ok {
		return id
	}
	id := int32(len(c.forms))
	c.index[lower] = id
	c.forms = append(c.forms, &form{spellings: map[string]int{}})
	c.lower = append(c.lower, lower)
	return id
}

// add splits text into sentences and words. A sentence ends at . ! ? or a
// line break; other punctuation is passed over, so "Well, I" is a bigram.
func (c *corpus) add(text string) {
	var sent []int32
	flush := func() {
		if len(sent) > 0 {
			c.sentences = append(c.sentences, sent)
			sent = nil
		}
	}
	for _, tok := range tokenize(text) {
		switch {
		case tok == ".":
			flush()
		case tok == "#":
			sent = append(sent, -1)
		default:
			lower := strings.ToLower(tok)
			id := c.intern(lower)
			f := c.forms[id]
			f.count++
			c.tokens++
			if len(sent) > 0 {
				f.mid++
				r, _ := utf8.DecodeRuneInString(tok)
				if unicode.IsUpper(r) {
					f.midCap++
					if utf8.RuneCountInString(tok) >= 2 && tok == strings.ToUpper(tok) {
						f.midAll++
					}
				}
				if len(f.spellings) < 8 || f.spellings[tok] > 0 {
					f.spellings[tok]++
				}
			}
			sent = append(sent, id)
		}
	}
	flush()
}

// tokenize returns words (letters with inner apostrophes, ’ written as '),
// "." for a sentence end, and "#" for a token that is not a word.
func tokenize(text string) []string {
	var out []string
	rs := []rune(text)
	for i := 0; i < len(rs); {
		r := rs[i]
		switch {
		case unicode.IsLetter(r):
			j := i
			var sb strings.Builder
			for j < len(rs) {
				if unicode.IsLetter(rs[j]) || unicode.IsMark(rs[j]) {
					sb.WriteRune(rs[j])
					j++
					continue
				}
				if (rs[j] == '\'' || rs[j] == '’') && j+1 < len(rs) && unicode.IsLetter(rs[j+1]) {
					sb.WriteByte('\'')
					j++
					continue
				}
				break
			}
			// A word glued to a digit or another symbol ("3rd", "a_b") is not a word.
			if j < len(rs) && (unicode.IsDigit(rs[j]) || rs[j] == '_' || rs[j] == '@') {
				for j < len(rs) && !unicode.IsSpace(rs[j]) {
					j++
				}
				out = append(out, "#")
			} else {
				out = append(out, sb.String())
			}
			i = j
		case r == '.' || r == '!' || r == '?' || r == '\n' || r == '…':
			out = append(out, ".")
			i++
		case unicode.IsDigit(r) || r == '@' || r == '#' || r == '$' || r == '%' || r == '&' || r == '/':
			for i < len(rs) && !unicode.IsSpace(rs[i]) && !strings.ContainsRune(",;:!?\"", rs[i]) {
				i++
			}
			out = append(out, "#")
		default:
			i++
		}
	}
	return out
}
