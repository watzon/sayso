package localengine

import (
	"reflect"
	"testing"
)

func runTracker(samples []float32, chunk int) []vadEvent {
	tr := newTracker(&energyDetector{})
	var events []vadEvent
	for len(samples) > 0 {
		n := min(chunk, len(samples))
		events = append(events, tr.accept(samples[:n])...)
		samples = samples[n:]
	}
	return append(events, tr.finish()...)
}

func near(actual, expected uint64) bool {
	if actual > expected {
		return actual-expected <= 100
	}
	return expected-actual <= 100
}

var q, l = false, true

func TestSpeechWithALongPauseIsTwoParts(t *testing.T) {
	found := speechParts(runTracker(audioOf(audioPiece{1000, q}, audioPiece{2000, l}, audioPiece{1500, q}, audioPiece{1000, l}, audioPiece{1000, q}), 2560))
	if len(found) != 2 || !near(found[0][0], 1000) || !near(found[0][1], 3000) || !near(found[1][0], 4500) || !near(found[1][1], 5500) {
		t.Fatalf("%v", found)
	}
}

func TestAShortPauseDoesNotEndSpeechAndShortSpeechIsLeftOut(t *testing.T) {
	found := speechParts(runTracker(audioOf(audioPiece{500, q}, audioPiece{1000, l}, audioPiece{300, q}, audioPiece{1000, l}, audioPiece{800, q}, audioPiece{200, l}, audioPiece{1000, q}), 2560))
	if len(found) != 1 || !near(found[0][0], 500) || !near(found[0][1], 2800) {
		t.Fatalf("one part, and the 200 ms are left out: %v", found)
	}
}

func TestSilenceAndNoiseWithoutSpeechGiveNoPart(t *testing.T) {
	for _, s := range [][]float32{audioOf(audioPiece{3000, q}), nil, audioOf(audioPiece{100, l})} {
		if found := speechParts(runTracker(s, 2560)); len(found) != 0 {
			t.Fatalf("%v", found)
		}
	}
}

func TestEventsDoNotDependOnTheChunkSize(t *testing.T) {
	samples := audioOf(audioPiece{700, q}, audioPiece{3000, l}, audioPiece{900, q}, audioPiece{40_000, l}, audioPiece{1000, q}, audioPiece{400, l})
	all := runTracker(samples, len(samples))
	for _, chunk := range []int{1, 160, 2560, 3333, 16_000} {
		if got := runTracker(samples, chunk); !reflect.DeepEqual(got, all) {
			t.Fatalf("chunk of %d samples: %v, want %v", chunk, got, all)
		}
	}
}

func TestLongSpeechIsCutIntoPartsOfAtMost30sThatTouch(t *testing.T) {
	found := speechParts(runTracker(audioOf(audioPiece{1000, q}, audioPiece{75_000, l}, audioPiece{1000, q}), 2560))
	if len(found) != 3 || !near(found[0][0], 1000) || found[0][1]-found[0][0] != 30_000 ||
		found[1] != [2]uint64{found[0][1], found[0][1] + 30_000} || found[2][0] != found[1][1] || !near(found[2][1], 76_000) {
		t.Fatalf("%v", found)
	}
}

func TestSpeechThatEndsJustAfter30sIsTwoPartsWithinTheRules(t *testing.T) {
	for _, ms := range []int{30_100, 30_400, 30_700} {
		found := speechParts(runTracker(audioOf(audioPiece{1000, q}, audioPiece{ms, l}, audioPiece{2000, q}), 2560))
		if len(found) != 2 || found[0][1] != found[1][0] {
			t.Fatalf("%d ms: %v", ms, found)
		}
		for _, p := range found {
			if p[1]-p[0] > 30_000 || p[1]-p[0] < 250 {
				t.Fatalf("%d ms: %v", ms, found)
			}
		}
	}
}

func TestASessionReportsEachStartBeforeItsEndAndEveryEndByTheFinish(t *testing.T) {
	samples := audioOf(audioPiece{500, q}, audioPiece{2000, l}, audioPiece{1000, q}, audioPiece{1000, l})
	tr := newTracker(&energyDetector{})
	var events []vadEvent
	for len(samples) > 0 {
		n := min(2560, len(samples))
		events = append(events, tr.accept(samples[:n])...)
		samples = samples[n:]
	}
	// The first part ended, and the second part has begun. Its end waits for a pause.
	if len(events) != 3 || events[0].isEnd || !events[1].isEnd || events[2].isEnd {
		t.Fatalf("%v", events)
	}
	rest := tr.finish()
	if len(rest) != 1 || !rest[0].isEnd || rest[0].endMs <= rest[0].startMs || rest[0].endMs > 4500 {
		t.Fatalf("the end is not after the audio: %v", rest)
	}
}

