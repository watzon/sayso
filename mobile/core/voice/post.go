// Package voice is the dictation side of the mobile core: it turns a
// transcript into the text to insert (PostProcess), keeps custom styles and
// voice history (Store), and builds the prompts for cloud style enhancement.
// The network calls themselves are in package cloud.
package voice

import (
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/textrules"
)

// Dictionary is what post-processing reads from the typing engine.
type Dictionary struct {
	// Words are the user's personal words, spelled as they want them.
	Words []string
	// Replacements are the user's text replacements.
	Replacements []textrules.Rule
	// Spelling is the most likely spelling of a word in the model, ignoring
	// case ("hello" for "Hello", "I" for "i"), or false when the model lacks
	// it. Nil: case is left as the recognizer wrote it.
	Spelling func(word string) (string, bool)
	// Autocap capitalizes a transcript that starts a sentence (the field's
	// automatic capitalization).
	Autocap bool
}

// PostProcess turns a raw transcript into the text to insert at the cursor:
// the user's replacements, personal word spellings, spoken punctuation
// ("comma", "new line"), then the first letter's case and the spaces around
// the text, from the text before and after the cursor. An empty or blank
// transcript gives "".
func PostProcess(raw, before, after string, d Dictionary) string {
	text := strings.TrimSpace(raw)
	if text == "" {
		return ""
	}
	if len(d.Replacements) > 0 {
		text, _ = textrules.NewReplacer(d.Replacements).Apply(text)
	}
	text = respell(text, d.Words)
	text = strings.TrimSpace(textrules.SpokenPunctuation(text))
	if text == "" {
		return ""
	}
	startsSentence := textrules.SentenceStart(before)
	switch {
	case startsSentence && d.Autocap:
		text = textrules.CapitalizeFirst(text)
	case !startsSentence:
		text = lowerFirstWord(text, d.Spelling)
	}
	next, _ := utf8.DecodeRuneInString(strings.TrimLeft(after, " \t"))
	if (wordRune(next) || textrules.AttachesLeft(next)) && strings.HasSuffix(text, ".") && !strings.HasSuffix(text, "..") {
		// The recognizer ends every utterance with a period; before more
		// text or a mark it would split the sentence or double the mark.
		text = strings.TrimSuffix(text, ".")
	}
	return spaced(text, before, after)
}

// spaced adds a space before text when it would touch the word before the
// cursor, and after it when it would touch the word after.
func spaced(text, before, after string) string {
	first, _ := utf8.DecodeRuneInString(text)
	last, _ := utf8.DecodeLastRuneInString(text)
	prev, _ := utf8.DecodeLastRuneInString(before)
	next, _ := utf8.DecodeRuneInString(after)
	if before != "" && !unicode.IsSpace(prev) && !opens(prev) && !textrules.AttachesLeft(first) && !unicode.IsSpace(first) {
		text = " " + text
	}
	if after != "" && !unicode.IsSpace(next) && !textrules.AttachesLeft(next) && !unicode.IsSpace(last) && !opens(last) {
		text += " "
	}
	return text
}

// opens reports whether r is a mark the next word joins with no space.
func opens(r rune) bool {
	switch r {
	case '(', '[', '{', '"', '“', '‘', '¿', '¡', '/', '#', '@':
		return true
	}
	return false
}

func wordRune(r rune) bool { return unicode.IsLetter(r) || unicode.IsDigit(r) }

var firstWord = regexp.MustCompile(`^\p{L}[\p{L}'’]*`)

// lowerFirstWord writes the first word in lower case when the recognizer
// capitalized it only because the utterance started there: the model
// spells it in lower case ("The" → "the", but "I", "Paris" and "NASA" stay).
func lowerFirstWord(text string, spelling func(string) (string, bool)) string {
	if spelling == nil {
		return text
	}
	loc := firstWord.FindStringIndex(text)
	if loc == nil {
		return text
	}
	w := text[:loc[1]]
	r, size := utf8.DecodeRuneInString(w)
	if !unicode.IsUpper(r) || strings.ToLower(w[size:]) != w[size:] {
		return text // already lower case, or more capitals than the first (an acronym, "McDonald")
	}
	s, ok := spelling(w)
	if !ok || s != strings.ToLower(w) {
		return text
	}
	return s + text[loc[1]:]
}

// maxRespellWords is the most spoken words one personal word may span
// ("git hub" → "GitHub").
const maxRespellWords = 3

var spokenToken = regexp.MustCompile(`[\p{L}\p{N}'’]+`)

// respell writes personal words in the user's spelling where the
// transcript has the same letters ("sayso" → "Sayso"). A word with a
// capital inside or a digit ("GitHub", "K8s") also replaces up to three
// spoken words joined by spaces or a hyphen ("git hub"); other words do
// not, so "I say so" stays when "Sayso" is a personal word.
func respell(text string, words []string) string {
	type personal struct {
		spelling string
		compound bool
	}
	byKey := map[string]personal{}
	for _, w := range words {
		if k := letterKey(w); len(k) >= 2 {
			byKey[k] = personal{w, compound(w)}
		}
	}
	if len(byKey) == 0 {
		return text
	}
	toks := spokenToken.FindAllStringIndex(text, -1)
	var b strings.Builder
	last := 0
	for i := 0; i < len(toks); {
		matched := 0
		var spelled string
		for n := min(maxRespellWords, len(toks)-i); n >= 1 && matched == 0; n-- {
			if !joinedBySpaces(text, toks[i:i+n]) {
				continue
			}
			var key strings.Builder
			for _, t := range toks[i : i+n] {
				key.WriteString(letterKey(text[t[0]:t[1]]))
			}
			if w, ok := byKey[key.String()]; ok && (n == 1 || w.compound) {
				matched, spelled = n, w.spelling
			}
		}
		if matched == 0 {
			i++
			continue
		}
		start, end := toks[i][0], toks[i+matched-1][1]
		b.WriteString(text[last:start])
		b.WriteString(spelled)
		last = end
		i += matched
	}
	b.WriteString(text[last:])
	return b.String()
}

// compound reports whether w has a capital after its first letter or a
// digit: a name spoken as several words.
func compound(w string) bool {
	for i, r := range w {
		if unicode.IsDigit(r) || (i > 0 && unicode.IsUpper(r)) {
			return true
		}
	}
	return false
}

// joinedBySpaces reports whether the tokens are separated only by single
// runs of spaces or one hyphen.
func joinedBySpaces(text string, toks [][]int) bool {
	for j := 1; j < len(toks); j++ {
		gap := text[toks[j-1][1]:toks[j][0]]
		if gap != "-" && strings.Trim(gap, " ") != "" {
			return false
		}
	}
	return true
}

// letterKey is a word's letters and digits in lower case.
func letterKey(s string) string {
	var b strings.Builder
	for _, r := range s {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			b.WriteRune(unicode.ToLower(r))
		}
	}
	return b.String()
}
