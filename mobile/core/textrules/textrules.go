// Package textrules is the pure text rules of Sayso: dictionary
// replacements, spoken punctuation, spelled words, the edit distance used to
// find dictionary words, and the casing and punctuation helpers of the
// keyboard. It imports only the standard library, so the desktop app and the
// mobile core share it.
package textrules

import (
	"regexp"
	"sort"
	"strings"
	"sync"
	"unicode"
	"unicode/utf8"
)

// Rust's regex classes are Unicode-aware; Go's \s and \b are ASCII. These
// helpers give the Unicode meaning where the Rust code relies on it.

// WSClass is Unicode White_Space, Rust's \s and char::is_whitespace.
const WSClass = `[\t\n\x{0B}\f\r \x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}]`

// IsWord is Rust regex's Unicode \w: Alphabetic, marks, decimal digits,
// connector punctuation, and the joiners.
func IsWord(r rune) bool {
	return unicode.IsLetter(r) || unicode.IsMark(r) || unicode.Is(unicode.Nd, r) || unicode.Is(unicode.Nl, r) ||
		unicode.Is(unicode.Pc, r) || unicode.Is(unicode.Other_Alphabetic, r) || r == 0x200C || r == 0x200D
}

// IsAlphabetic is Rust's char::is_alphabetic.
func IsAlphabetic(r rune) bool {
	return unicode.IsLetter(r) || unicode.Is(unicode.Nl, r) || unicode.Is(unicode.Other_Alphabetic, r)
}

// IsAlnum is Rust's char::is_alphanumeric.
func IsAlnum(r rune) bool { return IsAlphabetic(r) || unicode.IsNumber(r) }

// WordBefore / WordAfter: is there a word character just before / at i.
func WordBefore(s string, i int) bool {
	if i <= 0 {
		return false
	}
	r, _ := utf8.DecodeLastRuneInString(s[:i])
	return IsWord(r)
}

func WordAfter(s string, i int) bool {
	if i >= len(s) {
		return false
	}
	r, _ := utf8.DecodeRuneInString(s[i:])
	return IsWord(r)
}

