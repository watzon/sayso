package engine

import (
	"math"
	"slices"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/decoder"
	"github.com/watzon/sayso/mobile/core/learner"
	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/textrules"
)

// isLetter reports whether r is a letter the layout can type.
func (e *Engine) isLayoutLetter(r rune) (byte, bool) {
	if r >= 'A' && r <= 'Z' {
		r += 'a' - 'A'
	}
	if r >= 'a' && r <= 'z' && e.layout.LetterKey(byte(r)) >= 0 {
		return byte(r), true
	}
	return 0, false
}

// isWordChar reports whether r continues a word: letters, marks, digits.
func isWordChar(r rune) bool {
	return unicode.IsLetter(r) || unicode.IsMark(r) || unicode.IsDigit(r)
}

// TypeChar handles a character that was not a tapped letter: a letter from
// a hardware key or a long press, an apostrophe, a space or a mark that
// ends the word (and may autocorrect it), or any other character.
func (e *Engine) TypeChar(r rune) Action {
	e.mu.Lock()
	defer e.mu.Unlock()
	if c, ok := e.isLayoutLetter(r); ok {
		k, _ := e.synthTap(c)
		e.addLetter(c, unicode.IsUpper(r), k, true)
		return Action{}
	}
	if (r == '\'' || r == '’') && len(e.word) > 0 {
		e.typed = append(e.typed, string(r)...)
		e.before = append(e.before, string(r)...)
		e.dec.AddApostrophe()
		e.nsugs = 0
		return Action{}
	}
	if isWordChar(r) {
		// A letter the layout lacks ("é", a digit): keep the word as typed.
		if len(e.word) == 0 {
			e.lastValid = false
			e.confirmPending()
		}
		e.raw = true
		s := string(r)
		e.typed = append(e.typed, s...)
		e.word = append(e.word, tapRec{c: 0, key: -1, synth: true})
		e.appendBefore(s)
		e.nsugs = 0
		return Action{}
	}
	return e.commit(string(r))
}

// commit ends the word with sep.
func (e *Engine) commit(sep string) Action {
	sepRune, _ := utf8.DecodeRuneInString(sep)
	if len(e.word) == 0 {
		// No word: maybe swap a space the engine added for the mark.
		act := Action{}
		if e.autoSpace && textrules.AttachesLeft(sepRune) && len(e.before) > 0 && e.before[len(e.before)-1] == ' ' {
			e.before = e.before[:len(e.before)-1]
			act = Action{Kind: ActionReplace, DeleteChars: 1, DeleteUTF16: 1, Text: sep + " "}
			e.appendBefore(sep + " ")
			e.autoSpace = true
		} else {
			e.appendBefore(sep)
			e.autoSpace = false
		}
		e.lastValid = false
		e.refreshContext()
		return act
	}

	typed := string(e.typed)
	output, corrected := e.decideOutput(typed)
	act := Action{}
	if output != typed {
		act = replaceAction(typed, output+sep)
		act.Corrected = corrected
	}
	e.finishCommit(typed, output, sep, corrected)
	return act
}

func replaceAction(old, text string) Action {
	return Action{Kind: ActionReplace, DeleteChars: utf8.RuneCountInString(old), DeleteUTF16: utf16Len(old), Text: text}
}

func utf16Len(s string) int {
	n := 0
	for _, r := range s {
		if r >= 0x10000 {
			n += 2
		} else {
			n++
		}
	}
	return n
}

// decideOutput is what the typed word becomes on commit, and whether that
// is a correction the user may undo.
func (e *Engine) decideOutput(typed string) (string, bool) {
	if e.noFix {
		return typed, false // reverted: commit it as typed, replacements included
	}
	if r, ok := e.learn.Replacement(typed); ok {
		return strings.ReplaceAll(r.To, `\n`, "\n"), true
	}
	if e.raw || e.correctionsOff() || e.dec.Overflow() {
		return typed, false
	}
	if t, ok := e.learn.Promoted(typed); ok && !e.learn.Suppressed(typed, t) {
		return textrules.MatchCase(typed, t), true
	}
	dec := e.dec.Decide(&e.bonus)
	e.lastDecision = dec
	if dec.Correct {
		w := e.candText(dec.Cand)
		if !e.learn.Suppressed(typed, w) {
			return textrules.MatchCase(typed, w), true
		}
	}
	// The word as typed, in the lexicon's case: "i" → "I", "london" → "London".
	if dec.Literal.IsWord && isLower(typed) {
		m := e.srcModel(dec.Literal.Src)
		if m.Flags(dec.Literal.Word)&(lexicon.FlagProper|lexicon.FlagAllCaps) != 0 {
			w := string(m.Word(dec.Literal.Word))
			if w != typed && !e.learn.Suppressed(typed, w) {
				return w, true
			}
		}
	}
	return typed, false
}

