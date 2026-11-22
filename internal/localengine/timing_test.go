package localengine

import (
	"encoding/json"
	"reflect"
	"testing"
)

func texts(words []word) []string {
	var out []string
	for _, w := range words {
		out = append(out, w.Text)
	}
	return out
}

func segmentTexts(segments []textSegment) []string {
	var out []string
	for _, s := range segments {
		out = append(out, s.Text)
	}
	return out
}

func TestWordPiecesAndPunctuationJoinTheirWord(t *testing.T) {
	tokens := []string{" Hel", "lo", " there", ".", " How", " are", " you", "?"}
	starts := []float32{0.32, 0.48, 0.70, 1.10, 1.90, 2.30, 2.60, 2.90}
	words := wordsFromTokens(tokens, starts, nil, 3_500)
	if got := texts(words); !reflect.DeepEqual(got, []string{"Hello", "there.", "How", "are", "you?"}) {
		t.Fatalf("%q", got)
	}
	if words[0].StartMs != 320 || words[0].EndMs != 700 || words[1].EndMs != 1_900 {
		t.Fatalf("%+v", words[:2])
	}
	if words[4].EndMs != 3_300 {
		t.Fatalf("the last token gets 400 ms: %d", words[4].EndMs)
	}
}

func TestSentencepieceMarksAndDurationsWork(t *testing.T) {
	words := wordsFromTokens([]string{"▁Hi", "▁all"}, []float32{0.1, 0.5}, []float32{0.2, 0.3}, 2_000)
	want := []word{{100, 300, "Hi"}, {500, 800, "all"}}
	if !reflect.DeepEqual(words, want) {
		t.Fatalf("%+v", words)
	}
}

func TestALoneSpaceTokenStartsTheNextWord(t *testing.T) {
	words := wordsFromTokens([]string{"▁", "Hi", "▁", "all"}, []float32{0, 0.1, 0.3, 0.4}, nil, 1_000)
	if got := texts(words); !reflect.DeepEqual(got, []string{"Hi", "all"}) {
		t.Fatalf("%q", got)
	}
}

func TestEachCJKCharacterIsAWordAndJoinsWithoutSpaces(t *testing.T) {
	words := wordsFromTokens([]string{"你", "好", "。", "再", "见"}, []float32{0, 0.2, 0.4, 0.5, 0.7}, nil, 1_000)
	if got := texts(words); !reflect.DeepEqual(got, []string{"你", "好。", "再", "见"}) {
		t.Fatalf("%q", got)
	}
	if got := segmentTexts(segmentsFromWords(words)); !reflect.DeepEqual(got, []string{"你好。", "再见"}) {
		t.Fatalf("%q", got)
	}
}

func TestSegmentsEndAtSentencesPausesAndTwentySeconds(t *testing.T) {
	words := []word{{0, 300, "One."}, {400, 700, "two"}, {1_500, 1_800, "three"}, {1_900, 21_600, "long"}, {21_700, 22_000, "four"}}
	if got := segmentTexts(segmentsFromWords(words)); !reflect.DeepEqual(got, []string{"One.", "two", "three", "long", "four"}) {
		t.Fatalf("%q", got)
	}
}

func TestPiecesMoveToTheTimeOfTheRecording(t *testing.T) {
	pieces := []piece{
		{0, 10_000, timed{text: "a b.", kind: timingWords, words: []word{{100, 200, "a"}, {300, 400, "b."}}}},
		{10_000, 20_000, untimed("rest")},
		{20_000, 25_000, untimed("  ")},
		{25_000, 30_000, timed{text: "x", kind: timingSegments, segments: []textSegment{{StartMs: 1_000, EndMs: 9_000, Text: " x "}}}},
	}
	all := allSegments(pieces)
	if len(all) != 3 {
		t.Fatalf("the empty piece is left out: %+v", all)
	}
	type span struct {
		s, e uint64
		text string
	}
	got := []span{{all[0].StartMs, all[0].EndMs, all[0].Text}, {all[1].StartMs, all[1].EndMs, all[1].Text}, {all[2].StartMs, all[2].EndMs, all[2].Text}}
	want := []span{{100, 400, "a b."}, {10_000, 20_000, "rest"}, {26_000, 30_000, "x"}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("%+v", got)
	}
	raw, _ := json.Marshal(all)
	var decoded []map[string]any
	if err := json.Unmarshal(raw, &decoded); err != nil {
		t.Fatal(err)
	}
	if decoded[0]["words"].([]any)[1].(map[string]any)["start_ms"].(float64) != 300 {
		t.Fatalf("%s", raw)
	}
	if _, ok := decoded[1]["words"]; ok {
		t.Fatalf("untimed piece has words: %s", raw)
	}
}

func TestSegmentsNeverOverlap(t *testing.T) {
	all := allSegments([]piece{{0, 5_000, timed{text: "a. b.", kind: timingSegments, segments: []textSegment{
		{StartMs: 0, EndMs: 2_000, Text: "a."}, {StartMs: 1_500, EndMs: 1_800, Text: "b."},
	}}}})
	if all[1].StartMs != 2_000 || all[1].EndMs != 2_000 {
		t.Fatalf("%+v", all[1])
	}
}
