package ui

import (
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
)

var chipWords = []string{"Watzon", "Sayso", "MyGo", "Kubernetes", "Fraunces", "WhisperKit", "Parakeet", "Tailscale", "Roost", "Postgres", "Grafana"}

// chipRects are the boxes of every word's text and remove button.
func chipRects(tt *mu.Tester) map[string]mu.Rect {
	out := map[string]mu.Rect{}
	for _, a := range tt.AccessAll() {
		for _, w := range chipWords {
			if a.Name == w || a.Name == i18n.T("dictionary.remove_word", "word", w) {
				out[a.Name] = a.Bounds
			}
		}
	}
	return out
}

// Hovering a word's slip shows its remove button without changing the
// size of any slip, so no slip moves to another line under the pointer.
func TestDictionaryChipsKeepTheirSizeOnHover(t *testing.T) {
	e := newEnv(t)
	for _, w := range chipWords {
		e.a.AddWord(w)
	}
	e.a.SetPage(PageDictionary)
	tt := e.hub(t)
	tt.Advance(time.Second)
	before := chipRects(tt)
	if len(before) != 2*len(chipWords) {
		t.Fatalf("found %d of %d boxes: %v", len(before), 2*len(chipWords), before)
	}
	// The last slip of each line, and the others.
	lineEnd := map[float32]string{}
	for _, w := range chipWords {
		r := before[w]
		if end, ok := lineEnd[r.Y]; !ok || r.X > before[end].X {
			lineEnd[r.Y] = w
		}
	}
	if len(lineEnd) < 2 {
		t.Fatalf("the words fit on one line: %v", lineEnd)
	}
	for _, w := range chipWords {
		r := before[w]
		tt.Move(r.X+r.W/2, r.Y+r.H/2)
		tt.Advance(time.Second)
		for name, b := range chipRects(tt) {
			if b != before[name] {
				t.Errorf("hovering %q moved %q from %+v to %+v", w, name, before[name], b)
			}
		}
	}
	// Keyboard focus shows the button too, and moves nothing either.
	tt.Move(0, 0)
	tt.Advance(time.Second)
	press := i18n.T("dictionary.remove_word", "word", lineEnd[before[chipWords[0]].Y])
	tt.Key(0, mu.KeyTab)
	for range len(chipWords) * 3 {
		if tt.Focused(press) {
			break
		}
		tt.Key(0, mu.KeyTab)
	}
	tt.Advance(time.Second)
	for name, b := range chipRects(tt) {
		if b != before[name] {
			t.Errorf("focus moved %q from %+v to %+v", name, before[name], b)
		}
	}
}

// labelWidth is the width of the add-word label at 14 DIPs, as the hub
// measures it.
func labelWidth(t *testing.T) float32 {
	t.Helper()
	var w float32
	mu.NewTester(func(c *mu.Context) {
		w, _ = c.MeasureText(0, mu.Span{Text: i18n.T("dictionary.add_word"), Size: 14, Weight: 400})
	}, 100, 100)
	return w
}

// The dashed "+ Add word" slip shows its whole label, at rest and as a
// well, in English and in the longest translations.
func TestDictionaryAddSlipFitsItsLabel(t *testing.T) {
	for _, locale := range []string{"en", "pt-BR", "fr", "de"} {
		t.Run(locale, func(t *testing.T) {
			e := newEnv(t)
			i18n.SetLocale(locale)
			e.a.SetPage(PageDictionary)
			tt := e.hub(t)
			tt.Advance(time.Second)
			name := i18n.T("dictionary.placeholder.new_word")
			need := labelWidth(t)
			in, ok := controlBounds(tt, name)
			if !ok {
				t.Fatalf("no field %q: %q", name, tt.Texts())
			}
			if in.W < need {
				t.Errorf("at rest the field is %v wide for a label of %v", in.W, need)
			}
			tt.ClickAt(in.X+in.W/2, in.Y+in.H/2)
			tt.Advance(time.Second)
			open, _ := controlBounds(tt, name)
			if open.W < need || open.W <= in.W {
				t.Errorf("as a well the field is %v wide (at rest %v) for a label of %v", open.W, in.W, need)
			}
		})
	}
}

// A click opens the slip into a well over frames, not in one jump.
func TestDictionaryAddSlipEasesIntoAWell(t *testing.T) {
	e := newEnv(t)
	e.a.SetPage(PageDictionary)
	tt := e.hub(t)
	tt.Advance(time.Second)
	name := i18n.T("dictionary.placeholder.new_word")
	in, _ := controlBounds(tt, name)
	tt.ClickAt(in.X+in.W/2, in.Y+in.H/2)
	open := sample(tt, time.Second, func() float32 { r, _ := controlBounds(tt, name); return r.W })
	full := open[len(open)-1]
	if full <= in.W {
		t.Fatalf("the well is %v wide, the slip %v", full, in.W)
	}
	if n := strictlyBetween(open, in.W, full); n < 5 {
		t.Errorf("the slip opened in %d frames: %v", n, open[:12])
	}
}