func isLower(s string) bool {
	for _, r := range s {
		if unicode.IsUpper(r) {
			return false
		}
	}
	return true
}

func (e *Engine) srcModel(src uint8) *lexicon.Model {
	if src == decoder.SrcPersonal && e.dec.Personal() != nil {
		return e.dec.Personal()
	}
	return e.model
}

// candText is a candidate's spelling (two words for a split).
func (e *Engine) candText(c decoder.Cand) string {
	if c.Word == lexicon.NoWord {
		return string(e.typed)
	}
	m := e.srcModel(c.Src)
	s := string(m.Word(c.Word))
	if c.Word2 != lexicon.NoWord {
		s += " " + string(e.model.Word(c.Word2))
	}
	return s
}

// finishCommit updates the mirror, the undo state, and the learner.
func (e *Engine) finishCommit(typed, output, sep string, corrected bool) {
	if strings.HasSuffix(string(e.before), typed) {
		e.before = e.before[:len(e.before)-len(typed)]
	} else {
		e.before = e.before[:0] // the mirror lost the word's start (a very long word): restart it
	}
	e.appendBefore(output + sep)
	e.autoSpace = false

	taps := append([]tapRec(nil), e.word...)
	c := commit{typed: typed, output: output, sep: sep, corrected: corrected, taps: taps}

	// A reopened word that came out different is a manual fix (or a revert).
	if e.reopenedOK && e.edited && !strings.EqualFold(e.reopened, output) {
		if prev, ok := e.findHistory(e.reopened); ok && prev.corrected && strings.EqualFold(prev.typed, output) {
			e.learn.Reverted(prev.typed, prev.output)
		} else {
			from := e.reopened
			if ok && prev.corrected {
				from = prev.typed
			}
			e.learn.ManualFix(from, output)
		}
	}
	e.reopenedOK, e.edited = false, false

	e.learnFromCommit(&c)
	e.last, e.lastValid = c, true
	e.history[e.histN%historySize] = c
	e.histN++
	e.resetWord()
	e.refreshContext()
}

func (e *Engine) findHistory(output string) (commit, bool) {
	for i := 1; i <= historySize && i <= e.histN; i++ {
		c := e.history[(e.histN-i)%historySize]
		if strings.EqualFold(c.output, output) {
			return c, true
		}
	}
	return commit{}, false
}

// learnFromCommit records bigrams and kept words, and holds the word's taps
// until the next word confirms it was not undone.
func (e *Engine) learnFromCommit(c *commit) {
	if e.learn.Privacy() {
		return
	}
	prev := e.prevWord()
	words := strings.Fields(c.output)
	for _, w := range words {
		w = strings.Trim(w, ".,!?;:\"()")
		if prev != "" && w != "" {
			e.learn.Committed(prev, w)
		}
		prev = w
	}
	if !c.corrected {
		if _, ok := e.model.Find([]byte(c.typed), true); !ok && e.personalLookup(c.typed) == lexicon.NoWord && plausibleWord(c.typed) {
			e.learn.Kept(c.typed)
		}
	}
	e.pending = e.samplesFor(c)
	e.maybeRefreshPersonal()
}

// plausibleWord: only letters and inner apostrophes, two letters or more.
func plausibleWord(s string) bool {
	n := 0
	for i, r := range s {
		switch {
		case unicode.IsLetter(r):
			n++
		case (r == '\'' || r == '’') && i > 0:
		default:
			return false
		}
	}
	return n >= 2
}

