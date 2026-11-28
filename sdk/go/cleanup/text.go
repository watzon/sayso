package cleanup

// Text rules of the app (mobile/core/textrules) that the cleanup checks use.

import (
	"regexp"
	"sync"
	"unicode"
	"unicode/utf8"
)

var (
	regexpCacheMu sync.Mutex
	regexpCache   = map[string]*regexp.Regexp{}
)

func quoteForRegexp(s string) string { return regexp.QuoteMeta(s) }

// isWord is Rust regex's Unicode \w: Alphabetic, marks, decimal digits,
// connector punctuation, and the joiners.
func isWord(r rune) bool {
	return unicode.IsLetter(r) || unicode.IsMark(r) || unicode.Is(unicode.Nd, r) || unicode.Is(unicode.Nl, r) ||
		unicode.Is(unicode.Pc, r) || unicode.Is(unicode.Other_Alphabetic, r) || r == 0x200C || r == 0x200D
}

// wordBefore / wordAfter: is there a word character just before / at i.
func wordBefore(s string, i int) bool {
	if i <= 0 {
		return false
	}
	r, _ := utf8.DecodeLastRuneInString(s[:i])
	return isWord(r)
}

func wordAfter(s string, i int) bool {
	if i >= len(s) {
		return false
	}
	r, _ := utf8.DecodeRuneInString(s[i:])
	return isWord(r)
}

// findAccepted is the leftmost-first matches of re in s that accept takes,
// as a Rust regex with the same pattern plus the checks would find them. A
// refused match restarts the search one character after its start.
func findAccepted(re *regexp.Regexp, s string, accept func(loc []int) bool) [][]int {
	var out [][]int
	pos := 0
	for pos <= len(s) {
		loc := re.FindStringSubmatchIndex(s[pos:])
		if loc == nil {
			break
		}
		for i := range loc {
			if loc[i] >= 0 {
				loc[i] += pos
			}
		}
		if accept == nil || accept(loc) {
			out = append(out, loc)
			if loc[1] > loc[0] {
				pos = loc[1]
				continue
			}
		}
		if loc[0] >= len(s) {
			break
		}
		_, size := utf8.DecodeRuneInString(s[loc[0]:])
		pos = loc[0] + size
	}
	return out
}

// mustCompileCached compiles a pattern built from quoted text, once.
func mustCompileCached(pattern string) *regexp.Regexp {
	regexpCacheMu.Lock()
	defer regexpCacheMu.Unlock()
	if re, ok := regexpCache[pattern]; ok {
		return re
	}
	re := regexp.MustCompile(pattern)
	if len(regexpCache) > 512 {
		regexpCache = map[string]*regexp.Regexp{}
	}
	regexpCache[pattern] = re
	return re
}
