package localengine

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestEqualLanguagesGetOneRecognizer(t *testing.T) {
	for _, same := range []string{"en", "en-US", "en_GB", "EN", " en "} {
		if got := recognizerLanguage(offlineWhisper, same); got != "en" {
			t.Errorf("%q: %q", same, got)
		}
	}
	for _, auto := range []string{"", "auto", "AUTO", "  "} {
		if got := recognizerLanguage(offlineWhisper, auto); got != "auto" {
			t.Errorf("%q: %q", auto, got)
		}
	}
	if recognizerLanguage(offlineWhisper, "zh-CN") != "zh" {
		t.Error("zh-CN")
	}
}

func TestSenseVoiceSharesTheAutoRecognizerForLanguagesItDoesNotKnow(t *testing.T) {
	if recognizerLanguage(offlineSenseVoice, "ja-JP") != "ja" || recognizerLanguage(offlineSenseVoice, "yue") != "yue" {
		t.Error("known codes")
	}
	for _, other := range []string{"de", "fr-CA", "", "auto"} {
		if got := recognizerLanguage(offlineSenseVoice, other); got != "auto" {
			t.Errorf("%q: %q", other, got)
		}
	}
}

func TestAModelWithoutALanguageHasOneRecognizer(t *testing.T) {
	for _, l := range []string{"en", "de", "", "auto"} {
		if recognizerLanguage(offlineNemoCtc, l) != "auto" {
			t.Errorf("%q", l)
		}
	}
}

func TestWhisperWithSegmentsHasItsOwnRecognizer(t *testing.T) {
	dictation, job := keyFor(offlineWhisper, "auto", false), keyFor(offlineWhisper, "", true)
	if dictation != (recognizerKey{"auto", false}) || job != (recognizerKey{"auto", true}) {
		t.Fatalf("%v %v", dictation, job)
	}
	if keyFor(offlineWhisper, "en-US", true) != (recognizerKey{"en", true}) {
		t.Fatal("en-US")
	}
	for _, k := range []offlineKind{offlineSenseVoice, offlineNemoCtc} {
		if keyFor(k, "auto", true) != keyFor(k, "auto", false) {
			t.Errorf("kind %d: other models share the recognizer with and without segments", k)
		}
	}
}

func TestWhisperSegmentsBecomeSegmentTimes(t *testing.T) {
	d := &decoded{Text: " One. Two. Three.", SegmentTimestamps: []float32{0, 5.16, 9.84}, SegmentDurations: []float32{5.16, 4.68, 3.12},
		SegmentTexts: []string{" One.", " Two.", " Three."}, hasSegmentTimes: true}
	got := timingOf(d, 12_000)
	want := []textSegment{{StartMs: 0, EndMs: 5_160, Text: "One."}, {StartMs: 5_160, EndMs: 9_840, Text: "Two."}, {StartMs: 9_840, EndMs: 12_000, Text: "Three."}}
	if got.kind != timingSegments || !reflect.DeepEqual(got.segments, want) {
		t.Fatalf("the last end stays in the audio: %+v", got)
	}
}

func TestTokenTimesBecomeWordTimesAndNoTimesIsNone(t *testing.T) {
	words := timingOf(&decoded{Text: "Guten Tag.", Tokens: []string{"▁Guten", "▁Tag."}, Timestamps: []float32{0.3, 0.66}}, 2_000)
	if words.kind != timingWords || len(words.words) != 2 {
		t.Fatalf("%+v", words)
	}
	if none := timingOf(&decoded{Text: "Hello.", Tokens: []string{" Hello"}, Timestamps: []float32{}}, 2_000); none.kind != timingNone {
		t.Fatalf("%+v", none)
	}
}

func TestAMissingOrEmptyFileFailsAtTheLoad(t *testing.T) {
	dir := t.TempDir()
	write := func(name, body string) string {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
		return p
	}
	if err := checkReadable(&offlineFiles{kind: offlineSenseVoice, model: write("model.onnx", "x"), tokens: write("tokens.txt", "x")}); err != nil {
		t.Fatal(err)
	}
	err := checkReadable(&offlineFiles{kind: offlineSenseVoice, model: write("model.onnx", ""), tokens: write("tokens.txt", "x")})
	if err == nil || !strings.HasSuffix(err.Error(), "is empty") {
		t.Fatalf("%v", err)
	}
	if checkReadable(&offlineFiles{kind: offlineSenseVoice, model: filepath.Join(dir, "gone.onnx"), tokens: write("tokens.txt", "x")}) == nil {
		t.Fatal("missing file passed")
	}
}

// The lang and segment keys of a real result: Whisper tiny (int8) with segment
// times, sherpa-onnx 1.13.8, on an English clip.
const whisperSegmentsJSON = `{"lang":"en","segment_timestamps":[0.00,5.16,9.84],"segment_durations":[5.16,4.68,3.12],"segment_texts":[" Hello, this is a test of the speech recognizer."," The quick brown fox jumps over the lazy dog."," Now we are speaking the third sentence after another pause."],"text":" Hello, this is a test of the speech recognizer. The quick brown fox jumps over the lazy dog. Now we are speaking the third sentence after another pause.","tokens":[" Hello",","," this"],"timestamps":[]}`

func TestWhisperSegmentsAndLanguageComeFromTheJSON(t *testing.T) {
	d, err := parseDecoded(whisperSegmentsJSON)
	if err != nil {
		t.Fatal(err)
	}
	if d.Lang != "en" || !strings.HasPrefix(d.Text, " Hello, this is a test") || !reflect.DeepEqual(d.Tokens, []string{" Hello", ",", " this"}) ||
		len(d.Timestamps) != 0 || !d.hasSegmentTimes || len(d.SegmentTexts) != 3 || d.SegmentTimestamps[1] != 5.16 || d.SegmentDurations[2] != 3.12 {
		t.Fatalf("%+v", d)
	}
}

func TestSenseVoiceGivesTheLanguageInTagsAndTokenTimes(t *testing.T) {
	d, err := parseDecoded(`{"lang":"<|de|>","emotion":"<|NEUTRAL|>","event":"<|Speech|>","text":"Guten Tag.","timestamps":[0.30,0.66],"durations":[],"tokens":["▁Guten","▁Tag."],"words":[]}`)
	if err != nil || d.Lang != "<|de|>" || !reflect.DeepEqual(d.Timestamps, []float32{0.30, 0.66}) || d.hasSegmentTimes {
		t.Fatalf("%+v %v", d, err)
	}
	if normalizeLanguage(d.Lang) != "de" {
		t.Fatal("tag not normalized")
	}
}

func TestMissingKeysAreEmptyAndSegmentsNeedStartsAndDurations(t *testing.T) {
	d, err := parseDecoded(`{"text":"","lang":""}`)
	if err != nil || d.Text != "" || d.hasSegmentTimes || len(d.Tokens) != 0 {
		t.Fatalf("%+v %v", d, err)
	}
	if _, err := parseDecoded("not json"); err == nil {
		t.Fatal("garbage parsed")
	}
	d, _ = parseDecoded(`{"text":"a","segment_timestamps":[0.0],"segment_texts":["a"]}`)
	if d.hasSegmentTimes || timingOf(d, 1000).kind != timingNone {
		t.Fatal("segments without durations")
	}
}
