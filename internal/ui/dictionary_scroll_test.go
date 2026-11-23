package ui

import (
	"fmt"
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
)

// fillDictionary adds words and replacements, for a page longer than the
// window.
func fillDictionary(t testing.TB, e *testEnv, words, reps int) {
	t.Helper()
	names := []string{"Claude Code", "Devhouse", "Fenneko", "ForgeCAD", "MayFirmOS", "Pindrop", "Sayso", "Kubernetes",
		"Fraunces", "WhisperKit", "Parakeet", "Tailscale", "Roost", "Postgres", "Grafana", "Homelytics", "Butterbase",
		"Attune", "Trellis", "Sprout", "Thorn", "MyGo", "Cloudflare", "Astro", "Bitstorm", "FluidAudio", "Riolink"}
	for i := range words {
		w := names[i%len(names)]
		if i >= len(names) {
			w = fmt.Sprintf("%s %d", w, i/len(names)+1)
		}
		e.a.AddWord(w)
	}
	for i := range reps {
		e.a.AddReplacement(fmt.Sprintf("Mac OS %d", i+1), fmt.Sprintf("macOS %d", i+1))
	}
	if e.a.dict.err != "" {
		t.Fatal(e.a.dict.err)
	}
	e.a.SetPage(PageDictionary)
}

func mustBounds(t *testing.T, tt *mu.Tester, name string) mu.Rect {
	t.Helper()
	b, ok := controlBounds(tt, name)
	if !ok {
		t.Fatalf("no control %q", name)
	}
	return b
}

// textBounds is the box of the first text that reads s.
func textBounds(t *testing.T, tt *mu.Tester, s string) mu.Rect {
	t.Helper()
	for _, a := range tt.AccessAll() {
		if a.Role == mu.RoleText && a.Name == s {
			return a.Bounds
		}
	}
	t.Fatalf("no text %q", s)
	return mu.Rect{}
}

// On a wide page the words and the replacements scroll on their own, and
// Try a phrase and the row that adds a rule stay at the foot of the window
// (Chris, 2026-11-22: the whole page used to scroll).
func TestDictionaryColumnsScrollOnTheirOwn(t *testing.T) {
	for _, locale := range []string{"en", "ar"} {
		t.Run(locale, func(t *testing.T) {
			e := newEnv(t)
			i18n.SetLocale(locale)
			e.a.cfg.General.Language = locale
			fillDictionary(t, e, 40, 30)
			tt := e.hub(t)
			tt.Frame()
			tt.Frame()
			d := &e.a.dict
			if d.wordsScroll.MaxY <= 0 || d.repsScroll.MaxY <= 0 {
				t.Fatalf("words scroll %+v, rules scroll %+v: want both to scroll", d.wordsScroll, d.repsScroll)
			}
			try := mustBounds(t, tt, i18n.T("dictionary.try.title"))
			add := mustBounds(t, tt, i18n.T("dictionary.placeholder.from"))
			window := mu.Rect{W: HubWidth, H: HubHeight}
			for name, b := range map[string]mu.Rect{"try": try, "add rule": add} {
				if !inside(b, window) || b.H < 15 {
					t.Errorf("%s at %+v is not in the window", name, b)
				}
				if b.Y+b.H < HubHeight-200 {
					t.Errorf("%s at %+v is not at the foot of the window", name, b)
				}
			}
			words, ok := tt.AccessByRole(mu.RoleGroup, i18n.T("dictionary.words"))
			if !ok {
				t.Fatal("no words group")
			}
			if words.Bounds.Y+words.Bounds.H > try.Y {
				t.Errorf("the words %+v run under Try a phrase %+v", words.Bounds, try)
			}
			head := textBounds(t, tt, strings.ToUpper(i18n.T("dictionary.column.from")))

			// Scrolling the words leaves the rules and the pinned parts be.
			tt.Scroll(words.Bounds.X+words.Bounds.W/2, words.Bounds.Y+words.Bounds.H/2, 0, 1000)
			tt.Frame()
			if d.wordsScroll.Y < d.wordsScroll.MaxY-1 || d.repsScroll.Y != 0 {
				t.Errorf("after scrolling the words: words %+v, rules %+v", d.wordsScroll, d.repsScroll)
			}
			if b := mustBounds(t, tt, i18n.T("dictionary.try.title")); b != try {
				t.Errorf("Try a phrase moved from %+v to %+v", try, b)
			}

			// Scrolling the rules leaves the words, the table's heading and
			// the add row be.
			first, ok := tt.Access("Mac OS 1")
			if !ok {
				t.Fatal("no first rule")
			}
			tt.Scroll(first.Bounds.X+first.Bounds.W/2, first.Bounds.Y+first.Bounds.H/2, 0, 1000)
			tt.Frame()
			if d.repsScroll.Y < d.repsScroll.MaxY-1 || d.wordsScroll.Y < d.wordsScroll.MaxY-1 {
				t.Errorf("after scrolling the rules: words %+v, rules %+v", d.wordsScroll, d.repsScroll)
			}
			for name, want := range map[string]mu.Rect{i18n.T("dictionary.try.title"): try, i18n.T("dictionary.placeholder.from"): add} {
				if b := mustBounds(t, tt, name); b != want {
					t.Errorf("%q moved from %+v to %+v", name, want, b)
				}
			}
			if b := textBounds(t, tt, strings.ToUpper(i18n.T("dictionary.column.from"))); b != head {
				t.Errorf("the table's heading moved from %+v to %+v", head, b)
			}
			if last, ok := tt.Access("Mac OS 30"); !ok || last.Offscreen || last.Bounds.Y+last.Bounds.H > add.Y+1 {
				t.Errorf("the last rule %+v is not in view above the add row %+v", last.Bounds, add)
			}
		})
	}
}

