package speech

import (
	"math"
	"sort"
	"strings"
	"sync"
	"unicode"

	"github.com/watzon/sayso/sdk/go/protocol"
)

// The echo rule (docs/sdk.md, "System audio"; sayso-speech/src/live/echo.rs):
// without headphones the microphone hears the call too, so a session with the
// sources mic and system drops a microphone segment that a system segment
// covers in time and in words.
//
// A system segment can come after the microphone segment that it covers,
// because each source has its own final pass. So echoGate holds each
// microphone segment until the system source has transcribed the audio of
// that time, and decides then. The segments of the system go out at once.

const (
	// echoLimit is the part of the words of a microphone segment that must
	// be in the system audio of the same time. docs/sdk.md says how the test
	// data in internal/bridge/testdata/echo_pairs.tsv chose it.
	echoLimit = 0.6
	// A microphone segment with echoShortWords words or fewer is an echo only
	// when it is also most of the system words of its time.
	echoShortWords = 3
	echoShortLimit = 0.5
	// echoTimeSlackMs is how far the times of the two sources can differ.
	echoTimeSlackMs = 1_000
	// echoMinTimeCover of a microphone segment must be in the time of system
	// segments.
	echoMinTimeCover = 0.5
	// echoMaxHoldMs is the most system audio that a microphone segment waits.
	echoMaxHoldMs = 60_000
	// echoKeepSystemMs is how much system text the gate keeps, in audio time.
	echoKeepSystemMs = 600_000
)

func isAlphanumeric(c rune) bool {
	return unicode.IsLetter(c) || unicode.IsNumber(c) || unicode.Is(unicode.Other_Alphabetic, c)
}

// echoWords are the words of text, in lower case and without punctuation. A
// character of a script without spaces counts as one word.
func echoWords(text string) []string {
	var out []string
	var word strings.Builder
	flush := func() {
		if word.Len() > 0 {
			out = append(out, word.String())
			word.Reset()
		}
	}
	for _, c := range text {
		switch {
		case protocol.IsUnspaced(c):
			flush()
			out = append(out, string(c))
		case isAlphanumeric(c):
			word.WriteString(strings.ToLower(string(c)))
		case c != '\'' && c != '’':
			flush()
		}
	}
	flush()
	return out
}

// sameWord is true for the same word, or a word with one wrong letter, as two
// transcripts of one sound often have.
func sameWord(a, b string) bool {
	if a == b {
		return true
	}
	x, y := []rune(a), []rune(b)
	if min(len(x), len(y)) < 4 || absDiff(len(x), len(y)) > 1 {
		return false
	}
	start := 0
	for start < len(x) && start < len(y) && x[start] == y[start] {
		start++
	}
	end := 0
	for end < len(x)-start && end < len(y)-start && x[len(x)-1-end] == y[len(y)-1-end] {
		end++
	}
	return max(len(x), len(y))-start-end <= 1
}

func absDiff(a, b int) int {
	if a > b {
		return a - b
	}
	return b - a
}

// commonWords is the number of words of a that are in b in the same order.
func commonWords(a, b []string) int {
	row := make([]int, len(b)+1)
	for _, word := range a {
		diagonal := 0
		for j, other := range b {
			up := row[j+1]
			if sameWord(word, other) {
				row[j+1] = diagonal + 1
			} else {
				row[j+1] = max(up, row[j])
			}
			diagonal = up
		}
	}
	return row[len(b)]
}

// wordsEcho is the decision on the words alone.
func wordsEcho(mic, system []string, limit float32) bool {
	if len(mic) == 0 || len(system) == 0 {
		return false
	}
	common := float32(commonWords(mic, system))
	return common/float32(len(mic)) >= limit && (len(mic) > echoShortWords || common/float32(len(system)) >= echoShortLimit)
}

type timedWord struct {
	at   int64
	text string
}

// systemSegment is a system segment with times on the capture clock.
type systemSegment struct {
	start, end int64
	words      []timedWord
}

func newSystemSegment(s Segment, clockOffset int64) systemSegment {
	start := int64(s.StartMs) + clockOffset
	end := int64(s.EndMs) + clockOffset
	var words []timedWord
	if len(s.Words) > 0 {
		for _, w := range s.Words {
			for _, text := range echoWords(w.Text) {
				words = append(words, timedWord{int64(w.StartMs) + clockOffset, text})
			}
		}
	} else {
		// No times for the words: they are about evenly spread over the segment.
		all := echoWords(s.Text)
		count := int64(max(len(all), 1))
		for i, text := range all {
			words = append(words, timedWord{start + (end-start)*int64(i)/count, text})
		}
	}
	return systemSegment{start: start, end: end, words: words}
}

