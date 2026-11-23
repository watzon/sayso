package engine

import (
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/decoder"
	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/touch"
)

var (
	modelOnce sync.Once
	model     *lexicon.Model
)

func loadModel(t testing.TB) *lexicon.Model {
	t.Helper()
	modelOnce.Do(func() {
		var err error
		if model, err = lexicon.Open("../../models/en-US.sslm"); err != nil {
			panic(err)
		}
	})
	return model
}

// field is a text field: it applies the engine's actions like an app.
type field struct {
	t    testing.TB
	e    *Engine
	text string
}

func newField(t testing.TB, journal string) *field {
	t.Helper()
	e, err := New(loadModel(t), journal)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { e.Close() })
	return &field{t: t, e: e}
}

func (f *field) apply(a Action, fallback string) {
	switch a.Kind {
	case ActionNone:
		f.text += fallback
	case ActionReplace:
		r := []rune(f.text)
		if a.DeleteChars > len(r) {
			f.t.Fatalf("delete %d of %q", a.DeleteChars, f.text)
		}
		f.text = string(r[:len(r)-a.DeleteChars]) + a.Text
	}
}

// tap types a character by tapping the centre of its key.
func (f *field) tap(c rune) {
	l := f.e.layout
	var k int
	switch {
	case c == ' ':
		k = l.Space()
	case c >= 'a' && c <= 'z', c >= 'A' && c <= 'Z':
		k = l.LetterKey(byte(c | 0x20))
	default:
		f.apply(f.e.TypeChar(c), string(c))
		return
	}
	x, y := l.Center(k)
	r := f.e.OnTouch(x, y, c >= 'A' && c <= 'Z')
	if r.Code == ' ' {
		f.apply(f.e.TypeChar(' '), " ")
		return
	}
	f.text += string(r.Code)
}

func (f *field) typeText(s string) {
	for _, c := range s {
		f.tap(c)
	}
}

func (f *field) backspace() {
	a := f.e.Backspace()
	if a.Kind == ActionNone {
		_, size := utf8.DecodeLastRuneInString(f.text)
		f.text = f.text[:len(f.text)-size]
		return
	}
	f.apply(a, "")
}

func (f *field) check(want string) {
	f.t.Helper()
	if f.text != want {
		f.t.Fatalf("text %q, want %q", f.text, want)
	}
	if got := f.e.Before(); got != f.text {
		f.t.Fatalf("engine mirror %q, field %q", got, f.text)
	}
}

func TestAutocorrectAndRevert(t *testing.T) {
	f := newField(t, "")
	f.typeText("I dont ")
	f.check("I don't ")
	f.backspace() // undoes the correction
	f.check("I dont")
	f.typeText(" ")
	f.check("I dont ") // kept as typed this time
	f.typeText("know ")
	f.check("I dont know ")
}

func TestRevertsSuppressAfterTwo(t *testing.T) {
	f := newField(t, "")
	for range 2 {
		f.typeText("dont ")
		f.backspace()
		f.typeText(" ")
	}
	f.typeText("dont ")
	f.check("dont dont dont ")
}

func TestManualFixesPromote(t *testing.T) {
	f := newField(t, "")
	// "qwop" is not a word and nothing is close: kept as typed.
	f.typeText("qwop ")
	f.check("qwop ")
	fix := func() {
		for range 5 { // the space and the four letters
			f.backspace()
		}
		f.typeText("quip ")
	}
	fix()
	f.check("quip ")
	f.typeText("qwop ")
	fix()
	f.check("quip quip ")
	f.typeText("qwop ")
	f.check("quip quip quip ")
}

func TestCaseAndSentenceStart(t *testing.T) {
	f := newField(t, "")
	if !f.e.ShouldCapitalize() {
		t.Fatal("an empty field starts a sentence")
	}
	f.typeText("Hello i am in london. ")
	f.check("Hello I am in London. ")
	if !f.e.ShouldCapitalize() {
		t.Fatal("after a period")
	}
}

func TestMissedSpaceAndAutoSpace(t *testing.T) {
	f := newField(t, "")
	f.typeText("it is ofthe ")
	f.check("it is of the ")
	f.e.SetContext("")
	f.text = ""
	f.typeText("I want ")
	sugs := f.e.Suggestions(3)
	if len(sugs) == 0 {
		t.Fatal("no predictions")
	}
	f.apply(f.e.Pick(0), "")
	want := "I want " + sugs[0].Text + " "
	f.check(want)
	f.typeText(".")
	f.check(strings.TrimSuffix(want, " ") + ". ")
}

