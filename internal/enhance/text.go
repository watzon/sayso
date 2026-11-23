// Package enhance is the text pipeline of a dictation: dictionary
// replacements, spoken punctuation, spelled words, styles, and the AI
// enhancers (ports of sayso-core dictionary/spoken_punctuation/spelled/style/
// enhance/pipeline, sayso-cleanup-models, sayso-cleanup-client, and
// sayso-enhance).
//
// The pure text rules live in mobile/core/textrules, which the mobile
// keyboard shares; this file adapts them to the store types.
package enhance

import (
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/mobile/core/textrules"
)

// Helpers of textrules that the cleanup code uses.
var (
	isAlnum           = textrules.IsAlnum
	wordBefore        = textrules.WordBefore
	wordAfter         = textrules.WordAfter
	findAccepted      = textrules.FindAccepted
	editDistance      = textrules.EditDistance
	quoteForRegexp    = textrules.QuoteForRegexp
	mustCompileCached = textrules.MustCompileCached
)

// Replacer holds compiled replacement rules. Build once per dictionary change.
type Replacer struct {
	r *textrules.Replacer
}

// NewReplacer compiles the rules: longer phrases first, so "new paragraph"
// wins over "new".
func NewReplacer(replacements []store.Replacement) *Replacer {
	rules := make([]textrules.Rule, len(replacements))
	for i, r := range replacements {
		rules[i] = textrules.Rule{ID: r.ID, From: r.From, To: r.To, CaseSensitive: r.CaseSensitive}
	}
	return &Replacer{r: textrules.NewReplacer(rules)}
}

// Apply runs every rule and returns the text and the rules that changed it.
func (r *Replacer) Apply(text string) (string, []store.AppliedReplacement) {
	out, applied := r.r.Apply(text)
	trail := make([]store.AppliedReplacement, len(applied))
	for i, a := range applied {
		trail[i] = store.AppliedReplacement{ReplacementID: a.RuleID, From: a.From, To: a.To, Count: a.Count}
	}
	return out, trail
}

// WordsHeard is the dictionary words that the transcript can contain: with
// the correct spelling, in separate words, or with a small error (at most
// one third of the characters).
func WordsHeard(words []string, transcript string) []string {
	return textrules.WordsHeard(words, transcript)
}

// SpokenPunctuation changes spoken commands ("comma", "new line") into marks
// and line breaks. A mark attaches to the word before it; the word after a
// sentence end or a line break starts with a capital letter.
func SpokenPunctuation(text string) string { return textrules.SpokenPunctuation(text) }

// Spelled joins the letters of each spelled word ("cats, that's K A T Z")
// and puts the word in place of the cue and the words it corrects.
func Spelled(text string) string { return textrules.Spelled(text) }
