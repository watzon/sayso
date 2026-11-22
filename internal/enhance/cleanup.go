package enhance

import (
	"fmt"
	"slices"
	"sort"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/abadojack/whatlanggo"
)

// Cleanup models: small local models that clean up text with fixed settings
// and read no prompt (a port of sayso-cleanup-models; docs/cleanup-models.md).

// PromptFormat is how a model wants its input.
type PromptFormat int

const (
	PromptS1Chat   PromptFormat = iota // ChatML with a fixed system prompt and a control line
	PromptKeyValue                     // `key: value` lines, then `out:`
)

// Tone, Layout, and Format are the settings of a cleanup.
type (
	Tone   int
	Layout int
	Format int
)

const (
	ToneFormal Tone = iota // the default
	ToneCasual
	ToneVeryCasual
)

const (
	LayoutParagraphs Layout = iota
	LayoutList
)

const (
	FormatPlain Format = iota
	FormatEmail
)

func (t Tone) Code() string {
	return [...]string{"formal", "casual", "very-casual"}[t]
}
func (l Layout) Code() string { return [...]string{"paragraphs", "list"}[l] }
func (f Format) Code() string { return [...]string{"plain", "email"}[f] }

// CleanupModelInfo is one cleanup model Sayso can download and run.
type CleanupModelInfo struct {
	ID, Name, Publisher string
	Repo, Revision      string
	File                string
	SizeBytes           uint64
	SHA256              string
	LicenseFiles        []string
	Languages           []string
	Tones               bool
	Layouts             []Layout
	Formats             []Format
	Context             bool // reads the app and the site
	Prompt              PromptFormat
}

const (
	// NCtx is the context size the runtime loads a model with.
	NCtx uint32 = 2048
	// MaxPartTokens is the most text tokens in one part.
	MaxPartTokens uint32 = 700
)

const s1System = "You are a text normalizer for speech-to-text transcripts. The input begins with a control line " +
	"specifying the styling, structure, and context settings; clean the transcript to match those settings and output only " +
	"the cleaned text."

var cleanupCatalog = []CleanupModelInfo{
	{
		ID: "s1-mini", Name: "S1-mini", Publisher: "Superwhisper",
		Repo: "superwhisper/s1-mini-GGUF", Revision: "34add00a48a2e5d24e5a4ee5405a99620a3a240c",
		File: "s1-mini-q4_k_m.gguf", SizeBytes: 484_219_808,
		SHA256:       "3b41ebe2502cbd03e811d5d16b022f5ab551eda58d62597d152f89535003c634",
		LicenseFiles: []string{"LICENSE", "NOTICE"}, Languages: []string{"en"},
		Layouts: []Layout{LayoutParagraphs, LayoutList}, Formats: []Format{FormatPlain, FormatEmail},
		Prompt: PromptS1Chat,
	},
	{
		ID: "spokenly-cleanup-1", Name: "Cleanup 1", Publisher: "Spokenly",
		Repo: "spokenly/cleanup-1-gguf", Revision: "79936c95e5c0ea9162ae7a435e64cb3127020e36",
		File: "cleanup-1-q8_0.gguf", SizeBytes: 639_442_784,
		SHA256:       "66509e0fe1578d2ac4b0773c79a6c83e8d6f218618dfe11e4532beacd999b6e8",
		LicenseFiles: []string{"LICENSE", "NOTICE"}, Languages: []string{"en", "de", "es", "fr", "zh", "ru", "ja"},
		Tones: true, Layouts: []Layout{LayoutParagraphs}, Formats: []Format{FormatPlain},
		Context: true, Prompt: PromptKeyValue,
	},
}

// CleanupCatalog is every cleanup model.
func CleanupCatalog() []CleanupModelInfo { return cleanupCatalog }

// FindCleanupModel is the model with a catalog id.
func FindCleanupModel(id string) (*CleanupModelInfo, bool) {
	for i := range cleanupCatalog {
		if cleanupCatalog[i].ID == id {
			return &cleanupCatalog[i], true
		}
	}
	return nil, false
}

// CleanupSettings is the [cleanup] table of a style file. The values are
// strings, so a value from a newer Sayso does not break the file.
type CleanupSettings struct {
	Tone, Layout, Format *string
}

