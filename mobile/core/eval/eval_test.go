package eval

import (
	"testing"

	"github.com/watzon/sayso/mobile/core/lexicon"
)

// TestReplayGate is the quality gate: it types the held-out sentences in
// every mode and fails when false autocorrections pass 0.2% or when
// hit-target expansion and learning stop paying for themselves.
func TestReplayGate(t *testing.T) {
	m, err := lexicon.Open("../../models/en-US.sslm")
	if err != nil {
		t.Fatal(err)
	}
	defer m.Close()
	n := 0 // all 3,000
	if testing.Short() {
		n = 600
	}
	sentences, err := LoadSentences("testdata/heldout-en.txt", n)
	if err != nil {
		t.Fatal(err)
	}
	results, table, err := Report(m, sentences, DefaultUser)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("\n%s", table)
	raw, base, exp, learn := results[0], results[1], results[2], results[3]
	for _, r := range results[1:] {
		if r.FalseRate() >= 0.002 {
			t.Errorf("%s: false autocorrect rate %.3f%% (limit 0.2%%): %v", r.Mode, 100*r.FalseRate(), r.FalseExamples)
		}
	}
	if base.WER() >= raw.WER()/3 {
		t.Errorf("the decoder should cut raw-tap errors by two thirds: %.2f%% vs %.2f%%", 100*base.WER(), 100*raw.WER())
	}
	if exp.WER() > base.WER()*0.95 {
		t.Errorf("expansion should cut errors by 5%% or more: %.2f%% vs %.2f%%", 100*exp.WER(), 100*base.WER())
	}
	if learn.WER() > exp.WER()*0.95 {
		t.Errorf("learning should cut errors by 5%% or more: %.2f%% vs %.2f%%", 100*learn.WER(), 100*exp.WER())
	}
	if base.RecoveryRate() < 0.6 {
		t.Errorf("typo recovery %.1f%%, want 60%% or more", 100*base.RecoveryRate())
	}
}

func TestWordsAndSlips(t *testing.T) {
	if got := words("Don’t pay $5, Tom."); got != nil {
		t.Fatal("a sentence with a number is skipped", got)
	}
	got := words("“Don’t go,” said Tom — twice.")
	want := []string{"Don't", "go", "said", "Tom", "twice"}
	if len(got) != len(want) {
		t.Fatal(got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatal(got)
		}
	}
}

// TestSloppyTypistReplays: a stray tap on the delete key can eat back past
// the start of the word being typed. That used to panic slicing the text.
func TestSloppyTypistReplays(t *testing.T) {
	m, err := lexicon.Open("../../models/en-US.sslm")
	if err != nil {
		t.Fatal(err)
	}
	defer m.Close()
	sentences, err := LoadSentences("testdata/heldout-en.txt", 0)
	if err != nil {
		t.Fatal(err)
	}
	u := DefaultUser
	u.NoiseX, u.NoiseY, u.TypoRate = 0.32, 0.30, 0.06
	r, err := Run(m, sentences, u, Raw)
	if err != nil {
		t.Fatal(err)
	}
	if r.Words == 0 {
		t.Fatal("typed nothing")
	}
}

// TestProtectedFormsGate types names with a capital mid-sentence and
// contractions with their apostrophe, after words that make a common word
// more likely ("I told Jim", "I feel we'll"). The typist aims right, so
// every change the engine makes to such a word is a false correction.
// Before the decoder protected these forms, 6% to 8% of them were changed
// ("Jim → Him", "Mike → Like", "we'll → well").
func TestProtectedFormsGate(t *testing.T) {
	m, err := lexicon.Open("../../models/en-US.sslm")
	if err != nil {
		t.Fatal(err)
	}
	defer m.Close()
	sentences, err := LoadSentences("testdata/protect-en.txt", 0)
	if err != nil {
		t.Fatal(err)
	}
	u := DefaultUser
	u.SkipApostrophe, u.TypoRate, u.SubRate = 0, 0, 0
	results, table, err := Report(m, sentences, u)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("\n%s", table)
	for _, r := range results[1:] {
		if r.FalseRate() >= 0.01 {
			t.Errorf("%s: false autocorrect rate %.2f%% on protected forms (limit 1%%): %v", r.Mode, 100*r.FalseRate(), r.FalseExamples)
		}
	}
}
