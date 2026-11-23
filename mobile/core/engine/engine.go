// Package engine is the keyboard core the apps talk to (through capi): it
// keeps a mirror of the text before the cursor and the word being typed,
// turns taps into keys, decides corrections on commit, undoes them on
// backspace, and feeds the learner. Every method is safe for concurrent use;
// calls are serialized.
package engine

import (
	"errors"
	"sync"
	"unicode"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/decoder"
	"github.com/watzon/sayso/mobile/core/learner"
	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/textrules"
	"github.com/watzon/sayso/mobile/core/touch"
)

// Editor flags, set per text field.
const (
	FlagPrivacy       uint32 = 1 << iota // learn and write nothing (incognito or sensitive fields)
	FlagAutocap                          // sentence starts get a capital
	FlagNoAutocorrect                    // never replace what is typed
	FlagNoSuggest                        // no suggestions (passwords)
)

// ActionKind is what the app must do to the text.
type ActionKind int32

const (
	// ActionNone: the app does its usual thing (inserts the character,
	// deletes one character).
	ActionNone ActionKind = iota
	// ActionReplace: delete Delete characters before the cursor, then
	// insert Text.
	ActionReplace
)

// Action is a change to the text before the cursor.
type Action struct {
	Kind ActionKind
	// DeleteChars and DeleteUTF16 are the same span, in code points and in
	// UTF-16 units (Android counts the second, iOS the first).
	DeleteChars, DeleteUTF16 int
	Text                     string
	// Corrected is set when the action autocorrected a word.
	Corrected bool
}

// TouchResult is the key a tap typed.
type TouchResult struct {
	Code     rune // the character typed (a letter in the right case, ' ', '.', ...) or a negative function code
	Key      int  // the key's index in the layout
	Borrowed bool // hit-target expansion moved the tap off the key under it
	Core     bool // the tap was in the core of its key
}

const (
	maxBefore      = 256 // bytes of text before the cursor kept for context
	maxSuggestions = 8
	historySize    = 8
)

// commit is one committed word, for undo and for spotting manual fixes.
type commit struct {
	typed, output, sep string
	corrected          bool
	taps               []tapRec
}

type tapRec struct {
	dist  touch.Dist
	c     byte  // letter typed, lower case
	key   int16 // key that typed it
	synth bool  // not a real tap (typed by a hardware key, or reopened text)
	upper bool
}

// Engine is one keyboard session.
type Engine struct {
	mu sync.Mutex

	model  *lexicon.Model
	layout *touch.Layout
	tm     *touch.Model
	dec    *decoder.Decoder
	learn  *learner.Learner
	flags  uint32

	before []byte // text before the cursor, as the engine believes it
	word   []tapRec
	raw    bool // the word has a character the decoder cannot search
	typed  []byte
	noFix  bool // the word was reverted: commit it as typed

	last       commit // the last commit, while nothing was typed after it
	lastValid  bool
	autoSpace  bool // the last action ended with a space the engine added
	history    [historySize]commit
	histN      int
	reopened   string // a committed word the user backspaced into
	reopenedOK bool
	edited     bool // the reopened word lost a letter
	pending    []learner.Sample

	personalVersion int
	bonus           bonusIndex

	sugs  [maxSuggestions]decoder.Suggestion
	nsugs int

	lastTouch     TouchResult
	lastDecision  decoder.Decision
	prior         touch.Prior
	scratch       touch.Dist
	personalWords []string
}

// New makes an engine over an open model, with the plain QWERTY layout
// until SetLayout is called. journal is the learner's file ("" for none).
func New(m *lexicon.Model, journal string) (*Engine, error) {
	if m == nil {
		return nil, errors.New("engine: no model")
	}
	e := &Engine{model: m, learn: learner.New(), flags: FlagAutocap}
	if err := e.learn.Open(journal); err != nil {
		return nil, err
	}
	l, err := touch.NewLayout(touch.QWERTY(36, 54))
	if err != nil {
		return nil, err
	}
	e.layout = l
	e.tm = touch.NewModel(l, touch.DefaultParams)
	e.dec = decoder.New(m, e.tm, decoder.DefaultParams)
	e.bonus.m = m
	e.applyOffsets()
	e.refreshPersonal()
	e.refreshContext()
	return e, nil
}

// Close closes the learner's journal. The model is the caller's.
func (e *Engine) Close() error {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.learn.Close()
}

// Learner is the engine's learner (tests and the replay benchmark).
func (e *Engine) Learner() *learner.Learner { return e.learn }

// Decoder is the engine's decoder (tests and the replay benchmark).
func (e *Engine) Decoder() *decoder.Decoder { return e.dec }

// SetJournal reopens the learner on another journal ("" for none).
func (e *Engine) SetJournal(path string) error {
	e.mu.Lock()
	defer e.mu.Unlock()
	if err := e.learn.Open(path); err != nil {
		return err
	}
	e.learn.SetPrivacy(e.flags&FlagPrivacy != 0)
	e.applyOffsets()
	e.refreshPersonal()
	e.refreshContext()
	return nil
}