// Settings are cleanup settings with known values.
type Settings struct {
	Tone   Tone
	Layout Layout
	Format Format
}

// NewCleanupSettings writes only the values that are not the default.
func NewCleanupSettings(tone Tone, layout Layout, format Format) CleanupSettings {
	set := func(code, def string) *string {
		if code == def {
			return nil
		}
		return &code
	}
	return CleanupSettings{Tone: set(tone.Code(), ToneFormal.Code()), Layout: set(layout.Code(), LayoutParagraphs.Code()), Format: set(format.Code(), FormatPlain.Code())}
}

// Known is the settings, or false when a value is not known to this version.
func (c CleanupSettings) Known() (Settings, bool) {
	var s Settings
	switch deref(c.Tone) {
	case "", "formal":
		if c.Tone != nil && *c.Tone == "" {
			return s, false
		}
		s.Tone = ToneFormal
	case "casual":
		s.Tone = ToneCasual
	case "very-casual":
		s.Tone = ToneVeryCasual
	default:
		return s, false
	}
	switch deref(c.Layout) {
	case "", "paragraphs":
		if c.Layout != nil && *c.Layout == "" {
			return s, false
		}
		s.Layout = LayoutParagraphs
	case "list":
		s.Layout = LayoutList
	default:
		return s, false
	}
	switch deref(c.Format) {
	case "", "plain":
		if c.Format != nil && *c.Format == "" {
			return s, false
		}
		s.Format = FormatPlain
	case "email":
		s.Format = FormatEmail
	default:
		return s, false
	}
	return s, true
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

// Runs is true when the model can run a style with these settings.
func (m *CleanupModelInfo) Runs(c CleanupSettings) bool {
	s, ok := c.Known()
	return ok && slices.Contains(m.Layouts, s.Layout) && slices.Contains(m.Formats, s.Format)
}

// SupportsLanguage is true for a language code the model knows.
func (m *CleanupModelInfo) SupportsLanguage(code string) bool { return slices.Contains(m.Languages, code) }

// FileURL is the URL of a file of the pinned revision.
func (m *CleanupModelInfo) FileURL(file string) string {
	return fmt.Sprintf("https://huggingface.co/%s/resolve/%s/%s", m.Repo, m.Revision, file)
}

// RawPrompt is one request to a cleanup model, for one part of the text.
type RawPrompt struct {
	Text      string
	End       []string
	MaxTokens uint32
}

// MaxAnswerTokens is the longest answer for a part of textTokens tokens.
func MaxAnswerTokens(textTokens uint32) uint32 { return textTokens*13/10 + 32 }

// Render writes the input of model for one part of the text.
func Render(model *CleanupModelInfo, text string, s Settings, app, site string, textTokens uint32) RawPrompt {
	maxTokens := MaxAnswerTokens(textTokens)
	if model.Prompt == PromptS1Chat {
		structure, context := "prose", "general"
		if s.Layout == LayoutList {
			structure = "lists"
		}
		if s.Format == FormatEmail {
			context = "email"
		}
		return RawPrompt{
			Text: "<|im_start|>system\n" + s1System + "<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: " + structure +
				"] [Context: " + context + "]\n" + text + "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
			End:       []string{"<|im_end|>"},
			MaxTokens: maxTokens,
		}
	}
	oneLine := func(v string) string { return strings.Join(strings.Fields(v), " ") }
	var lines []string
	if model.Context {
		if a := oneLine(app); a != "" {
			lines = append(lines, "app: "+a)
		}
		if st := oneLine(site); st != "" {
			lines = append(lines, "site: "+st)
		}
	}
	if model.Tones {
		lines = append(lines, "tone: "+[...]string{"formal", "casual", "very casual"}[s.Tone])
	}
	lines = append(lines, "text: "+text, "out:")
	return RawPrompt{Text: strings.Join(lines, "\n"), End: []string{"\ntext:", "\nout:"}, MaxTokens: maxTokens}
}

// PlanParts splits text into parts of at most maxTokens tokens. Every
// character is in exactly one part. A part ends after a sentence end if it
// can, else after a comma, else after a space, else after any character.
func PlanParts(text string, maxTokens uint32, count func(string) (uint32, error)) ([]string, error) {
	parts := []string{}
	rest := text
	for rest != "" {
		n, err := count(rest)
		if err != nil {
			return nil, err
		}
		if n <= maxTokens {
			parts = append(parts, rest)
			break
		}
		cut, err := bestCut(rest, maxTokens, count)
		if err != nil {
			return nil, err
		}
		parts = append(parts, rest[:cut])
		rest = rest[cut:]
	}
	return parts, nil
}

func bestCut(text string, maxTokens uint32, count func(string) (uint32, error)) (int, error) {
	levels := []func(rune) bool{
		func(c rune) bool { return strings.ContainsRune(".?!。？！\n", c) },
		func(c rune) bool { return c == ',' || c == '，' || c == '、' },
		unicode.IsSpace,
	}
	for _, isCut := range levels {
		cut, ok, err := longestFit(text, cutPoints(text, isCut), maxTokens, count)
		if err != nil || ok {
			return cut, err
		}
	}
	var every []int
	for i, c := range text {
		every = append(every, i+utf8.RuneLen(c))
	}
	cut, ok, err := longestFit(text, every, maxTokens, count)
	if err != nil {
		return 0, err
	}
	if !ok {
		cut = every[0]
	}
	return cut, nil
}

// cutPoints is the byte index after each cut character and the white space after it.
func cutPoints(text string, isCut func(rune) bool) []int {
	var cuts []int
	runes := []rune(text)
	offsets := make([]int, len(runes)+1)
	for i, o := 0, 0; i < len(runes); i++ {
		offsets[i] = o
		o += utf8.RuneLen(runes[i])
		offsets[i+1] = o
	}
	for i := 0; i < len(runes); i++ {
		if !isCut(runes[i]) {
			continue
		}
		end := offsets[i+1]
		for i+1 < len(runes) && unicode.IsSpace(runes[i+1]) {
			i++
			end = offsets[i+1]
		}
		if end < len(text) && (len(cuts) == 0 || cuts[len(cuts)-1] != end) {
			cuts = append(cuts, end)
		}
	}
	return cuts
}

func longestFit(text string, cuts []int, maxTokens uint32, count func(string) (uint32, error)) (int, bool, error) {
	lo, hi, best, ok := 0, len(cuts), 0, false
	for lo < hi {
		mid := (lo + hi) / 2
		n, err := count(text[:cuts[mid]])
		if err != nil {
			return 0, false, err
		}
		if n <= maxTokens {
			best, ok = cuts[mid], true
			lo = mid + 1
		} else {
			hi = mid
		}
	}
	return best, ok, nil
}

// JoinAnswers joins the answers of the parts: a part that ended with a new
// line keeps it between the answers; other parts get one space.
func JoinAnswers(parts, answers []string) string {
	var out strings.Builder
	for i, a := range answers {
		if a == "" {
			continue
		}
		if out.Len() > 0 {
			if strings.HasSuffix(strings.TrimRight(parts[i-1], " \t"), "\n") {
				out.WriteByte('\n')
			} else {
				out.WriteByte(' ')
			}
		}
		out.WriteString(a)
	}
	return out.String()
}

// CheckFailure is why an answer is not used.
type CheckFailure struct {
	Kind string // "cut", "empty", "too_short", "too_long"
	Stop string // Cut
}

func (c *CheckFailure) Error() string {
	switch c.Kind {
	case "cut":
		return fmt.Sprintf("the answer stopped early (%s)", stopDebug(c.Stop))
	case "empty":
		return "the answer is empty"
	case "too_short":
		return "the answer is much shorter than the text"
	}
	return "the answer is much longer than the text"
}

// stopDebug is Rust's Debug of a Stop.
func stopDebug(s string) string {
	if s == "" {
		return s
	}
	return strings.ToUpper(s[:1]) + s[1:]
}

var controlText = []string{"<|im_start|>", "<|im_end|>", "<|endoftext|>", "<think>", "</think>"}

// CheckAnswer checks and cleans the answer of a model for one part. Only an
// answer that stopped at its end marker is whole.
func CheckAnswer(input, stop, answer string) (string, error) {
	if stop != "end" {
		return "", &CheckFailure{Kind: "cut", Stop: stop}
	}
	text := answer
	for _, c := range controlText {
		text = strings.ReplaceAll(text, c, "")
	}
	text = strings.TrimSpace(text)
	if text == "" {
		if IsFillerOnly(input) {
			return text, nil
		}
		return "", &CheckFailure{Kind: "empty"}
	}
	before, after := contentUnits(input), contentUnits(text)
	if before >= 10 && after*4 < before {
		return "", &CheckFailure{Kind: "too_short"}
	}
	if utf8.RuneCountInString(text) > utf8.RuneCountInString(input)*3+200 {
		return "", &CheckFailure{Kind: "too_long"}
	}
	return text, nil
}

var numberWords = []string{"zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
	"twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty", "thirty", "forty",
	"fifty", "sixty", "seventy", "eighty", "ninety", "hundred", "thousand", "million", "billion", "point"}

func isCJK(c rune) bool {
	return c >= 0x3040 && c <= 0x30FF || c >= 0x3400 && c <= 0x4DBF || c >= 0x4E00 && c <= 0x9FFF || c >= 0xAC00 && c <= 0xD7AF || c >= 0xF900 && c <= 0xFAFF
}

// contentUnits counts words, CJK characters, and runs of numbers.
func contentUnits(text string) int {
	units, inNumber := 0, false
	var word strings.Builder
	flush := func() {
		if word.Len() == 0 {
			return
		}
		lower := strings.ToLower(word.String())
		number := strings.ContainsAny(lower, "0123456789") || slices.Contains(numberWords, lower)
		joins := inNumber && lower == "and"
		if number || joins {
			if !inNumber {
				units++
			}
			inNumber = true
		} else {
			units++
			inNumber = false
		}
		word.Reset()
	}
	for _, c := range text {
		switch {
		case isCJK(c):
			flush()
			units++
			inNumber = false
		case isAlnum(c):
			word.WriteRune(c)
		default:
			flush()
		}
	}
	flush()
	return units
}

var fillers = []string{"um", "umm", "uh", "uhm", "uhh", "er", "erm", "ah", "hmm", "hm", "mm", "mhm", "like", "so", "well", "you", "know",
	"äh", "ähm", "öh", "halt", "also", "euh", "bah", "ben", "hein", "eh", "em", "este", "pues", "bueno",
	"э", "эм", "ну", "вот", "嗯", "呃", "啊", "那个", "就是", "えー", "えっと", "あの", "あのー", "まあ"}

// IsFillerOnly is true when every word of text is a filler word.
func IsFillerOnly(text string) bool {
	var words []string
	for _, w := range strings.FieldsFunc(text, func(c rune) bool { return !isAlnum(c) && c != 'ー' }) {
		words = append(words, strings.ToLower(w))
	}
	if len(words) == 0 {
		return false
	}
	joined := strings.ReplaceAll(" "+strings.Join(words, " ")+" ", " you know ", " ")
	for _, w := range strings.Fields(joined) {
		if !slices.Contains(fillers, w) || w == "you" || w == "know" {
			return false
		}
	}
	return true
}

// RestoreSpelling puts back the spelling of terms that the model changed:
// a term that was in sent with its exact spelling (and in no other case) and
// is in answer with the same letters in another case, as a whole word.
func RestoreSpelling(sent, answer string, terms []string) string {
	var list []string
	for _, t := range terms {
		if t = strings.TrimSpace(t); t != "" {
			list = append(list, t)
		}
	}
	sort.SliceStable(list, func(i, j int) bool { return utf8.RuneCountInString(list[i]) > utf8.RuneCountInString(list[j]) })
	list = slices.Compact(list)
	out := answer
	type span struct{ start, end int }
	var fixed []span
	for _, term := range list {
		exact := wholeWord(term, false)
		if len(exact.find(sent)) == 0 {
			continue
		}
		anyCase := wholeWord(term, true)
		other := false
		for _, m := range anyCase.find(sent) {
			if sent[m[0]:m[1]] != term {
				other = true
			}
		}
		if other {
			continue
		}
		var found []span
		for _, m := range anyCase.find(out) {
			s := span{m[0], m[1]}
			if out[s.start:s.end] == term || s.end-s.start != len(term) {
				continue
			}
			overlaps := false
			for _, f := range fixed {
				if f.start < s.end && s.start < f.end {
					overlaps = true
				}
			}
			if !overlaps {
				found = append(found, s)
			}
		}
		for _, s := range found {
			out = out[:s.start] + term + out[s.end:]
			fixed = append(fixed, s)
		}
		for _, m := range exact.find(out) {
			fixed = append(fixed, span{m[0], m[1]})
		}
	}
	return out
}

type wordPattern struct {
	term     string
	anyCase  bool
	needS, needE bool
}

func wholeWord(term string, anyCase bool) wordPattern {
	first, _ := utf8.DecodeRuneInString(term)
	last, _ := utf8.DecodeLastRuneInString(term)
	return wordPattern{term, anyCase, isAlnum(first), isAlnum(last)}
}

// find is the leftmost non-overlapping whole-word matches.
func (w wordPattern) find(s string) [][]int {
	pattern := quoteForRegexp(w.term)
	if w.anyCase {
		pattern = "(?i)" + pattern
	}
	re := mustCompileCached(pattern)
	return findAccepted(re, s, func(loc []int) bool {
		return (!w.needS || !wordBefore(s, loc[0])) && (!w.needE || !wordAfter(s, loc[1]))
	})
}

// AutoLanguage is the language setting that detects the language.
const AutoLanguage = "auto"

// DictationLanguage is the language of a dictation when Sayso knows it.
func DictationLanguage(setting string, finalModelLanguages []string, text string, detect func(string) (string, bool)) (string, bool) {
	if setting != AutoLanguage && setting != "" {
		return setting, true
	}
	if len(finalModelLanguages) == 1 {
		return finalModelLanguages[0], true
	}
	return detect(text)
}

// DetectLanguage is the language of text as a Sayso code, when the detector is sure.
func DetectLanguage(text string) (string, bool) {
	info := whatlanggo.Detect(text)
	if !info.IsReliable() {
		return "", false
	}
	code := info.Lang.Iso6393()
	if mapped, ok := map[string]string{"eng": "en", "deu": "de", "spa": "es", "fra": "fr", "cmn": "zh", "rus": "ru", "jpn": "ja",
		"ita": "it", "por": "pt", "nld": "nl", "kor": "ko"}[code]; ok {
		code = mapped
	}
	return code, code != ""
}

// CleanupRequest is one cleanup of a whole text with a cleanup model.
type CleanupRequest struct {
	Model     string
	Text      string
	Settings  Settings
	App, Site string
	Timeout   time.Duration // one deadline for all parts; zero means none
}

// CleanupResponse is the cleaned text.
type CleanupResponse struct {
	Text                 string
	Model                string
	ElapsedMs, LoadMs    uint64
	Parts                uint32
	TokensIn, TokensOut  uint32
}

// CleanError is why a cleanup gave no text.
type CleanError struct {
	Kind    string // "timeout", "not_configured", "runtime", "invalid_output"
	Ms      uint64
	Message string
}

func (e *CleanError) Error() string {
	switch e.Kind {
	case "timeout":
		return fmt.Sprintf("timed out after %d ms", e.Ms)
	case "not_configured":
		return "the cleanup model is not set up: " + e.Message
	case "invalid_output":
		return "the answer is not usable: " + e.Message
	}
	return e.Message
}

// ToEnhanceError maps a cleanup error to an enhancement error.
func (e *CleanError) ToEnhanceError() *Error {
	switch e.Kind {
	case "timeout":
		return &Error{Kind: ErrTimeout, Ms: e.Ms}
	case "not_configured":
		return notConfigured(e.Message)
	case "invalid_output":
		return invalidOutput(e.Message)
	}
	return &Error{Kind: ErrCLI, Message: e.Message}
}

// Cleaner runs a cleanup model. Calls block.
type Cleaner interface {
	Clean(CleanupRequest) (CleanupResponse, error)
}