// samplesFor is the confirmed-tap samples of a commit: only when the word
// is one known word with one tap per letter, each near its key.
func (e *Engine) samplesFor(c *commit) []learner.Sample {
	if strings.ContainsRune(c.output, ' ') {
		return nil
	}
	var kb [lexicon.MaxKeyLen * 2]byte
	key, ok := lexicon.AppendKey(kb[:0], []byte(c.output))
	if !ok || len(key) != len(c.taps) {
		return nil
	}
	if _, known := e.model.Find([]byte(c.output), true); !known && e.personalLookup(c.output) == lexicon.NoWord {
		return nil
	}
	pitch := e.layout.Pitch()
	out := make([]learner.Sample, 0, len(c.taps))
	for i, t := range c.taps {
		if t.synth {
			return nil
		}
		k := e.layout.LetterKey(key[i])
		if k < 0 || t.dist.Cost[key[i]-'a'] > 6 {
			return nil
		}
		cx, cy := e.layout.Center(k)
		out = append(out, learner.Sample{Key: k, Row: e.layout.Row(k), DX: (t.dist.X - cx) / pitch, DY: (t.dist.Y - cy) / pitch})
	}
	return out
}

// confirmPending teaches the offsets from the last word, now that a new
// word started without undoing it.
func (e *Engine) confirmPending() {
	if len(e.pending) == 0 {
		return
	}
	e.learn.Confirm(e.pending)
	e.pending = nil
	e.applyOffsets()
}

// ---- Backspace and undo ----

// Backspace handles the delete key. Right after an autocorrection it undoes
// it; otherwise the app deletes one character (ActionNone) and the engine
// follows.
func (e *Engine) Backspace() Action {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(e.word) == 0 && e.lastValid && e.last.corrected {
		return e.revert()
	}
	e.lastValid = false
	e.pending = nil // the last word is being edited: do not learn from it
	if len(e.word) > 0 {
		_, size := utf8.DecodeLastRune(e.typed)
		removed := e.typed[len(e.typed)-size:]
		e.typed = e.typed[:len(e.typed)-size]
		if len(e.before) >= size {
			e.before = e.before[:len(e.before)-size]
		}
		if removed[0] == '\'' || (size == 3 && string(removed) == "’") {
			e.dec.RemoveLast()
		} else {
			e.word = e.word[:len(e.word)-1]
			if !e.raw {
				e.dec.RemoveLast()
			}
		}
		e.edited = true
		e.nsugs = 0
		if len(e.word) == 0 {
			e.resetWord()
			e.refreshContext()
		}
		return Action{}
	}
	if len(e.before) > 0 {
		_, size := utf8.DecodeLastRune(e.before)
		e.before = e.before[:len(e.before)-size]
	}
	e.autoSpace = false
	e.reopenTrailingWord(true)
	e.refreshContext()
	return Action{}
}

// RevertLast undoes the last autocorrection while nothing was typed after
// it (the same as Backspace there). Otherwise it does nothing.
func (e *Engine) RevertLast() Action {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(e.word) == 0 && e.lastValid && e.last.corrected {
		return e.revert()
	}
	return Action{}
}

// revert puts back what was typed before the last correction, without its
// separator, and reopens it as the word being typed; committing it again
// keeps it as typed.
func (e *Engine) revert() Action {
	c := e.last
	e.lastValid = false
	e.pending = nil
	written := c.output + c.sep
	if len(e.before) >= len(written) {
		e.before = e.before[:len(e.before)-len(written)]
	}
	act := replaceAction(written, c.typed)
	e.learn.Reverted(c.typed, c.output)
	e.resetWord()
	ti := 0
	for _, r := range c.typed {
		if r == '\'' || r == '’' {
			e.dec.AddApostrophe()
			continue
		}
		if ti >= len(c.taps) {
			break
		}
		t := c.taps[ti]
		ti++
		e.word = append(e.word, t)
		if t.c != 0 && !e.raw {
			e.dec.AddTap(&t.dist, t.c)
		} else {
			e.raw = true
		}
	}
	e.typed = append(e.typed[:0], c.typed...)
	e.appendBefore(c.typed)
	e.noFix = true
	e.maybeRefreshPersonal()
	e.refreshContextBeforeWord()
	return act
}

