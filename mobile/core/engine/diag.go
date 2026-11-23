package engine

import (
	"github.com/watzon/sayso/mobile/core/decoder"
	"github.com/watzon/sayso/mobile/core/touch"
)

// KeyDiag is one key in the developer overlay.
type KeyDiag struct {
	Code rune
	// Expansion is how far the key reaches into its neighbours for the next
	// tap, in layout units.
	Expansion float32
	// DX, DY are the learned offset of the key's target, in layout units.
	DX, DY  float32
	Samples int // confirmed taps that taught the key
}

// CandDiag is one candidate for the word being typed.
type CandDiag struct {
	Text               string
	Total, Spatial, LM float32
	Edits              int
	Kind               decoder.Kind
}

// Diagnostics is what the developer overlay shows.
type Diagnostics struct {
	Keys          []KeyDiag
	Cands         []CandDiag
	Typed         string
	LiteralIsWord bool
	LiteralTotal  float32
	// Margin is the autocorrect margin of the word being typed: how far the
	// best correction clears its bar (negative: no autocorrect).
	Margin    float32
	Correct   bool
	LastTouch TouchResult
	Learner   string
}

// Diagnostics reports the engine's state for the next tap and the word
// being typed. It allocates; it is for the developer overlay.
func (e *Engine) Diagnostics(maxCands int) Diagnostics {
	e.mu.Lock()
	defer e.mu.Unlock()
	d := Diagnostics{Typed: string(e.typed), LastTouch: e.lastTouch, Learner: e.learn.String()}
	var prior touch.Prior
	if !e.raw {
		e.dec.NextPrior(&prior)
	}
	exp := make([]float32, e.layout.Len())
	e.tm.Expansion(&prior, exp)
	for i, k := range e.layout.Keys {
		dx, dy := e.tm.Offset(i)
		d.Keys = append(d.Keys, KeyDiag{Code: k.Code, Expansion: exp[i], DX: dx, DY: dy, Samples: e.learn.KeySamples(i)})
	}
	if len(e.word) > 0 && !e.raw {
		dec := e.dec.Decide(&e.bonus)
		d.Margin, d.Correct = dec.Margin, dec.Correct && !e.correctionsOff()
		d.LiteralIsWord, d.LiteralTotal = dec.Literal.IsWord, dec.Literal.Total
		cands, _ := e.dec.Score(&e.bonus)
		for i, c := range cands {
			if i == maxCands {
				break
			}
			d.Cands = append(d.Cands, CandDiag{Text: e.candText(c), Total: c.Total, Spatial: c.Spatial, LM: c.LM, Edits: int(c.Edits), Kind: c.Kind})
		}
	}
	return d
}

// Before is the engine's mirror of the text before the cursor (tests).
func (e *Engine) Before() string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return string(e.before)
}
