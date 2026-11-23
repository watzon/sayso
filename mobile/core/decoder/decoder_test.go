package decoder

import (
	"strings"
	"sync"
	"testing"

	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/touch"
)

var (
	modelOnce sync.Once
	model     *lexicon.Model
	modelErr  error
)

func loadModel(t testing.TB) *lexicon.Model {
	t.Helper()
	modelOnce.Do(func() { model, modelErr = lexicon.Open("../../models/en-US.sslm") })
	if modelErr != nil {
		t.Fatal(modelErr)
	}
	return model
}

func newDecoder(t testing.TB) *Decoder {
	m := loadModel(t)
	l, err := touch.NewLayout(touch.QWERTY(36, 54))
	if err != nil {
		t.Fatal(err)
	}
	return New(m, touch.NewModel(l, touch.DefaultParams), DefaultParams)
}

// tapAt types letter c with a tap offset from the key centre by (dx, dy)
// key widths.
func tapAt(d *Decoder, c byte, dx, dy float32) {
	l := d.tm.L
	x, y := l.Center(l.LetterKey(c))
	x += dx * l.Pitch()
	y += dy * l.Pitch()
	var p touch.Prior
	d.NextPrior(&p)
	r := d.tm.Resolve(x, y, &p)
	var dist touch.Dist
	d.tm.Spatial(x, y, &dist)
	typed := c
	if l.Kind(r.Key) == touch.KindLetter {
		typed = byte(l.Keys[r.Key].Code)
	}
	d.AddTap(&dist, typed)
}

func typeWord(d *Decoder, w string) {
	d.Reset()
	for i := 0; i < len(w); i++ {
		if w[i] == '\'' {
			d.AddApostrophe()
			continue
		}
		tapAt(d, w[i], 0, 0)
	}
}

func wordText(d *Decoder, c Cand) string {
	s := string(d.models[c.Src].Word(c.Word))
	if c.Word2 != lexicon.NoWord {
		s += " " + string(d.models[c.Src].Word(c.Word2))
	}
	return s
}

func setContext(t testing.TB, d *Decoder, words ...string) {
	m := d.Model()
	ids := []lexicon.WordID{lexicon.NoWord, m.BOS()}
	for _, w := range words {
		id, ok := m.Find([]byte(w), true)
		if !ok {
			t.Fatalf("%q not in the model", w)
		}
		ids = append(ids, id)
	}
	d.SetContext(ids[len(ids)-2], ids[len(ids)-1])
}

func TestAutocorrect(t *testing.T) {
	d := newDecoder(t)
	cases := []struct {
		ctx         []string
		typed, want string // want "" means keep what was typed
	}{
		{nil, "dont", "don't"},
		{nil, "im", "I'm"},
		{[]string{"I"}, "teh", ""},     // "teh" after "I": not a word; see below
		{[]string{"in"}, "teh", "the"}, // transposition
		{[]string{"I", "would"}, "liek", "like"},
		{[]string{"it", "is"}, "ofthe", "of the"},
		{[]string{"I"}, "wanr", "want"}, // a far substitution
		// s for a is one key over; w for i ("isn't") is six.
		{nil, "wsnt", "want"},
		{[]string{"I"}, "wsnt", "want"},
		{[]string{"this", "is"}, "beautifl", "beautiful"},
		{[]string{"I"}, "think", ""}, // a word stays
		{[]string{"the"}, "cat", ""},
		{[]string{"we", "were"}, "there", ""},
		{[]string{"I", "like"}, "its", ""}, // key-mate of "it's" with no strong reason
	}
	for _, c := range cases {
		setContext(t, d, c.ctx...)
		typeWord(d, c.typed)
		dec := d.Decide(nil)
		got := ""
		if dec.Correct {
			got = wordText(d, dec.Cand)
		}
		if c.typed == "teh" && c.ctx[0] == "I" {
			continue // only checks that nothing panics in an odd context
		}
		if got != c.want {
			t.Errorf("%v %q: got %q (margin %.2f, best %q), want %q", c.ctx, c.typed, got, dec.Margin, wordText(d, dec.Cand), c.want)
		}
	}
}