// reopenTrailingWord makes a word at the end of the text the word being
// typed again (taps at key centres), so it can be corrected or completed.
// fromBackspace marks it as a committed word the user is editing.
func (e *Engine) reopenTrailingWord(fromBackspace bool) {
	if len(e.word) > 0 {
		return
	}
	i := len(e.before)
	for i > 0 {
		r, size := utf8.DecodeLastRune(e.before[:i])
		if !isWordChar(r) && r != '\'' && r != '’' {
			break
		}
		i -= size
	}
	w := string(e.before[i:])
	if w == "" || len(w) > 64 {
		return
	}
	e.before = e.before[:i]
	for _, r := range w {
		if c, ok := e.isLayoutLetter(r); ok {
			k, _ := e.synthTap(c)
			e.addLetter(c, unicode.IsUpper(r), k, true)
		} else if r == '\'' || r == '’' {
			s := string(r)
			e.typed = append(e.typed, s...)
			e.before = append(e.before, s...)
			e.dec.AddApostrophe()
		} else {
			e.raw = true
			e.typed = append(e.typed, string(r)...)
			e.word = append(e.word, tapRec{key: -1, synth: true})
			e.before = append(e.before, string(r)...)
		}
	}
	if fromBackspace {
		// The full committed word is the text plus nothing: the separator was
		// what got deleted. Edits start from here.
		e.reopened, e.reopenedOK, e.edited = w, true, false
	}
}

// ---- Suggestions ----

// Suggestion is one entry of the strip, ready to show.
type Suggestion struct {
	Text  string
	Score float32
	Flags uint8 // decoder.Sug* bits
}

// Suggestions returns up to n suggestions, best first: before any tap, next
// words; while typing, what a commit would write first. It allocates the
// strings; the scoring does not.
func (e *Engine) Suggestions(n int) []Suggestion {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.suggestions(n)
}

func (e *Engine) suggestions(n int) []Suggestion {
	if e.flags&FlagNoSuggest != 0 || e.raw || (len(e.word) == 0 && !e.predictions) {
		e.nsugs = 0
		return nil
	}
	n = min(n, maxSuggestions)
	e.nsugs = e.dec.Suggest(&e.bonus, e.sugs[:n])
	if e.correctionsOff() {
		for i := range e.sugs[:e.nsugs] {
			e.sugs[i].Flags &^= decoder.SugAutocorrect // a commit will not write it
		}
	}
	out := make([]Suggestion, 0, e.nsugs)
	for _, s := range e.sugs[:e.nsugs] {
		out = append(out, Suggestion{Text: e.sugText(s), Score: s.Total, Flags: s.Flags})
	}
	return out
}

func (e *Engine) sugText(s decoder.Suggestion) string {
	if s.Flags&decoder.SugLiteral != 0 && s.Flags&decoder.SugAutocorrect == 0 {
		return string(e.typed)
	}
	t := e.candText(s.Cand)
	if s.Flags&decoder.SugPrediction != 0 {
		if e.flags&FlagAutocap != 0 && textrules.SentenceStart(string(e.before)) {
			return textrules.CapitalizeFirst(t)
		}
		return t
	}
	return textrules.MatchCase(string(e.typed), t)
}

// Pick writes suggestion i of the last Suggestions call, followed by a
// space.
func (e *Engine) Pick(i int) Action {
	e.mu.Lock()
	defer e.mu.Unlock()
	if i < 0 || i >= e.nsugs {
		return Action{}
	}
	s := e.sugs[i]
	text := e.sugText(s)
	if len(e.word) == 0 {
		// A prediction: insert it, with a space before when needed.
		pre := ""
		if len(e.before) > 0 {
			if r, _ := utf8.DecodeLastRune(e.before); !unicode.IsSpace(r) {
				pre = " "
			}
		}
		e.confirmPending()
		e.appendBefore(pre + text + " ")
		if !e.learn.Privacy() {
			if prev := e.prevWordBefore(len(e.before) - len(text) - 1); prev != "" {
				e.learn.Committed(prev, text)
			}
		}
		e.autoSpace = true
		e.lastValid = false
		e.refreshContext()
		return Action{Kind: ActionReplace, Text: pre + text + " "}
	}
	typed := string(e.typed)
	literal := s.Flags&decoder.SugLiteral != 0 && s.Flags&decoder.SugAutocorrect == 0
	if !literal && s.Flags&decoder.SugAutocorrect == 0 && !strings.EqualFold(typed, text) {
		e.learn.ManualFix(typed, text) // a deliberate choice over the typed word
	}
	e.noFix = true
	act := replaceAction(typed, text+" ")
	e.finishCommit(typed, text, " ", !literal && text != typed)
	e.autoSpace = true
	return act
}

