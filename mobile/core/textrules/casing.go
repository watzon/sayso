package textrules

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// SentenceStart reports whether a word typed after before starts a sentence:
// before is empty or blank, or its last non-space character ends a sentence
// (. ! ?) or is a line break.
func SentenceStart(before string) bool {
	for i := len(before); i > 0; {
		r, size := utf8.DecodeLastRuneInString(before[:i])
		i -= size
		switch {
		case r == '\n' || r == '\r':
			return true
		case unicode.IsSpace(r):
			continue
		case r == '"' || r == '\'' || r == ')' || r == '’' || r == '”':
			// A closing quote or bracket after the mark: look before it.
			continue
		default:
			return r == '.' || r == '!' || r == '?' || r == '…'
		}
	}
	return true
}

// AttachesLeft reports whether a mark joins the word before it with no space
// between: the keyboard removes a space that it put before such a mark.
func AttachesLeft(r rune) bool {
	switch r {
	case '.', ',', '!', '?', ';', ':', '…', ')', '%':
		return true
	}
	return false
}

// EndsSentence reports whether a mark ends a sentence.
func EndsSentence(r rune) bool { return r == '.' || r == '!' || r == '?' || r == '…' }

// CapitalizeFirst writes the first letter of s in upper case.
func CapitalizeFirst(s string) string {
	r, size := utf8.DecodeRuneInString(s)
	if size == 0 || unicode.IsUpper(r) {
		return s
	}
	return string(unicode.ToUpper(r)) + s[size:]
}

// MatchCase gives word the case of typed: all capitals when typed has two or
// more letters and all of them are capitals, a capital first letter when
// typed starts with one, and otherwise word as it is (the lexicon spelling,
// so "london" stays "London" and "i" stays "I").
func MatchCase(typed, word string) string {
	letters, upper := 0, 0
	for _, r := range typed {
		if unicode.IsLetter(r) {
			letters++
			if unicode.IsUpper(r) {
				upper++
			}
		}
	}
	if letters >= 2 && upper == letters {
		return strings.ToUpper(word)
	}
	first, _ := utf8.DecodeRuneInString(typed)
	if unicode.IsUpper(first) {
		return CapitalizeFirst(word)
	}
	return word
}