// covered is true when the system segments cover a microphone segment in
// time and in words.
func covered(start, end int64, mic []string, system []systemSegment) bool {
	var near []*systemSegment
	for i := range system {
		s := &system[i]
		if s.start-echoTimeSlackMs < end && s.end+echoTimeSlackMs > start {
			near = append(near, s)
		}
	}
	spans := make([][2]int64, 0, len(near))
	for _, s := range near {
		spans = append(spans, [2]int64{max(s.start-echoTimeSlackMs, start), min(s.end+echoTimeSlackMs, end)})
	}
	sort.Slice(spans, func(i, j int) bool {
		if spans[i][0] != spans[j][0] {
			return spans[i][0] < spans[j][0]
		}
		return spans[i][1] < spans[j][1]
	})
	var inTime int64
	reached := start
	for _, sp := range spans {
		inTime += max(sp[1]-max(sp[0], reached), 0)
		reached = max(reached, sp[1])
	}
	if float32(inTime) < float32(max(end-start, 1))*echoMinTimeCover {
		return false
	}
	var heard []string
	for _, s := range near {
		for _, w := range s.words {
			if w.at >= start-echoTimeSlackMs && w.at <= end+echoTimeSlackMs {
				heard = append(heard, w.text)
			}
		}
	}
	return wordsEcho(mic, heard, echoLimit)
}

type heldSegment struct {
	segment    Segment
	start, end int64
}

// echoGate decides on the microphone segments of one session. The final
// passes and the system intake call it; no call waits for another source.
type echoGate struct {
	session uint64
	sink    EventSink
	mu      sync.Mutex
	system  []systemSegment
	// doneUntil: the system final pass has finished all the speech before it.
	doneUntil int64
	// heard: the system source has heard the audio up to it.
	heard int64
	ended bool
	held  []heldSegment
	kept  []Segment
}

func newEchoGate(session uint64, sink EventSink) *echoGate {
	return &echoGate{session: session, sink: sink, doneUntil: math.MinInt64, heard: math.MinInt64}
}

// micSegment is a microphone segment with clockOffset from its times to the
// capture clock.
func (g *echoGate) micSegment(s Segment, clockOffset int64) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.held = append(g.held, heldSegment{segment: s, start: int64(s.StartMs) + clockOffset, end: int64(s.EndMs) + clockOffset})
	g.release()
}

// systemSegment is a segment of the system source, after its event went out.
func (g *echoGate) systemSegment(s Segment, clockOffset int64) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.system = append(g.system, newSystemSegment(s, clockOffset))
	newest := g.system[len(g.system)-1].end
	kept := g.system[:0]
	for _, x := range g.system {
		if x.end >= newest-echoKeepSystemMs {
			kept = append(kept, x)
		}
	}
	g.system = kept
}

// systemDoneUntil says that the system final pass has finished all the
// speech before clockMs.
func (g *echoGate) systemDoneUntil(clockMs int64) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.doneUntil = max(g.doneUntil, clockMs)
	g.release()
}

// systemHeard says that the system source has heard the audio up to clockMs.
func (g *echoGate) systemHeard(clockMs int64) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.heard = max(g.heard, clockMs)
	g.release()
}

// systemEnded says that no more system segments come. It can be called more
// than once.
func (g *echoGate) systemEnded() {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.ended = true
	g.release()
}

// keptSegments are the microphone segments that stayed.
func (g *echoGate) keptSegments() []Segment {
	g.mu.Lock()
	defer g.mu.Unlock()
	return append([]Segment(nil), g.kept...)
}

// release decides on the held segments whose time has come, in order. The
// event goes out under the lock, so two goroutines cannot change the order.
func (g *echoGate) release() {
	for len(g.held) > 0 {
		front := g.held[0]
		ready := g.ended || g.doneUntil >= front.end+echoTimeSlackMs || g.heard >= front.end+echoMaxHoldMs
		if !ready {
			return
		}
		g.held = g.held[1:]
		if covered(front.start, front.end, echoWords(front.segment.Text), g.system) {
			Log.Debug("sayso: live session: a microphone segment is an echo", "session", g.session, "ms", front.end-front.start)
			continue
		}
		session, source := g.session, SourceMic
		g.sink.Event(&SegmentEvent{Session: &session, Source: &source, Segment: front.segment})
		g.kept = append(g.kept, front.segment)
	}
}
