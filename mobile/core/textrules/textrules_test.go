package textrules

import "testing"

// The full replacement, spoken punctuation, and spelling cases run through
// the desktop wrappers in internal/enhance/text_test.go. These check the
// package's own API and the keyboard helpers.

func TestReplacerDirect(t *testing.T) {
	out, applied := NewReplacer([]Rule{{ID: 7, From: "git hub", To: "GitHub"}}).Apply("on git  hub")
	if out != "on GitHub" || len(applied) != 1 || applied[0].RuleID != 7 || applied[0].Count != 1 {
		t.Fatal(out, applied)
	}
	if _, applied := NewReplacer(nil).Apply("x"); applied == nil || len(applied) != 0 {
		t.Fatal("want an empty, non-nil trail")
	}
}

func TestSentenceStart(t *testing.T) {
	cases := map[string]bool{
		"":               true,
		"   ":            true,
		"Hello.":         true,
		"Hello. ":        true,
		"Really?\" ":     true,
		"first line\n":   true,
		"hello ":         false,
		"Hello, ":        false,
		"e.g":            false,
		"wait… ":         true,
		"(see this) ":    false,
		"It works!)  ":   true,
		"don't ":         false,
		"over there.\t ": true,
	}
	for in, want := range cases {
		if got := SentenceStart(in); got != want {
			t.Errorf("%q: %v", in, got)
		}
	}
}

func TestMatchCase(t *testing.T) {
	cases := [][3]string{
		{"helo", "hello", "hello"},
		{"Helo", "hello", "Hello"},
		{"HELO", "hello", "HELLO"},
		{"london", "London", "London"},
		{"i", "I", "I"},
		{"im", "I'm", "I'm"},
		{"Dont", "don't", "Don't"},
		{"A", "a", "A"},
	}
	for _, c := range cases {
		if got := MatchCase(c[0], c[1]); got != c[2] {
			t.Errorf("MatchCase(%q, %q) = %q, want %q", c[0], c[1], got, c[2])
		}
	}
	if CapitalizeFirst("élan") != "Élan" || CapitalizeFirst("") != "" {
		t.Fatal("CapitalizeFirst")
	}
	if !AttachesLeft(',') || AttachesLeft('(') || !EndsSentence('?') || EndsSentence(',') {
		t.Fatal("punctuation helpers")
	}
}
