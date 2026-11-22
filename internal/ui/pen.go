package ui

import (
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/theme"
)

// The pen of the live preview (Rust overlay/pen.rs): it writes the words
// out on the slip.
//
// New letters do not show all at once. The pen writes them one by one at a
// hand's pace, and each letter fades in like wet ink. The tentative words
// are a pencil draft. When the speech model commits them, a second pen
// traces over the draft, and each letter turns from pencil to ink.
//
// A letter that the pen has not written yet keeps its place in the text,
// but it does not show. So the lines do not move while the pen writes.

const (
	// penWritePace is the letters per second that the pen writes when it
	// is not behind.
	penWritePace = 42
	// penWriteCatchUp: the pen writes faster when the model sends much
	// text at once. Its speed is the letters it has still to write,
	// divided by this many seconds.
	penWriteCatchUp = 0.35
	// penInkPace is the letters per second that the ink traces over the draft.
	penInkPace    = 32
	penInkCatchUp = 0.5
	// penWet is the seconds for a letter to fade in after the pen writes it.
	penWet = 0.14
	// penInking is the seconds for a letter to turn from pencil to ink.
	penInking = 0.22
)

// penLetter is one letter of the text, and when the pens got to it, in
// seconds on the clock of the pen.
type penLetter struct {
	ch                 rune
	written, inked     float32
	isWritten, isInked bool
}

// stroke is how one letter looks in a frame.
type stroke struct {
	opacity float32 // 0: not written yet; 1: written, and the ink is dry
	ink     float32 // 0: pencil; 1: ink
}

type pen struct {
	letters []penLetter
	// committed is the number of first letters that are committed; the
	// rest is the tentative tail.
	committed int
	// written and inked are how far the pens are, in letters. They are
	// fractional, so a slow frame rate does not change the pace.
	written, inked float32
	now            float32 // seconds since the pen started
	last           time.Time
}

// clear forgets the text: the next text is written from its start.
func (p *pen) clear() {
	p.letters, p.committed, p.written, p.inked = p.letters[:0], 0, 0, 0
}

// set gives the pen the text of this frame. Letters that did not change
// keep their ink; the pen goes back to the first letter that changed.
func (p *pen) set(committed, tentative string) {
	text := []rune(committed)
	committedLen := len(text)
	if needsSpace(committed, tentative) {
		text = append(text, ' ')
	}
	text = append(text, []rune(tentative)...)
	same := 0
	for same < len(p.letters) && same < len(text) && p.letters[same].ch == text[same] {
		same++
	}
	if same == len(text) && same == len(p.letters) && committedLen == p.committed {
		return
	}
	p.letters = p.letters[:same]
	for _, ch := range text[same:] {
		p.letters = append(p.letters, penLetter{ch: ch})
	}
	// A letter that is no longer committed goes back to pencil.
	for i := min(committedLen, same); i < len(p.letters); i++ {
		p.letters[i].isInked = false
	}
	p.committed = committedLen
	p.written = min(p.written, float32(same))
	p.inked = min(p.inked, float32(min(committedLen, same)))
}

// update moves the pens on to now. With reduce, all of the text shows at
// once. It returns true while the pens move or the ink is wet.
func (p *pen) update(now time.Time, reduce bool) bool {
	// A clock that goes back (a new window) moves the pen by nothing.
	var dt float32
	if !p.last.IsZero() && now.After(p.last) {
		dt = float32(now.Sub(p.last).Seconds())
	}
	p.last = now
	if reduce {
		p.finish()
	} else {
		p.advance(dt)
	}
	return p.moving()
}

func (p *pen) advance(dt float32) {
	// A long pause (the first frame) does not write a burst of letters.
	dt = max(0, min(dt, 1.0/20))
	p.now += dt
	n := float32(len(p.letters))
	behind := n - p.written
	p.written = min(p.written+max(penWritePace, behind/penWriteCatchUp)*dt, n)
	toInk := min(float32(p.committed), p.written)
	if behind := toInk - p.inked; behind > 0 {
		p.inked = min(p.inked+max(penInkPace, behind/penInkCatchUp)*dt, toInk)
	}
	written, inked := int(p.written), int(p.inked)
	for i := range p.letters {
		l := &p.letters[i]
		if i < written && !l.isWritten {
			l.written, l.isWritten = p.now, true
		}
		if i < inked && !l.isInked {
			l.inked, l.isInked = p.now, true
		}
	}
}

// finish writes and inks all of the text, dry.
func (p *pen) finish() {
	dry := p.now - max(penWet, penInking)
	p.written, p.inked = float32(len(p.letters)), float32(p.committed)
	for i := range p.letters {
		l := &p.letters[i]
		if !l.isWritten || l.written > dry {
			l.written, l.isWritten = dry, true
		}
		if i < p.committed && (!l.isInked || l.inked > dry) {
			l.inked, l.isInked = dry, true
		}
	}
}

