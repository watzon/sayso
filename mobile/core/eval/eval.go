// Package eval is the typing replay benchmark: it types held-out sentences
// (never used to build the model) with synthetic noisy taps drawn from a
// touch model of a user, through the whole engine, and measures how many
// words come out right.
package eval

import (
	"bufio"
	"fmt"
	"io"
	"math/rand/v2"
	"os"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/engine"
	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/touch"
)

// User is the synthetic typist.
type User struct {
	// Noise is the spread of taps around the aimed point, in key pitches.
	NoiseX, NoiseY float32
	// BiasX, BiasY is where this user lands relative to key centres, in
	// pitches (people tend to hit low).
	BiasX, BiasY float32
	// TypoRate is the chance per word of a slip of the finger: a letter
	// left out, doubled, or two letters swapped.
	TypoRate float64
	// SkipApostrophe is the chance of typing "dont" for "don't".
	SkipApostrophe float64
	Seed           uint64
}

// DefaultUser is the typist of the gate.
var DefaultUser = User{NoiseX: 0.24, NoiseY: 0.22, BiasX: 0.06, BiasY: 0.2, TypoRate: 0.03, SkipApostrophe: 0.5, Seed: 7}

// Mode is what the engine is allowed to do.
type Mode int

const (
	Raw       Mode = iota // the keys under the taps, nothing else
	Baseline              // the decoder and autocorrect, no hit-target expansion, no learning
	Expansion             // plus hit-target expansion
	Learning              // plus adaptive learning
)

func (m Mode) String() string {
	return [...]string{"raw taps", "decoder", "+expansion", "+expansion+learning"}[m]
}

// Result counts one replay.
type Result struct {
	Mode  Mode
	Words int
	// Errors are words that came out wrong (case aside).
	Errors int
	// TypedRight are words whose taps typed them right; FalseCorrections
	// are those of them the engine changed to something wrong.
	TypedRight, FalseCorrections int
	// TypedWrong are words whose taps typed them wrong; Recovered are those
	// of them the engine put right.
	TypedWrong, Recovered int
	// Corrections counts every replacement the engine made.
	Corrections int
	// FalseExamples are a few false corrections, "typed → written".
	FalseExamples []string
}

func (r Result) WER() float64          { return ratio(r.Errors, r.Words) }
func (r Result) FalseRate() float64    { return ratio(r.FalseCorrections, r.TypedRight) }
func (r Result) RecoveryRate() float64 { return ratio(r.Recovered, r.TypedWrong) }

func ratio(a, b int) float64 {
	if b == 0 {
		return 0
	}
	return float64(a) / float64(b)
}

// LoadSentences reads a held-out file: one sentence per line, # comments.
func LoadSentences(path string, limit int) ([]string, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	return readSentences(f, limit)
}

func readSentences(r io.Reader, limit int) ([]string, error) {
	var out []string
	sc := bufio.NewScanner(r)
	for sc.Scan() {
		l := strings.TrimSpace(sc.Text())
		if l == "" || strings.HasPrefix(l, "#") {
			continue
		}
		out = append(out, l)
		if limit > 0 && len(out) == limit {
			break
		}
	}
	return out, sc.Err()
}

// words splits a sentence into the words the replay types: letters with
// inner apostrophes. Sentences with digits or other symbols in a word are
// skipped by returning nil.
func words(s string) []string {
	var out []string
	for _, f := range strings.FieldsFunc(s, func(r rune) bool { return unicode.IsSpace(r) || r == '-' || r == '—' }) {
		f = strings.Trim(f, ".,!?;:\"()“”‘…")
		f = strings.ReplaceAll(f, "’", "'")
		f = strings.Trim(f, "'")
		if f == "" {
			continue
		}
		for _, r := range f {
			if !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r == '\'') {
				return nil
			}
		}
		out = append(out, f)
	}
	return out
}

