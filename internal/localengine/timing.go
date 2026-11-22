package localengine

import (
	"strings"
	"unicode/utf8"
)

// word is one word and its time, in milliseconds from the start of the audio.
type word struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
}

// textSegment is a part of the text with its time.
type textSegment struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
	// Words are set when the model gives word times.
	Words []word `json:"words,omitempty"`
}

// timingKind is how much timing a final pass gives.
type timingKind int

const (
	timingNone timingKind = iota
	timingWords
	timingSegments
)

// timed is the text of a final pass, its timing, and the language that the
// model found (empty when it does not find the language).
type timed struct {
	text     string
	kind     timingKind
	words    []word
	segments []textSegment
	language string
}

func untimed(text string) timed { return timed{text: text} }

const (
	// pauseMs ends a segment before a pause of this length between two words.
	pauseMs = 700
	// maxSegmentMs ends a segment before it would pass this length.
	maxSegmentMs = 20_000
	// lastTokenMs is the end of a token without a duration and without a token after it.
	lastTokenMs = 400.0
)

func secondsToMs(s float32) uint64 {
	if s < 0 {
		s = 0
	}
	return uint64(s * 1000)
}

// wordsFromTokens makes words from the tokens of a model and the start time
// of each token, in seconds. A token that starts with a space or ▁ starts a
// word, and so does each CJK character. Other tokens join the word before
// them. durations gives the length of each token, when the model has it.
func wordsFromTokens(tokens []string, starts []float32, durations []float32, audioMs uint64) []word {
	count := min(len(tokens), len(starts))
	var words []word
	spaceBefore := false
	for i := range count {
		token := tokens[i]
		start := secondsToMs(starts[i])
		var end uint64
		switch {
		case i < len(durations) && durations[i] > 0:
			end = secondsToMs(starts[i] + durations[i])
		case i+1 < count:
			end = secondsToMs(starts[i+1])
		default:
			s := starts[i]
			if s < 0 {
				s = 0
			}
			end = uint64(s*1000 + lastTokenMs)
		}
		end = max(start, min(end, max(audioMs, start)))
		marked := spaceBefore || strings.HasPrefix(token, " ") || strings.HasPrefix(token, "▁")
		spaceBefore = false
		piece := strings.TrimSpace(strings.ReplaceAll(token, "▁", " "))
		if piece == "" {
			spaceBefore = marked || token != ""
			continue
		}
		first, _ := utf8.DecodeRuneInString(piece)
		cjk := isCJK(first)
		if n := len(words); n > 0 && !marked && !cjk {
			last := &words[n-1]
			lastRune, _ := utf8.DecodeLastRuneInString(last.Text)
			if !isCJK(lastRune) || allPunctuation(piece) {
				last.Text += piece
				last.EndMs = max(last.EndMs, end)
				continue
			}
		}
		words = append(words, word{StartMs: start, EndMs: end, Text: piece})
	}
	return words
}

func allPunctuation(s string) bool {
	for _, c := range s {
		if !(c < 0x80 && strings.ContainsRune("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~", c)) && !isCJKPunctuation(c) {
			return false
		}
	}
	return true
}

// segmentsFromWords cuts words into segments: after a word that ends a
// sentence, before a pause of 700 ms or more, and before a segment passes 20 s.
func segmentsFromWords(words []word) []textSegment {
	var segments []textSegment
	var current []word
	for _, w := range words {
		if n := len(current); n > 0 {
			pause := w.StartMs >= current[n-1].EndMs && w.StartMs-current[n-1].EndMs >= pauseMs
			tooLong := w.EndMs >= current[0].StartMs && w.EndMs-current[0].StartMs > maxSegmentMs
			if pause || tooLong {
				segments = append(segments, segmentOf(current))
				current = nil
			}
		}
		current = append(current, w)
		if endsSentence(w.Text) {
			segments = append(segments, segmentOf(current))
			current = nil
		}
	}
	if len(current) > 0 {
		segments = append(segments, segmentOf(current))
	}
	return segments
}

func segmentOf(words []word) textSegment {
	return textSegment{StartMs: words[0].StartMs, EndMs: words[len(words)-1].EndMs, Text: joinWords(words), Words: words}
}

// joinWords is the text of words: a space between two words, except between CJK text.
func joinWords(words []word) string {
	var b strings.Builder
	for _, w := range words {
		last, _ := utf8.DecodeLastRuneInString(b.String())
		first, _ := utf8.DecodeRuneInString(w.Text)
		cjkJoin := b.Len() > 0 && (isCJK(last) || isCJKPunctuation(last)) && isCJK(first)
		if b.Len() > 0 && !cjkJoin {
			b.WriteByte(' ')
		}
		b.WriteString(w.Text)
	}
	return b.String()
}

func endsSentence(text string) bool {
	text = strings.TrimRight(text, "\"')”’」』")
	r, _ := utf8.DecodeLastRuneInString(text)
	return strings.ContainsRune(".?!。？！", r) && text != ""
}

func isCJK(c rune) bool {
	return (c >= 0x3040 && c <= 0x30FF) || (c >= 0x3400 && c <= 0x4DBF) || (c >= 0x4E00 && c <= 0x9FFF) ||
		(c >= 0xAC00 && c <= 0xD7AF) || (c >= 0xF900 && c <= 0xFAFF)
}

func isCJKPunctuation(c rune) bool {
	return (c >= 0x3000 && c <= 0x303F) || (c >= 0xFF00 && c <= 0xFFEF)
}

// piece is one piece of the audio that the final pass transcribed alone.
type piece struct {
	startMs, endMs uint64
	timed          timed
}

// allSegments are the segments of a whole recording from its pieces. Times
// move from the start of each piece to the start of the recording. A piece
// without times is one segment.
func allSegments(pieces []piece) []textSegment {
	var all []textSegment
	for _, p := range pieces {
		shift := func(ms uint64) uint64 { return min(ms+p.startMs, p.endMs) }
		switch p.timed.kind {
		case timingWords:
			for _, s := range segmentsFromWords(p.timed.words) {
				s.StartMs, s.EndMs = shift(s.StartMs), shift(s.EndMs)
				moved := make([]word, len(s.Words))
				for i, w := range s.Words {
					moved[i] = word{StartMs: shift(w.StartMs), EndMs: shift(w.EndMs), Text: w.Text}
				}
				s.Words = moved
				all = append(all, s)
			}
		case timingSegments:
			for _, s := range p.timed.segments {
				all = append(all, textSegment{StartMs: shift(s.StartMs), EndMs: shift(s.EndMs), Text: strings.TrimSpace(s.Text)})
			}
		default:
			all = append(all, textSegment{StartMs: p.startMs, EndMs: p.endMs, Text: strings.TrimSpace(p.timed.text)})
		}
	}
	kept := all[:0]
	for _, s := range all {
		if s.Text != "" {
			kept = append(kept, s)
		}
	}
	// In order and without overlaps, also when a model gives a time twice.
	var lastEnd uint64
	for i := range kept {
		kept[i].StartMs = max(kept[i].StartMs, lastEnd)
		kept[i].EndMs = max(kept[i].EndMs, kept[i].StartMs)
		lastEnd = kept[i].EndMs
	}
	if kept == nil {
		return []textSegment{}
	}
	return kept
}
