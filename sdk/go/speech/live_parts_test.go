package speech

import (
	"bufio"
	"encoding/binary"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/protocol"
)

// ---- echo rule (echo.rs) ----

type textSink struct {
	mu    sync.Mutex
	texts []string
}

func (s *textSink) Event(e Event) {
	if seg, ok := e.(*SegmentEvent); ok {
		s.mu.Lock()
		s.texts = append(s.texts, seg.Text)
		s.mu.Unlock()
	}
}

func (s *textSink) got() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]string(nil), s.texts...)
}

func seg(start, end uint64, text string) Segment {
	return Segment{StartMs: start, EndMs: end, Text: text}
}

func newTestGate() (*textSink, *echoGate) {
	sink := &textSink{}
	return sink, newEchoGate(1, sink)
}

func TestEchoWordsIgnoreCaseAndPunctuationAndSplitScriptsWithoutSpaces(t *testing.T) {
	if got := echoWords("Don't stop, the 2 BIG-ones!"); !reflect.DeepEqual(got, []string{"dont", "stop", "the", "2", "big", "ones"}) {
		t.Fatal(got)
	}
	if got := echoWords("今天 hello世界"); !reflect.DeepEqual(got, []string{"今", "天", "hello", "世", "界"}) {
		t.Fatal(got)
	}
	if got := echoWords(" ... "); len(got) != 0 {
		t.Fatal(got)
	}
}

func TestOneWrongLetterInALongerWordIsTheSameWord(t *testing.T) {
	for _, c := range []struct {
		a, b string
		same bool
	}{{"transcribe", "transcribes", true}, {"meeting", "meating", true}, {"the", "she", false}, {"there", "their", false}, {"monday", "sunday", false}} {
		if sameWord(c.a, c.b) != c.same {
			t.Fatal(c)
		}
	}
}

func TestShortEchoSegmentsMustBeMostOfTheSystemWords(t *testing.T) {
	w := echoWords
	cases := []struct {
		mic, system string
		echo        bool
	}{
		{"Yes.", "yes", true},
		{"Yes.", "yes we should ship the release on Friday", false},
		{"I think so", "I think we should ship it", false},
		{"I think so", "I think so and we should ship it on Friday", false},
		{"I think so", "I think so", true},
		{"I think so", "yes I think so too", true},
		// Four words: only the limit of the words of the microphone counts.
		{"I think so too", "I think so too and we should ship it on Friday", true},
	}
	for _, c := range cases {
		if wordsEcho(w(c.mic), w(c.system), echoLimit) != c.echo {
			t.Fatal(c)
		}
	}
}

func TestAMicrophoneSegmentWaitsForTheSystemAndAnEchoIsDropped(t *testing.T) {
	sink, g := newTestGate()
	g.micSegment(seg(1_000, 3_000, "we should ship the release on friday"), 0)
	g.systemDoneUntil(3_500)
	if len(sink.got()) != 0 {
		t.Fatal("the system can still have a segment of this time")
	}
	g.systemSegment(seg(900, 3_100, "We should ship the release on Friday."), 0)
	g.systemDoneUntil(4_000)
	if len(sink.got()) != 0 || len(g.keptSegments()) != 0 {
		t.Fatal(sink.got())
	}
}

func TestTheOwnWordsOfTheUserStayAndKeepTheirOrder(t *testing.T) {
	sink, g := newTestGate()
	g.micSegment(seg(1_000, 2_000, "I can take that one"), 0)
	g.micSegment(seg(2_500, 3_500, "and the notes too"), 0)
	g.systemSegment(seg(1_000, 3_500, "does anyone want to review the budget numbers"), 0)
	g.systemDoneUntil(10_000)
	if got := sink.got(); !reflect.DeepEqual(got, []string{"I can take that one", "and the notes too"}) || len(g.keptSegments()) != 2 {
		t.Fatal(got)
	}
}

func TestEchoTimesAndClockOffsets(t *testing.T) {
	// The same words at another time are not an echo.
	sink, g := newTestGate()
	g.systemSegment(seg(1_000, 3_000, "we should ship the release on friday"), 0)
	g.micSegment(seg(30_000, 32_000, "we should ship the release on friday"), 0)
	g.systemDoneUntil(40_000)
	if len(sink.got()) != 1 {
		t.Fatal(sink.got())
	}
	// The segment times differ by 20 s, and the capture clock says that they are the same time.
	sink, g = newTestGate()
	g.systemSegment(seg(1_000, 3_000, "we should ship the release on friday"), 100_000)
	g.micSegment(seg(21_000, 23_000, "we should ship the release on friday"), 80_000)
	g.systemDoneUntil(110_000)
	if len(sink.got()) != 0 {
		t.Fatal(sink.got())
	}
	// Only the words of the same time in a long system segment count.
	sink, g = newTestGate()
	long := "i think we should go " + strings.Repeat("and talk about many other things here ", 10)
	g.systemSegment(seg(0, 30_000, long), 0)
	g.micSegment(seg(27_000, 28_500, "I think we should go"), 0)
	g.systemDoneUntil(40_000)
	if len(sink.got()) != 1 {
		t.Fatal(sink.got())
	}
}