func TestSpeechThatRunsToTheEndOfTheAudioEndsInsideIt(t *testing.T) {
	found := speechParts(runTracker(audioOf(audioPiece{500, q}, audioPiece{2000, l}), 2560))
	if len(found) != 1 || found[0][1] > 2500 || !near(found[0][1], 2500) {
		t.Fatalf("%v", found)
	}
}

func TestEveryEventListIsOrderedAndWithoutOverlap(t *testing.T) {
	events := runTracker(audioOf(audioPiece{300, l}, audioPiece{900, q}, audioPiece{5000, l}, audioPiece{600, q}, audioPiece{35_000, l}, audioPiece{2000, q}, audioPiece{900, l}), 1000)
	open, hasOpen := uint64(0), false
	var lastEnd uint64
	for _, e := range events {
		if !e.isEnd {
			if hasOpen || e.startMs < lastEnd {
				t.Fatalf("start %d", e.startMs)
			}
			open, hasOpen = e.startMs, true
			continue
		}
		if !hasOpen || open != e.startMs || e.endMs <= e.startMs {
			t.Fatalf("end %+v", e)
		}
		hasOpen, lastEnd = false, e.endMs
	}
	if hasOpen {
		t.Fatal("a part is still open")
	}
}

func TestSpeakersAreNumberedInTheOrderTheyFirstSpeak(t *testing.T) {
	speakers, turns := finishDiarization([]rawTurn{{5, 6, 0}, {1, 2, 2}, {3, 4, 1}, {7, 8, 2}}, 10_000)
	var who []uint32
	var starts []uint64
	for _, tt := range turns {
		who, starts = append(who, tt.Speaker), append(starts, tt.StartMs)
	}
	if speakers != 3 || !reflect.DeepEqual(who, []uint32{0, 1, 2, 0}) || !reflect.DeepEqual(starts, []uint64{1000, 3000, 5000, 7000}) {
		t.Fatalf("%d %v %v", speakers, who, starts)
	}
}

func TestTurnsOfOneSpeakerThatOverlapOrTouchBecomeOne(t *testing.T) {
	speakers, turns := finishDiarization([]rawTurn{{0, 2, 0}, {1.5, 3, 0}, {3, 3.5, 0}, {4, 5, 0}, {2, 2.5, 1}}, 10_000)
	want := []turn{{0, 3500, 0}, {2000, 2500, 1}, {4000, 5000, 0}}
	if speakers != 2 || !reflect.DeepEqual(turns, want) {
		t.Fatalf("%d %v", speakers, turns)
	}
}

func TestATurnInsideALongerTurnOfTheSameSpeakerDoesNotShortenIt(t *testing.T) {
	_, turns := finishDiarization([]rawTurn{{0, 5, 3}, {1, 2, 3}}, 10_000)
	if !reflect.DeepEqual(turns, []turn{{0, 5000, 0}}) {
		t.Fatalf("%v", turns)
	}
}

func TestTimesAreWholeMillisecondsInsideTheAudioAndEmptyTurnsGo(t *testing.T) {
	speakers, turns := finishDiarization([]rawTurn{{0.0004, 1.2346, 0}, {2, 2, 1}, {9.5, 12, 0}, {-1, 0.5, 0}}, 10_000)
	var spans [][2]uint64
	for _, tt := range turns {
		spans = append(spans, [2]uint64{tt.StartMs, tt.EndMs})
	}
	if speakers != 1 || !reflect.DeepEqual(spans, [][2]uint64{{0, 1235}, {9500, 10_000}}) {
		t.Fatalf("%d %v", speakers, spans)
	}
}

func TestNoTurnsMeansNoSpeakers(t *testing.T) {
	speakers, turns := finishDiarization(nil, 10_000)
	if speakers != 0 || len(turns) != 0 {
		t.Fatalf("%d %v", speakers, turns)
	}
}