// Run replays sentences in one mode.
func Run(m *lexicon.Model, sentences []string, u User, mode Mode) (Result, error) {
	e, err := engine.New(m, "")
	if err != nil {
		return Result{}, err
	}
	defer e.Close()
	if err := e.SetLayout(touch.QWERTY(36, 54)); err != nil {
		return Result{}, err
	}
	flags := engine.FlagAutocap
	switch mode {
	case Raw:
		flags |= engine.FlagNoAutocorrect | engine.FlagPrivacy
	case Baseline, Expansion:
		flags |= engine.FlagPrivacy
	}
	e.SetFlags(flags)
	if mode == Raw || mode == Baseline {
		p := touch.DefaultParams
		p.MaxBorrow = 0
		e.SetTouchParams(p)
	}
	rng := rand.New(rand.NewPCG(u.Seed, 0x5a7))
	l := touch.QWERTY(36, 54)
	layout, _ := touch.NewLayout(l)
	pitch := layout.Pitch()
	res := Result{Mode: mode}
	for _, s := range sentences {
		ws := words(s)
		if len(ws) == 0 {
			continue
		}
		e.SetContext("")
		text := ""
		for _, truth := range ws {
			aim := slip(truth, u, rng)
			start := len(text)
			for i, c := range aim {
				if c == '\'' {
					text = apply(text, e.TypeChar('\''), "'")
					continue
				}
				k := layout.LetterKey(byte(unicode.ToLower(c)))
				cx, cy := layout.Center(k)
				x := cx + (u.BiasX+float32(rng.NormFloat64())*u.NoiseX)*pitch
				y := cy + (u.BiasY+float32(rng.NormFloat64())*u.NoiseY)*pitch
				r := e.OnTouch(x, y, i == 0 && unicode.IsUpper(c))
				if r.Code > 0 && unicode.IsLetter(r.Code) {
					text += string(r.Code)
				} else {
					// A tap that landed on a non-letter key: the app passes it on.
					switch {
					case r.Code == ' ':
						text = apply(text, e.TypeChar(' '), " ")
					case r.Code > 0:
						text = apply(text, e.TypeChar(r.Code), string(r.Code))
					case r.Code == touch.CodeBackspace:
						text = backspace(text, e.Backspace())
					}
				}
			}
			typed := text[start:]
			a := e.TypeChar(' ')
			text = apply(text, a, " ")
			if a.Kind == engine.ActionReplace {
				res.Corrections++
			}
			written := lastWord(text[start:])
			res.Words++
			right := strings.EqualFold(written, truth)
			if !right {
				res.Errors++
			}
			if strings.EqualFold(strings.TrimSpace(typed), truth) {
				res.TypedRight++
				if !right {
					res.FalseCorrections++
					if len(res.FalseExamples) < 12 {
						res.FalseExamples = append(res.FalseExamples, fmt.Sprintf("%s → %s", typed, written))
					}
				}
			} else {
				res.TypedWrong++
				if right {
					res.Recovered++
				}
			}
		}
	}
	return res, nil
}

// slip is what the user aims to type: the word, sometimes with a slip of
// the finger, sometimes without its apostrophe.
func slip(w string, u User, rng *rand.Rand) string {
	if strings.Contains(w, "'") && rng.Float64() < u.SkipApostrophe {
		w = strings.ReplaceAll(w, "'", "")
	}
	if len(w) < 3 || rng.Float64() >= u.TypoRate {
		return w
	}
	b := []byte(w)
	i := 1 + rng.IntN(len(b)-1)
	switch rng.IntN(3) {
	case 0: // a letter left out
		b = append(b[:i:i], b[i+1:]...)
	case 1: // a letter doubled
		b = append(b[:i+1:i+1], b[i:]...)
	default: // two letters swapped
		b[i-1], b[i] = b[i], b[i-1]
	}
	return strings.Trim(string(b), "'")
}

func apply(text string, a engine.Action, fallback string) string {
	if a.Kind == engine.ActionNone {
		return text + fallback
	}
	r := []rune(text)
	n := min(a.DeleteChars, len(r))
	return string(r[:len(r)-n]) + a.Text
}

func backspace(text string, a engine.Action) string {
	if a.Kind == engine.ActionNone {
		_, size := utf8.DecodeLastRuneInString(text)
		return text[:len(text)-size]
	}
	return apply(text, a, "")
}

// lastWord is what one word's typing wrote, without the separator; a split
// writes two words.
func lastWord(s string) string { return strings.TrimSpace(s) }

// Report runs every mode and formats a table.
func Report(m *lexicon.Model, sentences []string, u User) ([]Result, string, error) {
	var out []Result
	var b strings.Builder
	fmt.Fprintf(&b, "%-22s %7s %8s %10s %10s %8s\n", "mode", "words", "WER", "false-AC", "recovery", "changes")
	for _, mode := range []Mode{Raw, Baseline, Expansion, Learning} {
		r, err := Run(m, sentences, u, mode)
		if err != nil {
			return nil, "", err
		}
		out = append(out, r)
		fmt.Fprintf(&b, "%-22s %7d %7.2f%% %9.3f%% %9.1f%% %8d\n", mode, r.Words, 100*r.WER(), 100*r.FalseRate(), 100*r.RecoveryRate(), r.Corrections)
	}
	return out, b.String(), nil
}
