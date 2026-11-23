// Command sslminfo prints a .sslm model's sizes and, for each word given,
// its key-mates with their costs and flags and the likeliest next words.
//
//	cd mobile && go run ./tools/sslminfo don\'t im the
package main

import (
	"cmp"
	"flag"
	"fmt"
	"log"
	"slices"

	"github.com/watzon/sayso/mobile/core/lexicon"
)

func main() {
	path := flag.String("model", "models/en-US.sslm", "model file")
	top := flag.Int("top", 8, "next words to show")
	flag.Parse()
	m, err := lexicon.Open(*path)
	if err != nil {
		log.Fatal(err)
	}
	defer m.Close()
	fmt.Printf("%s: %d bytes, locale %s, %d words, %d nodes (%d B), %d bigrams, %d trigrams\n", *path, m.Size(), m.Locale(),
		m.WordCount(), m.NodeCount(), m.NodeCount()*12, m.BigramCount(), m.TrigramCount())
	for _, arg := range flag.Args() {
		key, ok := lexicon.KeyOf(arg)
		if !ok {
			fmt.Printf("%q: no key\n", arg)
			continue
		}
		lo, n := m.Lookup([]byte(key))
		fmt.Printf("key %q: %d words\n", key, n)
		for i := lo; i < lo+lexicon.WordID(n); i++ {
			fmt.Printf("  %-16s cost %5.2f flags %06b\n", m.Word(i), m.Unigram(i), m.Flags(i))
			c := m.NewContext(lexicon.NoWord, i)
			type s struct {
				w    lexicon.WordID
				cost float32
			}
			var next []s
			m.Successors(&c, func(w lexicon.WordID, cost float32) bool { next = append(next, s{w, cost}); return true })
			slices.SortFunc(next, func(a, b s) int { return cmp.Compare(a.cost, b.cost) })
			fmt.Print("    next:")
			for j := 0; j < len(next) && j < *top; j++ {
				fmt.Printf(" %s(%.1f)", m.Word(next[j].w), next[j].cost)
			}
			fmt.Println()
		}
	}
	c := m.NewContext(lexicon.NoWord, m.BOS())
	var starts []string
	m.Successors(&c, func(w lexicon.WordID, cost float32) bool {
		if cost < 3.5 {
			starts = append(starts, fmt.Sprintf("%s(%.1f)", m.Word(w), cost))
		}
		return true
	})
	fmt.Println("sentence starts under 3.5 nats:", starts)
}
