package ui

import (
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/theme"
)

// The tests of Rust overlay/pen.rs.

func runPen(p *pen, seconds float32) {
	for t := float32(0); t < seconds; t += 1.0 / 60 {
		p.advance(1.0 / 60)
	}
}

func shown(p *pen) string {
	var b strings.Builder
	for i, l := range p.letters {
		if p.stroke(i).opacity > 0 {
			b.WriteRune(l.ch)
		}
	}
	return b.String()
}

func TestPenWritesNewLettersOneByOne(t *testing.T) {
	var p pen
	p.set("", "hello there")
	runPen(&p, 0.1)
	part := shown(&p)
	if part == "" || len(part) >= len("hello there") || !strings.HasPrefix("hello there", part) {
		t.Fatalf("after 0.1 s: %q", part)
	}
	runPen(&p, 1)
	if shown(&p) != "hello there" || p.moving() || p.stroke(0).opacity != 1 {
		t.Fatalf("after 1 s: %q, moving %v, %+v", shown(&p), p.moving(), p.stroke(0))
	}
}

func TestPenCatchesUpWhenMuchTextComesAtOnce(t *testing.T) {
	var p pen
	// 200 letters take almost 5 seconds at the normal pace.
	p.set(strings.Repeat("word ", 40), "")
	runPen(&p, 1.5)
	if !p.caughtUp() {
		t.Fatalf("written %v of %d", p.written, len(p.letters))
	}
}

func TestPenRevisionKeepsTheLettersBeforeIt(t *testing.T) {
	var p pen
	p.set("", "I went their")
	runPen(&p, 1)
	p.set("", "I went there")
	if got := shown(&p); got != "I went the" {
		t.Fatalf("after the revision: %q", got)
	}
	runPen(&p, 1)
	if got := shown(&p); got != "I went there" {
		t.Fatalf("%q", got)
	}
}

func TestPenTurnsCommittedWordsFromPencilToInk(t *testing.T) {
	var p pen
	p.set("", "good morning")
	runPen(&p, 1)
	if p.stroke(0).ink != 0 {
		t.Fatal("a tentative letter is inked")
	}
	p.set("good morning", "")
	runPen(&p, 0.05)
	// The ink starts at the first letter and goes on from there.
	if !(p.stroke(0).ink > 0 && p.stroke(11).ink == 0) {
		t.Fatalf("%+v %+v", p.stroke(0), p.stroke(11))
	}
	runPen(&p, 1)
	for i := range p.letters {
		if p.stroke(i).ink != 1 {
			t.Fatalf("letter %d: %+v", i, p.stroke(i))
		}
	}
	if shown(&p) != "good morning" {
		t.Fatalf("%q", shown(&p))
	}
}

func TestPenWordNoLongerCommittedGoesBackToPencil(t *testing.T) {
	var p pen
	p.set("one two", "")
	runPen(&p, 1)
	p.set("one", "two")
	if p.stroke(0).ink != 1 || p.stroke(4).ink != 0 {
		t.Fatalf("%+v %+v", p.stroke(0), p.stroke(4))
	}
}

func TestPenReduceMotionShowsAllAtOnce(t *testing.T) {
	var p pen
	p.set("good", "morning")
	p.finish()
	if shown(&p) != "good morning" || p.stroke(0) != (stroke{1, 1}) || p.stroke(6) != (stroke{1, 0}) || p.moving() {
		t.Fatalf("%q %+v %+v moving %v", shown(&p), p.stroke(0), p.stroke(6), p.moving())
	}
}

func TestPreviewSpansKeepTheLatestWords(t *testing.T) {
	var p pen
	p.set(strings.TrimSpace(longSpeech), "and the latest")
	p.finish()
	spans := previewSpans(&p, 112, sumiLight())
	var text strings.Builder
	for _, s := range spans {
		text.WriteString(s.Text)
	}
	got := text.String()
	if !strings.HasPrefix(got, "…") || !strings.HasSuffix(got, "and the latest │") {
		t.Fatalf("preview %q", got)
	}
	// About the last 112 committed letters, from a word start, and the tail.
	if n := len([]rune(got)); n > 112+len("… and the latest │")+1 {
		t.Fatalf("%d letters: %q", n, got)
	}
	last := spans[len(spans)-2]
	if !last.Italic || last.Color != sumiLight().Pencil {
		t.Errorf("the tentative tail is not pencil italic: %+v", last)
	}
	if spans[1].Italic {
		t.Errorf("committed words are not ink: %+v", spans[1])
	}
}

func TestTailStartAndSpaces(t *testing.T) {
	if got := tailStart([]rune("hello world"), 50); got != 0 {
		t.Errorf("short text: %d", got)
	}
	if got := tailStart([]rune("one two three"), 7); got != 7 {
		t.Errorf("cut at the space before %q: %d", "three", got)
	}
	if !needsSpace("hello", "world") || needsSpace("", "world") || needsSpace("今日は", "いい天気") || needsSpace("I said", "こんにちは") {
		t.Error("needsSpace")
	}
}

func sumiLight() theme.Colors {
	return theme.NewPalette(theme.Ink{Preset: theme.Sumi}, false).Colors()
}