// SetLayout sets the keyboard's keys.
func (e *Engine) SetLayout(keys []touch.Key) error {
	l, err := touch.NewLayout(keys)
	if err != nil {
		return err
	}
	e.mu.Lock()
	defer e.mu.Unlock()
	e.layout = l
	e.tm = touch.NewModel(l, e.tm.P)
	rows := make([]int, l.Len())
	for i := range rows {
		rows[i] = l.Row(i)
	}
	e.learn.SetRows(rows)
	e.applyOffsets()
	e.dec.SetTouch(e.tm)
	e.resetWord()
	e.pending = nil
	return nil
}

// SetTouchParams changes the touch model's tuning (the replay benchmark).
func (e *Engine) SetTouchParams(p touch.Params) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.tm.P = p
}

// applyOffsets copies the learned offsets (in pitches) into the touch model.
func (e *Engine) applyOffsets() {
	pitch := e.layout.Pitch()
	for i := 0; i < e.layout.Len(); i++ {
		dx, dy := e.learn.KeyOffset(i)
		e.tm.SetOffset(i, dx*pitch, dy*pitch)
	}
}

// SetFlags sets the editor flags of the focused field.
func (e *Engine) SetFlags(f uint32) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.flags = f
	e.learn.SetPrivacy(f&FlagPrivacy != 0)
	if f&FlagPrivacy != 0 {
		e.pending = nil
	}
}

// Flags are the editor flags.
func (e *Engine) Flags() uint32 {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.flags
}

// SetContext tells the engine the text before the cursor, after the cursor
// moved somewhere the engine did not put it (a new field, a tap in the
// text). If the text is what the engine already has, nothing changes, so
// apps may call it after every edit.
func (e *Engine) SetContext(text string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	tail := trimFront(text, maxBefore)
	if tail == string(e.before) {
		return
	}
	e.before = append(e.before[:0], tail...)
	e.resetWord()
	e.lastValid, e.autoSpace, e.reopenedOK, e.edited = false, false, false, false
	e.pending = nil
	e.reopenTrailingWord(false)
	e.refreshContext()
}

// ShouldCapitalize reports whether the next letter should be a capital.
func (e *Engine) ShouldCapitalize() bool {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.flags&FlagAutocap != 0 && len(e.word) == 0 && textrules.SentenceStart(string(e.before))
}

func trimFront(s string, n int) string {
	if len(s) <= n {
		return s
	}
	s = s[len(s)-n:]
	for len(s) > 0 && !utf8.RuneStart(s[0]) {
		s = s[1:]
	}
	return s
}

func (e *Engine) appendBefore(s string) {
	e.before = append(e.before, s...)
	if len(e.before) > 2*maxBefore {
		t := trimFront(string(e.before), maxBefore)
		e.before = append(e.before[:0], t...)
	}
}

func (e *Engine) resetWord() {
	e.word = e.word[:0]
	e.typed = e.typed[:0]
	e.raw, e.noFix = false, false
	e.dec.Reset()
	e.nsugs = 0
}

// ---- Typing ----

// OnTouch turns a tap into a key. A letter joins the word being typed and
// is returned in upper case when shifted; anything else is only resolved,
// and the app passes it on (TypeChar for characters, Backspace, ...). The
// letter path does not allocate.
func (e *Engine) OnTouch(x, y float32, shifted bool) TouchResult {
	e.mu.Lock()
	defer e.mu.Unlock()
	if len(e.word) < decoder.MaxTaps && !e.raw {
		e.dec.NextPrior(&e.prior)
	} else {
		e.prior.Valid = false
	}
	if e.flags&FlagNoAutocorrect != 0 {
		e.prior.Valid = false // no expansion where the field wants raw typing
	}
	r := e.tm.Resolve(x, y, &e.prior)
	k := e.layout.Keys[r.Key]
	res := TouchResult{Code: k.Code, Key: r.Key, Borrowed: r.Borrowed(), Core: r.Core}
	if e.layout.Kind(r.Key) == touch.KindLetter {
		c := byte(k.Code)
		if shifted {
			res.Code = unicode.ToUpper(k.Code)
		}
		e.tm.Spatial(x, y, &e.scratch)
		e.addLetter(c, shifted, int16(r.Key), false)
	}
	e.lastTouch = res
	return res
}

// addLetter adds a letter to the word, from e.scratch.
func (e *Engine) addLetter(c byte, upper bool, key int16, synth bool) {
	if len(e.word) == 0 {
		e.lastValid = false // typing after a commit: backspace no longer undoes it
		e.confirmPending()
	}
	e.autoSpace = false
	e.scratch.Key = key
	if len(e.word) < 64 {
		e.word = append(e.word, tapRec{dist: e.scratch, c: c, key: key, synth: synth, upper: upper})
	}
	ch := c
	if upper {
		ch = c - 'a' + 'A'
	}
	e.typed = append(e.typed, ch)
	e.before = append(e.before, ch)
	if !e.raw {
		e.dec.AddTap(&e.scratch, c)
	}
	e.nsugs = 0
}

// synthTap fills e.scratch as a tap at the centre of letter c's key.
func (e *Engine) synthTap(c byte) (int16, bool) {
	k := e.layout.LetterKey(c)
	if k < 0 {
		return -1, false
	}
	x, y := e.layout.Center(k)
	dx, dy := e.tm.Offset(k)
	e.tm.Spatial(x+dx, y+dy, &e.scratch)
	return int16(k), true
}