// FindAccepted is the leftmost-first matches of re in s that accept takes,
// as a Rust regex with the same pattern plus the checks would find them. A
// refused match restarts the search one character after its start.
func FindAccepted(re *regexp.Regexp, s string, accept func(loc []int) bool) [][]int {
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

// ---- Dictionary replacements ----

// Rule is one dictionary replacement: what the speaker says and what to write.
type Rule struct {
	ID int64
	// From is what the speaker says ("git hub").
	From string
	// To is what to write; `\n` writes a line break.
	To            string
	CaseSensitive bool
}

// Applied is one rule that changed the text, and how many times.
type Applied struct {
	RuleID   int64
	From, To string
	Count    int
}

// Replacer holds compiled replacement rules. Build once per dictionary change.
type Replacer struct {
	rules []compiledRule
}

type compiledRule struct {
	rule               Rule
	re                 *regexp.Regexp
	needStart, needEnd bool
}

// NewReplacer compiles the rules: longer phrases first, so "new paragraph"
// wins over "new".
func NewReplacer(replacements []Rule) *Replacer {
	var sorted []Rule
	for _, r := range replacements {
		if strings.TrimSpace(r.From) != "" {
			sorted = append(sorted, r)
		}
	}
	sort.SliceStable(sorted, func(i, j int) bool {
		if len(sorted[i].From) != len(sorted[j].From) {
			return len(sorted[i].From) > len(sorted[j].From)
		}
		return sorted[i].ID < sorted[j].ID
	})
	rep := &Replacer{}
	for _, r := range sorted {
		words := strings.Fields(r.From)
		for i, w := range words {
			words[i] = regexp.QuoteMeta(w)
		}
		pattern := strings.Join(words, WSClass+"+")
		if !r.CaseSensitive {
			pattern = "(?i)" + pattern
		}
		re, err := regexp.Compile(pattern)
		if err != nil {
			continue
		}
		first, _ := utf8.DecodeRuneInString(strings.TrimLeftFunc(r.From, unicode.IsSpace))
		last, _ := utf8.DecodeLastRuneInString(strings.TrimRightFunc(r.From, unicode.IsSpace))
		rep.rules = append(rep.rules, compiledRule{rule: r, re: re, needStart: IsAlnum(first), needEnd: IsAlnum(last)})
	}
	return rep
}

// Apply runs every rule and returns the text and the rules that changed it.
func (r *Replacer) Apply(text string) (string, []Applied) {
	out := text
	applied := []Applied{}
	for _, c := range r.rules {
		matches := FindAccepted(c.re, out, func(loc []int) bool {
			// \b on a side of the phrase that starts or ends with a word character.
			return (!c.needStart || !WordBefore(out, loc[0])) && (!c.needEnd || !WordAfter(out, loc[1]))
		})
		if len(matches) == 0 {
			continue
		}
		to := strings.ReplaceAll(c.rule.To, `\n`, "\n")
		var b strings.Builder
		last := 0
		for _, m := range matches {
			b.WriteString(out[last:m[0]])
			b.WriteString(to)
			last = m[1]
		}
		b.WriteString(out[last:])
		out = b.String()
		applied = append(applied, Applied{RuleID: c.rule.ID, From: c.rule.From, To: c.rule.To, Count: len(matches)})
	}
	return tidySpacing(out), applied
}

// tidySpacing removes spaces that replacements leave around line breaks.
func tidySpacing(s string) string {
	lines := strings.Split(s, "\n")
	for i, l := range lines {
		lines[i] = strings.Trim(l, " ")
	}
	return strings.Join(lines, "\n")
}

// maxSpokenWords is the longest run of spoken words one dictionary word can be.
const maxSpokenWords = 4

// WordsHeard is the dictionary words that the transcript can contain: with
// the correct spelling, in separate words, or with a small error (at most
// one third of the characters).
func WordsHeard(words []string, transcript string) []string {
	var spoken [][]rune
	for _, part := range strings.FieldsFunc(transcript, func(c rune) bool { return !IsAlnum(c) }) {
		spoken = append(spoken, []rune(strings.ToLower(part)))
	}
	out := []string{}
	for _, word := range words {
		var key []rune
		for _, c := range strings.ToLower(word) {
			if IsAlnum(c) {
				key = append(key, c)
			}
		}
		if len(key) == 0 {
			continue
		}
		allowed := len(key) / 3
		found := false
		for start := 0; start < len(spoken) && !found; start++ {
			var run []rune
			for i := start; i < len(spoken) && i < start+maxSpokenWords; i++ {
				run = append(run, spoken[i]...)
				d := len(run) - len(key)
				if d < 0 {
					d = -d
				}
				if d <= allowed && EditDistance(run, key) <= allowed {
					found = true
					break
				}
			}
		}
		if found {
			out = append(out, word)
		}
	}
	return out
}

// EditDistance is the Levenshtein distance of two rune slices.
func EditDistance(a, b []rune) int {
	row := make([]int, len(b)+1)
	for i := range row {
		row[i] = i
	}
	for i, x := range a {
		diagonal := row[0]
		row[0] = i + 1
		for j, y := range b {
			change := diagonal
			if x != y {
				change++
			}
			diagonal = row[j+1]
			row[j+1] = min(change, row[j]+1, diagonal+1)
		}
	}
	return row[len(b)]
}

// ---- Spoken punctuation ----

var spokenCommand = regexp.MustCompile(`(?i)[ \t]*([,.;:!?]*)[ \t]*(new paragraph|new line|full stop|period|comma|question mark|exclamation mark|exclamation point|semicolon|colon)[,.;:!?]*[ \t]*`)

// SpokenPunctuation changes spoken commands ("comma", "new line") into marks
// and line breaks. A mark attaches to the word before it; the word after a
// sentence end or a line break starts with a capital letter.
func SpokenPunctuation(text string) string {
	var out strings.Builder
	last := 0
	space, capital := false, false
	pushText := func(part string, space, capital bool) {
		if part == "" {
			return
		}
		first, size := utf8.DecodeRuneInString(part)
		if space {
			out.WriteByte(' ')
		}
		if capital {
			out.WriteString(strings.ToUpper(string(first)))
		} else {
			out.WriteRune(first)
		}
		out.WriteString(part[size:])
	}
	matches := FindAccepted(spokenCommand, text, func(loc []int) bool {
		return !WordBefore(text, loc[4]) && !WordAfter(text, loc[5])
	})
	for _, m := range matches {
		pushText(text[last:m[0]], space, capital)
		last = m[1]
		var written string
		switch strings.ToLower(text[m[4]:m[5]]) {
		case "new paragraph":
			written = "\n\n"
		case "new line":
			written = "\n"
		case "comma":
			written = ","
		case "question mark":
			written = "?"
		case "exclamation mark", "exclamation point":
			written = "!"
		case "semicolon":
			written = ";"
		case "colon":
			written = ":"
		default:
			written = "."
		}
		lineBreak := strings.HasPrefix(written, "\n")
		if lineBreak {
			out.WriteString(text[m[2]:m[3]])
		}
		out.WriteString(written)
		space = !lineBreak
		capital = lineBreak || written == "." || written == "?" || written == "!"
	}
	pushText(text[last:], space, capital)
	return out.String()
}

// ---- Spelled words ----

// spelling is a cue and the letters after it. Rust's trailing `\b\.?` is the
// last group: a dot, the end, or a non-word character that is not part of
// the match.
var spelling = regexp.MustCompile(`(?i)[,;:]?` + WSClass + `*(that's|that’s|that is|spelled|spelt)[,:]?` + WSClass +
	`+(\p{L}(?:(?: |-|, |\. )\p{L})+)(?:(\.)|$|([^\p{L}\p{M}\p{Nd}\p{Nl}\p{Pc}\x{200C}\x{200D}]))`)

var spokenWord = regexp.MustCompile(`[\p{L}\p{N}'’]+`)

// maxReplacedWords is the most words before a cue one spelled word replaces.
const maxReplacedWords = 3

// Spelled joins the letters of each spelled word ("cats, that's K A T Z")
// and puts the word in place of the cue and the words it corrects.
func Spelled(text string) string {
	var out strings.Builder
	rest := text
	for {
		ms := FindAccepted(spelling, rest, func(loc []int) bool { return !WordBefore(rest, loc[2]) })
		if len(ms) == 0 {
			break
		}
		m := ms[0]
		end := m[1]
		if m[8] >= 0 { // the non-word character after the letters is not matched
			end = m[8]
		}
		var letters []rune
		for _, c := range rest[m[4]:m[5]] {
			if IsAlphabetic(c) {
				letters = append(letters, []rune(strings.ToLower(string(c)))...)
			}
		}
		cue := strings.ToLower(rest[m[2]:m[3]])
		if len(letters) < 3 && !strings.HasPrefix(cue, "spel") {
			out.WriteString(rest[:end])
			rest = rest[end:]
			continue
		}
		before := rest[:m[0]]
		start, found := correctedFrom(before, letters)
		kept := before
		if found {
			kept = before[:start]
		}
		out.WriteString(kept)
		if !found && kept != "" {
			out.WriteByte(' ')
		}
		capital := false
		if found {
			r, _ := utf8.DecodeRuneInString(before[start:])
			capital = unicode.IsUpper(r)
		}
		for i, c := range letters {
			if i == 0 && capital {
				out.WriteString(strings.ToUpper(string(c)))
			} else {
				out.WriteRune(c)
			}
		}
		if m[6] >= 0 {
			out.WriteByte('.')
		}
		rest = rest[end:]
	}
	out.WriteString(rest)
	return out.String()
}

// correctedFrom is where the words that letters corrects start in before.
func correctedFrom(before string, letters []rune) (int, bool) {
	words := spokenWord.FindAllStringIndex(before, -1)
	if len(words) == 0 || strings.TrimSpace(before[words[len(words)-1][1]:]) != "" {
		return 0, false
	}
	bestDist, bestStart, have := 0, 0, false
	var heard []rune
	for i := len(words) - 1; i >= 0 && i >= len(words)-maxReplacedWords; i-- {
		var key []rune
		for _, c := range before[words[i][0]:words[i][1]] {
			if IsAlnum(c) {
				key = append(key, []rune(strings.ToLower(string(c)))...)
			}
		}
		heard = append(key, heard...)
		d := EditDistance(heard, letters)
		if d*2 <= max(len(heard), len(letters)) && (!have || d < bestDist) {
			bestDist, bestStart, have = d, words[i][0], true
		}
	}
	return bestStart, have
}

var (
	regexpCacheMu sync.Mutex
	regexpCache   = map[string]*regexp.Regexp{}
)

// QuoteForRegexp quotes s for a pattern.
func QuoteForRegexp(s string) string { return regexp.QuoteMeta(s) }

// MustCompileCached compiles a pattern built from quoted text, once.
func MustCompileCached(pattern string) *regexp.Regexp {
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
