package localengine

import "testing"

func TestAutoOrEmptyLetsTheModelFindTheLanguage(t *testing.T) {
	for _, auto := range []string{"auto", "AUTO", "", "  ", " auto "} {
		if !isAutoLanguage(auto) {
			t.Errorf("%q is auto", auto)
		}
	}
	for _, named := range []string{"en", "de-DE", "yue"} {
		if isAutoLanguage(named) {
			t.Errorf("%q is not auto", named)
		}
	}
}

func TestLibraryFormsBecomeCatalogCodes(t *testing.T) {
	for in, want := range map[string]string{"<|en|>": "en", "<|yue|>": "yue", "de": "de", " EN ": "en", "": "", "<||>": ""} {
		if got := normalizeLanguage(in); got != want {
			t.Errorf("normalize(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestTheLanguageOfTheMostAudioWins(t *testing.T) {
	var tally languageTally
	tally.add(28*16_000, "Guten Tag.", "de")
	tally.add(10*16_000, "Hello.", "en")
	tally.add(25*16_000, "Hello again.", "en")
	if got := tally.mostAudio(); got != "en" {
		t.Fatalf("35 s of English against 28 s of German: %q", got)
	}
}

func TestEqualAudioKeepsTheFirstLanguage(t *testing.T) {
	var tally languageTally
	tally.add(100, "Hallo", "de")
	tally.add(100, "Hello", "en")
	if got := tally.mostAudio(); got != "de" {
		t.Fatalf("%q", got)
	}
}

func TestPiecesWithoutTextOrLanguageDoNotCount(t *testing.T) {
	var tally languageTally
	tally.add(1_000_000, "  ", "en")
	tally.add(1_000_000, "words", "")
	if got := tally.mostAudio(); got != "" {
		t.Fatalf("%q", got)
	}
	tally.add(10, "Hallo", "de")
	if got := tally.mostAudio(); got != "de" {
		t.Fatalf("%q", got)
	}
}

func TestEndpointsCommitTextAndChangesAreReportedOnce(t *testing.T) {
	var p partials
	check := func(what string, wantOK bool, wantC, wantT string) {
		t.Helper()
		c, tt, ok := p.changed()
		if ok != wantOK || c != wantC || tt != wantT {
			t.Fatalf("%s: got (%q, %q, %v), want (%q, %q, %v)", what, c, tt, ok, wantC, wantT, wantOK)
		}
	}
	p.update(segment{text: ""})
	check("an empty stream sends nothing", false, "", "")
	p.update(segment{text: "hello"})
	check("first text", true, "", "hello")
	p.update(segment{text: "hello"})
	check("no change, no event", false, "", "")
	p.update(segment{text: "hello world ", endpoint: true})
	check("endpoint", true, "hello world", "")
	p.update(segment{text: "again"})
	check("after endpoint", true, "hello world", "again")
	if got := p.text(); got != "hello world again" {
		t.Fatalf("text %q", got)
	}
	p.update(segment{text: "again and more", endpoint: true})
	p.update(segment{text: "", endpoint: true}) // An empty segment commits nothing.
	if got := p.text(); got != "hello world again and more" {
		t.Fatalf("text %q", got)
	}
	check("final", true, "hello world again and more", "")
}