func TestPersonalWordsAndReplacements(t *testing.T) {
	f := newField(t, "")
	f.e.AddWord("Sayso")
	f.e.AddReplacement("omw", "on my way", false)
	f.typeText("sayso is omw ")
	f.check("Sayso is on my way ")
	f.backspace() // a replacement can be undone like a correction
	f.check("Sayso is omw")
}

func TestPickWhileTyping(t *testing.T) {
	f := newField(t, "")
	f.typeText("I want to beau")
	sugs := f.e.Suggestions(5)
	idx := -1
	for i, s := range sugs {
		if strings.HasPrefix(s.Text, "beaut") {
			idx = i
		}
	}
	if idx < 0 {
		t.Fatalf("no completion: %+v", sugs)
	}
	f.apply(f.e.Pick(idx), "")
	f.check("I want to " + sugs[idx].Text + " ")
}

func TestPrivacyWritesNothing(t *testing.T) {
	path := filepath.Join(t.TempDir(), "journal")
	f := newField(t, path)
	f.typeText("hello there ")
	f.e.SetFlags(FlagPrivacy | FlagAutocap)
	before, _ := os.ReadFile(path)
	f.typeText("my secret zxqv zxqv dont ")
	f.backspace()
	f.typeText(" ")
	f.e.AddWord("Hunter")
	after, _ := os.ReadFile(path)
	if string(before) != string(after) {
		t.Fatalf("privacy mode wrote:\n%s", after[len(before):])
	}
}

func TestJournalPersistsAcrossEngines(t *testing.T) {
	path := filepath.Join(t.TempDir(), "journal")
	f := newField(t, path)
	f.e.AddWord("Zorblax")
	f.e.Close()
	g := newField(t, path)
	g.typeText("zorblax ")
	g.check("Zorblax ")
	if err := g.e.ResetLearned(); err != nil {
		t.Fatal(err)
	}
	g.typeText("zorblax ")
	if strings.HasSuffix(g.text, "Zorblax ") {
		t.Fatal("reset kept the personal word")
	}
}

func TestSetContextIsIdempotent(t *testing.T) {
	f := newField(t, "")
	f.typeText("I dont")
	f.e.SetContext(f.text) // the app echoes the text it has
	f.typeText(" ")
	f.check("I don't ")
	// A cursor move into the middle of a word reopens it.
	f.e.SetContext("I want teh")
	f.text = "I want teh"
	f.typeText(" ")
	f.check("I want the ")
}

func TestDiagnostics(t *testing.T) {
	f := newField(t, "")
	f.typeText("I want t")
	d := f.e.Diagnostics(5)
	if len(d.Keys) != f.e.layout.Len() || len(d.Cands) == 0 || d.Typed != "t" {
		t.Fatalf("%+v", d)
	}
	var any bool
	for _, k := range d.Keys {
		if k.Expansion > 0 {
			any = true
		}
	}
	if !any {
		t.Fatal("no key expands after \"t\"")
	}
	if d.Cands[0].Kind != decoder.KindWord && d.Cands[0].Kind != decoder.KindCompletion {
		t.Fatal(d.Cands[0])
	}
}

func TestLayoutChange(t *testing.T) {
	f := newField(t, "")
	if err := f.e.SetLayout(touch.QWERTY(30, 44)); err != nil {
		t.Fatal(err)
	}
	f.typeText("hello ")
	f.check("hello ")
	if err := f.e.SetLayout(nil); err == nil {
		t.Fatal("empty layout accepted")
	}
}

func TestOnTouchDoesNotAllocate(t *testing.T) {
	f := newField(t, "")
	l := f.e.layout
	f.typeText("I want ")
	x, y := l.Center(l.LetterKey('g'))
	allocs := testing.AllocsPerRun(500, func() {
		f.e.OnTouch(x, y, false)
		if len(f.e.word) >= 20 {
			f.e.mu.Lock()
			f.e.before = f.e.before[:len(f.e.before)-len(f.e.typed)]
			f.e.resetWord()
			f.e.mu.Unlock()
		}
	})
	if allocs != 0 {
		t.Fatalf("%v allocs per tap", allocs)
	}
}

func TestVeryLongWordDoesNotBreakTheMirror(t *testing.T) {
	f := newField(t, "")
	f.typeText(strings.Repeat("abcdefghij", 40) + " fine ")
	if !strings.HasSuffix(f.e.Before(), " fine ") {
		t.Fatalf("mirror %q", f.e.Before())
	}
}