// ---- Context ----

// lastWords finds the two words before position end of the mirror, as
// main-model IDs (BOS at a sentence start, NoWord when unknown), and the
// last one's text.
func (e *Engine) lastWords(end int) (w1, w2 lexicon.WordID, prev string) {
	b := e.before[:end]
	ids := [2]lexicon.WordID{lexicon.NoWord, lexicon.NoWord}
	found := 0
	i := len(b)
	for found < 2 {
		// Skip spaces and marks that do not end a sentence.
		for i > 0 {
			r, size := utf8.DecodeLastRune(b[:i])
			if isWordChar(r) {
				break
			}
			if textrules.EndsSentence(r) || r == '\n' {
				ids[1-found] = e.model.BOS()
				return ids[0], ids[1], prev
			}
			i -= size
		}
		if i == 0 {
			ids[1-found] = e.model.BOS()
			return ids[0], ids[1], prev
		}
		j := i
		for j > 0 {
			r, size := utf8.DecodeLastRune(b[:j])
			if !isWordChar(r) && r != '\'' && r != '’' {
				break
			}
			j -= size
		}
		word := b[j:i]
		if found == 0 {
			prev = string(word)
		}
		id, ok := e.model.Find(word, true)
		if !ok {
			// An unknown word: no context beyond it.
			if found == 0 {
				return lexicon.NoWord, lexicon.NoWord, prev
			}
			return lexicon.NoWord, ids[1], prev
		}
		ids[1-found] = id
		found++
		i = j
	}
	return ids[0], ids[1], prev
}

func (e *Engine) prevWord() string {
	_, _, p := e.lastWords(len(e.before))
	return p
}

func (e *Engine) prevWordBefore(end int) string {
	if end < 0 || end > len(e.before) {
		return ""
	}
	_, _, p := e.lastWords(end)
	return p
}

// refreshContext sets the decoder's context from the mirror, for a word
// that has not started yet.
func (e *Engine) refreshContext() {
	if len(e.word) > 0 {
		e.refreshContextBeforeWord()
		return
	}
	w1, w2, prev := e.lastWords(len(e.before))
	e.dec.SetContext(w1, w2)
	e.bonus.load(e, prev)
}

// refreshContextBeforeWord sets the context from the text before the word
// being typed.
func (e *Engine) refreshContextBeforeWord() {
	end := len(e.before) - len(e.typed)
	if end < 0 {
		end = 0
	}
	w1, w2, prev := e.lastWords(end)
	e.dec.SetContext(w1, w2)
	e.bonus.load(e, prev)
	// SetContext keeps the taps; scores change with the context.
}

// ---- Personal words and replacements ----

// AddWord adds a word to the personal lexicon.
func (e *Engine) AddWord(w string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.learn.AddWord(w)
	e.maybeRefreshPersonal()
}

// RemoveWord removes a word from the personal lexicon.
func (e *Engine) RemoveWord(w string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.learn.RemoveWord(w)
	e.maybeRefreshPersonal()
}

// AddReplacement adds a text replacement (Sayso's dictionary): typing from
// as a word writes to. It fails when either is too long (see
// learner.AddReplacement).
func (e *Engine) AddReplacement(from, to string, caseSensitive bool) error {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.learn.AddReplacement(learner.Replacement{From: from, To: to, CaseSensitive: caseSensitive})
}

// RemoveReplacement removes the replacement of from.
func (e *Engine) RemoveReplacement(from string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.learn.RemoveReplacement(from)
}