func TestNoisyTaps(t *testing.T) {
	d := newDecoder(t)
	setContext(t, d, "I", "want", "to")
	// "going" with every tap pushed a third of a key to the right.
	d.Reset()
	for _, c := range []byte("going") {
		tapAt(d, c, 0.35, 0.1)
	}
	cands, _ := d.Score(nil)
	if len(cands) == 0 || wordText(d, cands[0]) != "going" {
		t.Fatalf("top %v", texts(d, cands))
	}
}

func texts(d *Decoder, cs []Cand) []string {
	var out []string
	for i, c := range cs {
		if i == 5 {
			break
		}
		out = append(out, wordText(d, c))
	}
	return out
}

func TestSuggestionsAndPredictions(t *testing.T) {
	d := newDecoder(t)
	setContext(t, d, "I", "don't")
	var out [3]Suggestion
	n := d.Suggest(nil, out[:])
	var preds []string
	for _, s := range out[:n] {
		preds = append(preds, wordText(d, s.Cand))
	}
	if n != 3 || !contains(preds, "know") || !contains(preds, "think") {
		t.Fatalf("predictions after \"I don't\": %v", preds)
	}
	setContext(t, d, "I", "want")
	typeWord(d, "beau")
	n = d.Suggest(nil, out[:])
	if n == 0 || out[0].Flags&SugLiteral == 0 {
		t.Fatalf("first suggestion should be what was typed: %+v", out[:n])
	}
	var all []string
	for _, s := range out[:n] {
		if s.Word != lexicon.NoWord {
			all = append(all, wordText(d, s.Cand))
		}
	}
	if !containsPrefix(all, "beaut") {
		t.Fatalf("completions of \"beau\": %v", all)
	}
	typeWord(d, "dont")
	n = d.Suggest(nil, out[:])
	if n == 0 || out[0].Flags&SugAutocorrect == 0 || wordText(d, out[0].Cand) != "don't" {
		t.Fatalf("dont: %+v", out[:n])
	}
}

func contains(s []string, w string) bool {
	for _, x := range s {
		if x == w {
			return true
		}
	}
	return false
}

func containsPrefix(s []string, p string) bool {
	for _, x := range s {
		if strings.HasPrefix(x, p) {
			return true
		}
	}
	return false
}

func TestBackspaceRebuildsTheSameBeam(t *testing.T) {
	d := newDecoder(t)
	typeWord(d, "hellp")
	d.RemoveLast()
	tapAt(d, 'o', 0, 0)
	a, _ := d.Score(nil)
	first := wordText(d, a[0])
	typeWord(d, "hello")
	b, _ := d.Score(nil)
	if first != wordText(d, b[0]) || a[0].Total != b[0].Total {
		t.Fatalf("%q %v vs %q %v", first, a[0].Total, wordText(d, b[0]), b[0].Total)
	}
	typeWord(d, "it's")
	if got := string(d.Typed(nil)); got != "it's" {
		t.Fatal(got)
	}
	_, lit := d.Score(nil)
	if !lit.IsWord || string(d.Model().Word(lit.Word)) != "it's" {
		t.Fatal("it's typed with its apostrophe is a word")
	}
	d.RemoveLast() // s
	d.RemoveLast() // t
	d.RemoveLast() // the apostrophe
	if got := string(d.Typed(nil)); got != "i" {
		t.Fatal(got)
	}
}

func TestStepDoesNotAllocate(t *testing.T) {
	d := newDecoder(t)
	setContext(t, d, "I", "want")
	l := d.tm.L
	x, y := l.Center(l.LetterKey('g'))
	var dist touch.Dist
	var p touch.Prior
	var out [3]Suggestion
	allocs := testing.AllocsPerRun(200, func() {
		d.Reset()
		for i := 0; i < 6; i++ {
			d.NextPrior(&p)
			d.tm.Resolve(x, y, &p)
			d.tm.Spatial(x, y, &dist)
			d.AddTap(&dist, 'g')
		}
		d.Decide(nil)
		d.Suggest(nil, out[:])
	})
	if allocs != 0 {
		t.Fatalf("%v allocs", allocs)
	}
}