// With few entries, the rules end at the add row and Try a phrase stays
// at the foot of the words column.
func TestDictionaryShortListsKeepTheirPlace(t *testing.T) {
	e := newEnv(t)
	fillDictionary(t, e, 5, 3)
	tt := e.hub(t)
	tt.Frame()
	tt.Frame()
	if d := e.a.dict; d.wordsScroll.MaxY > 0 || d.repsScroll.MaxY > 0 {
		t.Errorf("short lists scroll: words %+v, rules %+v", d.wordsScroll, d.repsScroll)
	}
	last, _ := tt.Access("Mac OS 3")
	add := mustBounds(t, tt, i18n.T("dictionary.placeholder.from"))
	// The rule's text is centered in its 48 px row; the add row follows.
	if gap := add.Y - (last.Bounds.Y + last.Bounds.H); gap < 0 || gap > 32 {
		t.Errorf("the add row %+v is %.1f px under the last rule %+v", add, gap, last.Bounds)
	}
	if try := mustBounds(t, tt, i18n.T("dictionary.try.title")); try.Y+try.H < HubHeight-200 {
		t.Errorf("Try a phrase %+v is not at the foot of the window", try)
	}
}

// A narrow window stacks the sections and scrolls the page, so no section
// is squeezed to nothing.
func TestDictionaryNarrowStacks(t *testing.T) {
	e := newEnv(t)
	fillDictionary(t, e, 40, 30)
	tt := mu.NewTester(e.a.HubView, sidebarWidth+700, HubHeight)
	tt.Frame()
	tt.Frame()
	try := mustBounds(t, tt, i18n.T("dictionary.try.title"))
	head := textBounds(t, tt, strings.ToUpper(i18n.T("dictionary.column.from")))
	words, ok := tt.Access("Kubernetes")
	if !ok || words.Bounds.H < 10 || try.H < 15 || head.H < 10 {
		t.Fatalf("collapsed: word %+v, try %+v, table head %+v", words.Bounds, try, head)
	}
	if !(words.Bounds.Y < try.Y && try.Y < head.Y) {
		t.Errorf("not stacked words, try, rules: %+v %+v %+v", words.Bounds, try, head)
	}
	if e.a.dict.wordsScroll.MaxY != 0 || e.a.dict.repsScroll.MaxY != 0 {
		t.Errorf("a narrow page has inner scrolls")
	}
	if _, ok := tt.Access("Mac OS 30"); !ok {
		t.Errorf("the last rule is not on the page")
	}
}