// PersonalWords lists the personal lexicon, each part sorted: the words
// added by hand, then the words learned by keeping them.
func (e *Engine) PersonalWords(added, learned bool) []string {
	e.mu.Lock()
	defer e.mu.Unlock()
	byHand := e.learn.AddedWords()
	var out []string
	if added {
		out = append(out, byHand...)
	}
	if learned {
		var kept []string
		for w := range e.learn.PersonalWords() {
			if !slices.Contains(byHand, w) {
				kept = append(kept, w)
			}
		}
		slices.Sort(kept)
		out = append(out, kept...)
	}
	return out
}

// Replacements lists the text replacements, sorted by what is typed.
func (e *Engine) Replacements() []learner.Replacement {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.learn.Replacements()
}

// ResetLearned forgets everything learned and empties the journal.
func (e *Engine) ResetLearned() error {
	e.mu.Lock()
	defer e.mu.Unlock()
	err := e.learn.Reset()
	e.pending = nil
	e.applyOffsets()
	e.refreshPersonal()
	e.refreshContext()
	return err
}

func (e *Engine) maybeRefreshPersonal() {
	if e.learn.Version != e.personalVersion {
		e.refreshPersonal()
	}
}

// refreshPersonal rebuilds the personal lexicon's trie from the learner.
func (e *Engine) refreshPersonal() {
	e.personalVersion = e.learn.Version
	words := e.learn.PersonalWords()
	b := &lexicon.Builder{Locale: e.model.Locale()}
	e.personalWords = e.personalWords[:0]
	for w, n := range words {
		if _, ok := lexicon.KeyOf(w); !ok || len(w) > 64 {
			continue
		}
		if id, ok := e.model.Find([]byte(w), false); ok && e.model.Flags(id)&lexicon.FlagNoAutocorrect == 0 {
			continue // the model has it as spelled
		}
		// A personal word is about as likely as a common word, more with use.
		cost := 9.0 - 0.5*float64(min(n, 20))
		var flags uint8
		if r, _ := utf8.DecodeRuneInString(w); unicode.IsUpper(r) {
			flags |= lexicon.FlagProper
		}
		if strings.ContainsAny(w, "'’") {
			flags |= lexicon.FlagContraction
		}
		b.Entries = append(b.Entries, lexicon.Entry{Display: w, Cost: cost, Flags: flags})
		e.personalWords = append(e.personalWords, w)
	}
	if len(b.Entries) == 0 {
		e.dec.SetPersonal(nil)
		return
	}
	var buf strings.Builder
	if err := b.Write(&buf); err != nil {
		e.dec.SetPersonal(nil)
		return
	}
	pm, err := lexicon.FromBytes([]byte(buf.String()))
	if err != nil {
		e.dec.SetPersonal(nil)
		return
	}
	e.dec.SetPersonal(pm)
}

func (e *Engine) personalLookup(w string) lexicon.WordID {
	pm := e.dec.Personal()
	if pm == nil {
		return lexicon.NoWord
	}
	id, ok := pm.Find([]byte(w), true)
	if !ok {
		return lexicon.NoWord
	}
	return id
}

// bonusIndex is the user's bigrams after the previous word, by word ID, so
// the decoder can read them without allocating.
type bonusIndex struct {
	m     *lexicon.Model
	byKey map[uint64]float32
}

func (b *bonusIndex) load(e *Engine, prev string) {
	if b.byKey == nil {
		b.byKey = map[uint64]float32{}
	}
	clear(b.byKey)
	if prev == "" {
		return
	}
	e.learn.Following(prev, func(w string, count float64) {
		if count < 0.5 {
			return
		}
		bonus := -float32(min(3, 0.8*logf(1+count)))
		if id, ok := e.model.Find([]byte(w), true); ok {
			b.byKey[uint64(decoder.SrcMain)<<32|uint64(id)] = bonus
		}
		if id := e.personalLookup(w); id != lexicon.NoWord {
			b.byKey[uint64(decoder.SrcPersonal)<<32|uint64(id)] = bonus
		}
	})
}

// WordBonus implements decoder.Bonus.
func (b *bonusIndex) WordBonus(src uint8, w lexicon.WordID) float32 {
	if len(b.byKey) == 0 {
		return 0
	}
	return b.byKey[uint64(src)<<32|uint64(w)]
}

func logf(x float64) float64 { return math.Log(x) }
