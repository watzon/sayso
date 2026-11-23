package voice

import (
	"strings"
	"testing"

	"github.com/watzon/sayso/mobile/core/textrules"
)

// spellings stands in for the model: the most likely spelling of a word.
var spellings = map[string]string{
	"the": "the", "very": "very", "world": "world", "i": "I", "paris": "Paris",
	"a": "a", "hello": "hello", "it's": "it's", "nasa": "NASA",
}

func testDict() Dictionary {
	return Dictionary{
		Words:        []string{"GitHub", "Sayso", "K8s"},
		Replacements: []textrules.Rule{{From: "omw", To: "on my way"}},
		Spelling: func(w string) (string, bool) {
			s, ok := spellings[strings.ToLower(w)]
			return s, ok
		},
		Autocap: true,
	}
}

func TestPostProcess(t *testing.T) {
	cases := []struct{ raw, before, after, want string }{
		{"", "x", "", ""},
		{"   ", "", "", ""},
		// Case from the cursor.
		{"hello there", "", "", "Hello there"},
		{"hello there", "Done.\n", "", "Hello there"},
		{"The cat sat.", "I said ", "", "the cat sat."},
		{"World.", "Hello", "", " world."},
		{"I think so.", "and ", "", "I think so."},
		{"Paris is nice.", "we like how ", "", "Paris is nice."},
		{"NASA called.", "then ", "", "NASA called."},
		{"It's late.", "well ", "", "it's late."},
		// Spaces and the recognizer's period, mid-sentence.
		{"Very.", "This is", " good", " very"},
		{"Very.", "This is ", "good", "very "},
		{"Very.", "This is ", ", right", "very"},
		{"aside", "(", ")", "aside"},
		{"comma and then", "first", "", ", and then"},
		// Spoken punctuation.
		{"hello comma world period new line next", "", "", "Hello, world.\nNext"},
		// Replacements and personal words.
		{"omw now", "", "", "On my way now"},
		{"push it to git hub please", "I'll ", "", "push it to GitHub please"},
		{"push it to git-hub", "", "", "Push it to GitHub"},
		{"I really say so", "", "", "I really say so"},
		{"sayso works", "", "", "Sayso works"},
		{"deploy to k 8 s", "", "", "Deploy to K8s"},
	}
	for _, c := range cases {
		if got := PostProcess(c.raw, c.before, c.after, testDict()); got != c.want {
			t.Errorf("PostProcess(%q, before %q, after %q) = %q, want %q", c.raw, c.before, c.after, got, c.want)
		}
	}
}

func TestPostProcessWithoutAutocapOrModel(t *testing.T) {
	d := Dictionary{}
	if got := PostProcess("hello", "", "", d); got != "hello" {
		t.Errorf("no autocap: %q", got)
	}
	if got := PostProcess("The end", "see ", "", d); got != "The end" {
		t.Errorf("no model, the recognizer's case stays: %q", got)
	}
}
