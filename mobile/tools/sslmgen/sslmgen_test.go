package main

import (
	"bytes"
	"math"
	"reflect"
	"testing"

	"github.com/watzon/sayso/mobile/core/lexicon"
)

func TestTokenize(t *testing.T) {
	got := tokenize("Tom's here. Don’t pay $5, OK? 3rd 'quoted' e-mail")
	want := []string{"Tom's", "here", ".", "Don't", "pay", "#", "OK", ".", "#", "quoted", "e", "mail"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("%q", got)
	}
}

// The estimates must form distributions: for a context, the probabilities
// of all words, through the written model's backoff, sum to about one.
func TestEstimatesNormalize(t *testing.T) {
	c := &corpus{index: map[string]int32{}}
	for range 3 {
		for _, s := range []string{"I like the cat.", "I like the dog.", "The cat sat on the mat.", "I saw the cat sat.", "The dog sat."} {
			c.add(s)
		}
	}
	scowl := scowlWords{"I": 10, "like": 10, "the": 10, "cat": 10, "dog": 10, "sat": 10, "on": 10, "mat": 20, "saw": 10, "zebra": 50}
	vocab := chooseVocab(c, scowl, nil, vocabOptions{maxLevel: 60, extendedLevel: 70, corpusOnlyMin: 25, maxWords: 100, properRank: 3})
	ng := estimate(c, vocab, ngramOptions{minBigram: 1, minTrigram: 1, maxBigrams: 100, maxTrigrams: 100})
	b := &lexicon.Builder{Locale: "en-US", Bigrams: ng.bigrams, Trigrams: ng.trigrams, TriBow: ng.triBow, CostStep: 0.03, BowStep: 0.01}
	for i, w := range vocab {
		b.Entries = append(b.Entries, lexicon.Entry{Display: w.display, Flags: w.flags, Cost: -ln(w.p), Bow: -ln(ng.bow2[i])})
	}
	b.BOSBow = -ln(ng.bow2[len(vocab)])
	var buf bytes.Buffer
	if err := b.Write(&buf); err != nil {
		t.Fatal(err)
	}
	m, err := lexicon.FromBytes(buf.Bytes())
	if err != nil {
		t.Fatal(err)
	}
	id := func(w string) lexicon.WordID {
		i, ok := m.Find([]byte(w), false)
		if !ok {
			t.Fatalf("%q missing", w)
		}
		return i
	}
	if _, ok := m.Find([]byte("I"), false); !ok {
		t.Fatal(`"I" should keep its capital`)
	}
	if m.Flags(id("zebra"))&lexicon.FlagUnseen == 0 {
		t.Fatal("zebra is unseen")
	}
	for _, ctx := range [][2]lexicon.WordID{{lexicon.NoWord, lexicon.NoWord}, {lexicon.NoWord, id("the")}, {id("like"), id("the")}, {m.BOS(), id("I")}} {
		c := m.NewContext(ctx[0], ctx[1])
		var sum float64
		for w := range lexicon.WordID(m.WordCount()) {
			sum += math.Exp(-float64(m.Cost(&c, w)))
		}
		if math.Abs(sum-1) > 0.02 {
			t.Errorf("context %v sums to %.4f", ctx, sum)
		}
	}
	c2 := m.NewContext(id("like"), id("the"))
	if m.Cost(&c2, id("cat")) >= m.Cost(&c2, id("mat")) {
		t.Error("\"like the cat\" should beat \"like the mat\"")
	}
}
