// Command replay runs the typing replay benchmark over the held-out
// sentences and prints a table.
//
//	cd mobile && go run ./core/eval/cmd/replay
package main

import (
	"flag"
	"fmt"
	"log"

	"github.com/watzon/sayso/mobile/core/eval"
	"github.com/watzon/sayso/mobile/core/lexicon"
)

func main() {
	modelPath := flag.String("model", "models/en-US.sslm", "model file")
	text := flag.String("text", "core/eval/testdata/heldout-en.txt", "held-out sentences")
	n := flag.Int("n", 0, "sentences to type (0: all)")
	examples := flag.Bool("examples", false, "print false corrections")
	u := eval.DefaultUser
	flag.Uint64Var(&u.Seed, "seed", u.Seed, "typist seed")
	noise := flag.Float64("noise", float64(u.NoiseX), "tap spread in key pitches (y is 0.02 less)")
	flag.Float64Var(&u.TypoRate, "typos", u.TypoRate, "chance per word of a missed, doubled, or swapped letter")
	flag.Float64Var(&u.SubRate, "subs", u.SubRate, "chance per word of a letter aimed at the wrong key")
	flag.Parse()
	u.NoiseX, u.NoiseY = float32(*noise), float32(*noise)-0.02
	m, err := lexicon.Open(*modelPath)
	if err != nil {
		log.Fatal(err)
	}
	sentences, err := eval.LoadSentences(*text, *n)
	if err != nil {
		log.Fatal(err)
	}
	results, table, err := eval.Report(m, sentences, u)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Print(table)
	if *examples {
		for _, r := range results {
			fmt.Println(r.Mode, r.FalseExamples)
		}
	}
}
