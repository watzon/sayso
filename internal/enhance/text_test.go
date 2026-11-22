package enhance

import (
	"reflect"
	"testing"

	"github.com/watzon/sayso/internal/store"
)

func rule(id int64, from, to string) store.Replacement {
	return store.Replacement{ID: id, From: from, To: to}
}

func TestReplacer(t *testing.T) {
	out, applied := NewReplacer([]store.Replacement{rule(1, "git hub", "GitHub")}).Apply("open a Git  Hub issue on git hub")
	if out != "open a GitHub issue on GitHub" || applied[0].Count != 2 {
		t.Fatal(out, applied)
	}
	if out, _ := NewReplacer([]store.Replacement{rule(1, "cat", "dog")}).Apply("concatenate the cat"); out != "concatenate the dog" {
		t.Fatal(out)
	}
	out, _ = NewReplacer([]store.Replacement{rule(1, "new", "NEW"), rule(2, "new paragraph", `\n\n`)}).Apply("first part new paragraph second part is new")
	if out != "first part\n\nsecond part is NEW" {
		t.Fatalf("%q", out)
	}
	cs := rule(1, "Go", "Golang")
	cs.CaseSensitive = true
	if out, _ := NewReplacer([]store.Replacement{cs}).Apply("go learn Go"); out != "go learn Golang" {
		t.Fatal(out)
	}
	if out, _ := NewReplacer([]store.Replacement{rule(1, "my email", "chris+$1@example.com")}).Apply("send it to my email"); out != "send it to chris+$1@example.com" {
		t.Fatal(out)
	}
	if out, _ := NewReplacer([]store.Replacement{rule(1, "  ", "x")}).Apply("hello"); out != "hello" {
		t.Fatal(out)
	}
	// Unicode word boundaries, as Rust's \b: "café" is one word.
	if out, _ := NewReplacer([]store.Replacement{rule(1, "caf", "X")}).Apply("café caf"); out != "café X" {
		t.Fatal(out)
	}
	// A phrase that ends in punctuation has no boundary on that side.
	if out, _ := NewReplacer([]store.Replacement{rule(1, "c++", "C++")}).Apply("I like c++."); out != "I like C++." {
		t.Fatal(out)
	}
}

func heard(transcript string) []string {
	return WordsHeard([]string{"Claude Code", "Devhouse", "Fenneko", "ForgeCAD", "MayFirmOS", "Pindrop", "Sayso"}, transcript)
}

func TestWordsHeard(t *testing.T) {
	cases := map[string][]string{
		"Commit everything you have locally, get it pushed up, and then let's create a new release.": {},
		"":                                         {},
		"open pin drop, then say so and forge cad": {"ForgeCAD", "Pindrop", "Sayso"},
		"I asked claude code about it":             {"Claude Code"},
		"the may firm o s build":                   {"MayFirmOS"},
		"Sayso.":                                   {"Sayso"},
		"ask fennec o for the file":                {"Fenneko"},
		"the dev houses repo":                      {"Devhouse"},
	}
	for in, want := range cases {
		if got := heard(in); !reflect.DeepEqual(got, want) {
			t.Errorf("%q: %v", in, got)
		}
	}
	d := func(a, b string) int { return editDistance([]rune(a), []rune(b)) }
	if d("sayso", "sayso") != 0 || d("fennec", "fenneko") != 2 || d("", "abc") != 3 {
		t.Fatal("edit distance")
	}
}

func TestSpokenPunctuation(t *testing.T) {
	cases := map[string]string{
		"hello comma how are you question mark":                       "hello, how are you?",
		"wait semicolon no colon yes exclamation point":               "wait; no: yes!",
		"it works period ship it full stop":                           "it works. Ship it.",
		"first item new line second item new paragraph the end":       "first item\nSecond item\n\nThe end",
		"one comma two":                                               "one, two",
		"Hello, comma, world. Period.":                                "Hello, world.",
		"Is it done? New line. Yes.":                                  "Is it done?\nYes.",
		"the periodic table and a comma-free line":                    "the periodic table and a, -free line",
		"Done PERIOD":                                                 "Done.",
		"":                                                            "",
		"Send it to Dana on Friday.":                                  "Send it to Dana on Friday.",
		"a  b\nc":                                                     "a  b\nc",
		"périod commaé":                                               "périod commaé",
	}
	for in, want := range cases {
		if got := SpokenPunctuation(in); got != want {
			t.Errorf("%q: %q", in, got)
		}
	}
}

func TestSpelled(t *testing.T) {
	cases := map[string]string{
		"send it to dana cats that's k a t z":                           "send it to dana katz",
		"Ask Shawn, that's S-E-A-N, about it.":                          "Ask Sean, about it.",
		"Her name is Katz, spelled K, A, T, Z.":                         "Her name is Katz.",
		"run cube control spelled k u b e c t l to see the pods":       "run kubectl to see the pods",
		"open engine x spelt n g i n x and reload it":                  "open nginx and reload it",
		"the name is spelled k a t z":                                   "the name is katz",
		"spelled k a t z":                                               "katz",
		"It is done. That is K A T Z speaking.":                         "It is done. katz speaking.",
		"the grade that's a b":                                          "the grade that's a b",
		"the unit is spelled k g":                                       "the unit is kg",
		"":                                                              "",
		"that's a good idea":                                            "that's a good idea",
		"I spelled it wrong and that is all":                            "I spelled it wrong and that is all",
		"plan a b c is ready":                                           "plan a b c is ready",
		"Call me at 5 p m.":                                             "Call me at 5 p m.",
		"ask shawn that's s e a n and john that's j o n":                "ask sean and jon",
	}
	for in, want := range cases {
		if got := Spelled(in); got != want {
			t.Errorf("%q: %q", in, got)
		}
	}
}
