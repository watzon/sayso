package engine

import "testing"

// A capital the user typed on a name, or an apostrophe they typed, keeps
// the word as typed even where the language model prefers another word.
func TestProtectsTypedForm(t *testing.T) {
	cases := []struct{ typed, want string }{
		{"I told Jim ", "I told Jim "},
		{"ask Jim ", "ask Jim "},
		{"I would Mike ", "I would Mike "},
		{"I feel we'll ", "I feel we'll "},
		{"I really we'll ", "I really we'll "},
	}
	for _, c := range cases {
		f := newField(t, "")
		f.e.SetFlags(FlagAutocap | FlagPrivacy)
		f.typeText(c.typed)
		if f.text != c.want {
			t.Errorf("%q wrote %q, want %q", c.typed, f.text, c.want)
		}
	}
}

// The same words typed without the user's mark may still be corrected:
// the protection comes from the mark, not from the word.
func TestUnmarkedFormsStillCorrect(t *testing.T) {
	f := newField(t, "")
	f.e.SetFlags(FlagAutocap | FlagPrivacy)
	f.typeText("I told jim ")
	if f.text == "I told jim " {
		t.Errorf("lower-case jim kept; the protection should need the typed capital")
	}
}

// A shifted first letter checks for a sentence start on the tap path; that
// must not allocate either.
func TestShiftedTapDoesNotAllocate(t *testing.T) {
	f := newField(t, "")
	f.typeText("After the long meeting downtown I told ")
	l := f.e.layout
	x, y := l.Center(l.LetterKey('j'))
	allocs := testing.AllocsPerRun(1000, func() {
		f.e.OnTouch(x, y, true)
		f.e.mu.Lock()
		f.e.resetWord()
		f.e.before = f.e.before[:len("After the long meeting downtown I told ")]
		f.e.mu.Unlock()
	})
	if allocs != 0 {
		t.Fatalf("%v allocations per shifted tap", allocs)
	}
}