// moving is true while a pen moves, or ink is still wet.
func (p *pen) moving() bool {
	if p.written < float32(len(p.letters)) || p.inked < min(float32(p.committed), p.written) {
		return true
	}
	for _, l := range p.letters {
		if (l.isWritten && p.now-l.written < penWet) || (l.isInked && p.now-l.inked < penInking) {
			return true
		}
	}
	return false
}

// caughtUp is true when the pen has written all of the text.
func (p *pen) caughtUp() bool { return p.written >= float32(len(p.letters)) }

// stroke is how letter i looks in this frame.
func (p *pen) stroke(i int) stroke {
	if i < 0 || i >= len(p.letters) {
		return stroke{}
	}
	l := p.letters[i]
	ease := func(set bool, t, span float32) float32 {
		if !set {
			return 0
		}
		return smoothstep(max(0, min((p.now-t)/span, 1)))
	}
	return stroke{opacity: ease(l.isWritten, l.written, penWet), ink: ease(l.isInked, l.inked, penInking)}
}

// smoothstep eases in and out, so a stroke starts and ends softly.
func smoothstep(t float32) float32 { return t * t * (3 - 2*t) }

// previewSpans is the text of the live preview with its look: committed
// words in ink, the tentative tail in pencil italic, and the caret. Only
// the last max letters of the committed words show (from a word start,
// after an ellipsis), so the slip stays at about three lines; the latest
// words always show (Rust overlay preview_text).
func previewSpans(pn *pen, maxLetters int, p theme.Colors) []mu.Span {
	letters := make([]rune, len(pn.letters))
	for i, l := range pn.letters {
		letters[i] = l.ch
	}
	start := tailStart(letters[:pn.committed], maxLetters)
	end := start
	for i := len(letters) - 1; i >= start; i-- {
		if !isSpace(letters[i]) {
			end = i + 1
			break
		}
	}
	var spans []mu.Span
	if start > 0 {
		spans = append(spans, mu.Span{Text: "…", Color: p.Ink})
	}
	// Letters with the same stroke share one span: dry ink, dry pencil.
	var last stroke
	for i := start; i < end; i++ {
		s := pn.stroke(i)
		if n := len(spans); n > 0 && i > start && s == last {
			spans[n-1].Text += string(letters[i])
			continue
		}
		spans = append(spans, strokeSpan(string(letters[i]), s, p))
		last = s
	}
	// The caret shows when the pen has caught up with the words.
	caret := float32(0)
	if pn.caughtUp() {
		caret = 0.7
	}
	return append(spans, mu.Span{Text: " │", Color: p.Accent.Alpha(caret)})
}

// strokeSpan is the look of letters: pencil italic that turns to ink, and
// fades in as it is written.
func strokeSpan(text string, s stroke, p theme.Colors) mu.Span {
	a, b := p.Pencil, p.Ink
	mix := func(x, y uint8) uint8 { return uint8(float32(x) + (float32(y)-float32(x))*s.ink + 0.5) }
	col := mu.Color{R: mix(a.R, b.R), G: mix(a.G, b.G), B: mix(a.B, b.B), A: mix(a.A, b.A)}.Alpha(s.opacity)
	return mu.Span{Text: text, Color: col, Italic: s.ink < 0.5}
}

// tailStart is the first committed letter that shows: the last max
// letters, from the first space among them.
func tailStart(committed []rune, maxLetters int) int {
	if len(committed) <= maxLetters {
		return 0
	}
	cut := len(committed) - maxLetters
	for i, ch := range committed[cut:] {
		if ch == ' ' {
			return cut + i
		}
	}
	return cut
}

func isSpace(r rune) bool { return r == ' ' || r == '\t' || r == '\n' || r == '\r' }

// needsSpace is true when the committed text and the tentative tail need a
// space between them: not when either side is empty, and not next to a
// script without spaces between words.
func needsSpace(committed, tentative string) bool {
	c, t := []rune(committed), []rune(tentative)
	if len(c) == 0 || len(t) == 0 {
		return false
	}
	return !isUnspaced(c[len(c)-1]) && !isUnspaced(t[0])
}

// isUnspaced is a character of a script with no spaces between words
// (Chinese, Japanese, Thai, Lao, Myanmar, Khmer); the ranges of Rust's
// is_unspaced.
func isUnspaced(r rune) bool {
	switch {
	case r >= 0x0E00 && r <= 0x0EFF, r >= 0x1000 && r <= 0x109F, r >= 0x1780 && r <= 0x17FF,
		r >= 0x3000 && r <= 0x30FF, r >= 0x3400 && r <= 0x4DBF, r >= 0x4E00 && r <= 0x9FFF,
		r >= 0xF900 && r <= 0xFAFF, r >= 0xFF00 && r <= 0xFFEF:
		return true
	}
	return false
}
