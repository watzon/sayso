// Command sslmgen builds Sayso's .sslm typing model from open data: SCOWL
// word lists and Tatoeba sentences (DATA_SOURCES.md). It downloads the
// inputs into a cache directory, checks their hashes, and writes the same
// model for the same inputs.
//
//	cd mobile && go run ./tools/sslmgen
package main

import (
	"bufio"
	"bytes"
	_ "embed"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"strings"

	"github.com/watzon/sayso/mobile/core/lexicon"
)

//go:embed extra-words.txt
var extraWords string

// pinnedMaxID is the largest sentence ID of the pinned Tatoeba snapshot.
const pinnedMaxID = 14127918

func main() {
	userCache, _ := os.UserCacheDir()
	cache := flag.String("cache", filepath.Join(userCache, "sayso-sslmgen"), "download cache directory")
	out := flag.String("out", "models/en-US.sslm", "model to write")
	heldOut := flag.String("heldout", "core/eval/testdata/heldout-en.txt", "held-out sentences to write for the replay benchmark (empty: none)")
	heldOutN := flag.Int("heldout-n", 3000, "how many held-out sentences to write")
	maxID := flag.Int("tatoeba-max-id", pinnedMaxID, "skip Tatoeba sentences with a larger ID (0: none)")
	strict := flag.Bool("strict", false, "fail when a download does not match its pinned hash")
	maxWords := flag.Int("max-words", 100000, "vocabulary size")
	maxBigrams := flag.Int("max-bigrams", 380000, "bigrams to keep")
	maxTrigrams := flag.Int("max-trigrams", 200000, "trigrams to keep")
	flag.Parse()
	log.SetFlags(0)

	if err := os.MkdirAll(*cache, 0o755); err != nil {
		log.Fatal(err)
	}
	scowlPath, err := fetch(*cache, scowlSource, *strict)
	if err != nil {
		log.Fatal(err)
	}
	tatoebaPath, err := fetch(*cache, tatoebaSource, *strict)
	if err != nil {
		log.Fatal(err)
	}
	scowl, err := readSCOWL(scowlPath, 95)
	if err != nil {
		log.Fatal(err)
	}
	c, err := readTatoeba(tatoebaPath, *maxID)
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("tatoeba: %d training sentences, %d tokens, %d held out, %d duplicates of held-out dropped, max id %d",
		len(c.sentences), c.tokens, len(c.heldOut), c.droppedDuplicates, c.maxID)

	var extra []string
	for _, l := range strings.Split(extraWords, "\n") {
		if l = strings.TrimSpace(l); l != "" && !strings.HasPrefix(l, "#") {
			extra = append(extra, l)
		}
	}
	vocab := chooseVocab(c, scowl, extra, vocabOptions{maxLevel: 60, extendedLevel: 70, corpusOnlyMin: 25, maxWords: *maxWords, properRank: 300})
	ng := estimate(c, vocab, ngramOptions{minBigram: 2, minTrigram: 3, maxBigrams: *maxBigrams, maxTrigrams: *maxTrigrams, pruneNats: 0.3})

	b := &lexicon.Builder{Locale: "en-US", Bigrams: ng.bigrams, Trigrams: ng.trigrams, TriBow: ng.triBow, CorpusTokens: uint32(c.tokens)}
	for i, w := range vocab {
		b.Entries = append(b.Entries, lexicon.Entry{Display: w.display, Flags: w.flags, Cost: -ln(w.p), Bow: -ln(ng.bow2[i])})
	}
	b.BOSBow = -ln(ng.bow2[len(vocab)])
	var buf bytes.Buffer
	if err := b.Write(&buf); err != nil {
		log.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(*out), 0o755); err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, buf.Bytes(), 0o644); err != nil {
		log.Fatal(err)
	}
	m, err := lexicon.FromBytes(buf.Bytes())
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("wrote %s: %d bytes, %d words, %d trie nodes, %d bigrams, %d trigrams",
		*out, m.Size(), m.WordCount(), m.NodeCount(), m.BigramCount(), m.TrigramCount())

	if *heldOut != "" {
		if err := writeHeldOut(*heldOut, c.heldOut, *heldOutN); err != nil {
			log.Fatal(err)
		}
	}
}

func writeHeldOut(path string, sentences []string, n int) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	w := bufio.NewWriter(f)
	fmt.Fprintf(w, "# Held-out Tatoeba sentences (IDs divisible by %d), not used to build the model.\n", heldOutEvery)
	fmt.Fprintln(w, "# Tatoeba (https://tatoeba.org), CC BY 2.0 FR. See mobile/tools/sslmgen/DATA_SOURCES.md.")
	for i, s := range sentences {
		if i == n {
			break
		}
		fmt.Fprintln(w, strings.ReplaceAll(s, "\n", " "))
	}
	if err := w.Flush(); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}