func TestTheEndOfTheSystemAndTheHoldLimitReleaseWhatWaits(t *testing.T) {
	sink, g := newTestGate()
	g.micSegment(seg(1_000, 2_000, "this one stays"), 0)
	g.systemEnded()
	if len(sink.got()) != 1 {
		t.Fatal(sink.got())
	}
	g.micSegment(seg(5_000, 6_000, "and this one too"), 0)
	if len(sink.got()) != 2 {
		t.Fatal("no wait after the end")
	}
	sink, g = newTestGate()
	g.micSegment(seg(1_000, 2_000, "this one stays"), 0)
	g.systemHeard(2_000 + echoMaxHoldMs - 1)
	if len(sink.got()) != 0 {
		t.Fatal(sink.got())
	}
	g.systemHeard(2_000 + echoMaxHoldMs)
	if len(sink.got()) != 1 {
		t.Fatal(sink.got())
	}
}

// TestEchoLimitKeepsRealRepliesAndDropsMostEchoes checks the echo limit
// against the labeled pairs of testdata/echo_pairs.tsv.
func TestEchoLimitKeepsRealRepliesAndDropsMostEchoes(t *testing.T) {
	f, err := os.Open(filepath.Join("testdata", "echo_pairs.tsv"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	type pair struct {
		echo        bool
		mic, system []string
	}
	var pairs []pair
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := strings.TrimRight(sc.Text(), "\r")
		if strings.HasPrefix(line, "#") || strings.TrimSpace(line) == "" {
			continue
		}
		fields := strings.Split(line, "\t")
		pairs = append(pairs, pair{fields[0] == "echo", echoWords(fields[1]), echoWords(fields[2])})
	}
	var echoes, others, droppedEchoes, droppedOthers int
	for _, p := range pairs {
		dropped := wordsEcho(p.mic, p.system, echoLimit)
		if p.echo {
			echoes++
			if dropped {
				droppedEchoes++
			}
		} else {
			others++
			if dropped {
				droppedOthers++
			}
		}
	}
	if echoes == 0 || others == 0 {
		t.Fatal(echoes, others)
	}
	if rate := float64(droppedEchoes) / float64(echoes); rate < 0.9 {
		t.Fatalf("the limit drops %.3f of the echoes", rate)
	}
	if rate := float64(droppedOthers) / float64(others); rate > 0.02 {
		t.Fatalf("the limit drops %.3f of the real text", rate)
	}
}

// ---- silence watch (silence.rs) ----

func TestSilenceWatch(t *testing.T) {
	runs := 0
	w := newSilenceWatch(100*time.Millisecond, 1, func() bool { runs++; return true })
	w.heard(0, 99)
	if runs != 0 {
		t.Fatal(runs)
	}
	w.heard(0, 100)
	w.heard(0, 200)
	if runs != 1 {
		t.Fatal("it runs one time at the limit", runs)
	}

	runs = 0
	w = newSilenceWatch(100*time.Millisecond, 2, func() bool { runs++; return true })
	w.heard(0, 500)
	w.heard(1, 50)
	if runs != 0 {
		t.Fatal("it waits for every source")
	}
	w.heard(1, 100)
	if runs != 1 {
		t.Fatal(runs)
	}

	runs = 0
	w = newSilenceWatch(100*time.Millisecond, 1, func() bool { runs++; return false })
	w.heard(0, 100)
	w.heard(0, 120)
	if runs != 2 {
		t.Fatal("it asks again when the action says not now", runs)
	}
}

// ---- recordings (wav.rs) ----

func TestTheHeaderHasTheSizesAfterFinish(t *testing.T) {
	path := filepath.Join(t.TempDir(), "mic.wav")
	w, err := createWav(path)
	if err != nil {
		t.Fatal(err)
	}
	w.write([]float32{0, 0.5, -0.5})
	w.write([]float32{1, -1})
	if n, err := w.finish(); err != nil || n != 5 {
		t.Fatal(n, err)
	}
	b, _ := os.ReadFile(path)
	le32 := func(at int) uint32 { return binary.LittleEndian.Uint32(b[at:]) }
	le16 := func(at int) uint16 { return binary.LittleEndian.Uint16(b[at:]) }
	if len(b) != 44+10 || string(b[0:4]) != "RIFF" || le32(4) != 36+10 || le32(24) != 16_000 || le16(22) != 1 || le16(34) != 16 || le32(40) != 10 {
		t.Fatal(b[:44])
	}
	got, err := readLiveWav(path)
	if err != nil || !reflect.DeepEqual(got, []float32{0, 0.5, -0.5, 32767.0 / 32768.0, -1}) {
		t.Fatal(got, err)
	}
	if runtime.GOOS != "windows" {
		if fi, _ := os.Stat(path); fi.Mode().Perm() != 0o600 {
			t.Fatal("a recording is for this user only", fi.Mode())
		}
	}
}

func TestClientSamplesGoBackToTheSamePCM(t *testing.T) {
	pcm := []int16{0, 1, -1, 12345, 32767, -32768}
	floats, err := DecodeF32(protocol.EncodeI16(pcm))
	if err != nil {
		t.Fatal(err)
	}
	for i, f := range floats {
		if liveToI16(f) != pcm[i] {
			t.Fatal(i, f)
		}
	}
}

// ---- queue ----

func TestTheQueueGivesItsItemsInOrderThenTheEnd(t *testing.T) {
	q := newLiveQueue[int]()
	done := make(chan []int)
	go func() {
		var got []int
		for {
			v, ok := q.pop()
			if !ok {
				done <- got
				return
			}
			got = append(got, v)
		}
	}()
	for i := 0; i < 1000; i++ {
		q.push(i)
	}
	q.close()
	if q.push(1000) {
		t.Fatal("a push after close")
	}
	got := <-done
	if len(got) != 1000 || got[0] != 0 || got[999] != 999 {
		t.Fatal(len(got))
	}
}
