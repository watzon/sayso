package localengine

import (
	"encoding/json"
	"reflect"
	"slices"
	"testing"
)

func mustSpec(t *testing.T, raw string) *modelSpec {
	t.Helper()
	s, err := parseSpec(json.RawMessage(raw))
	if err != nil {
		t.Fatalf("%s: %v", raw, err)
	}
	return s
}

func refuses(t *testing.T, raws ...string) {
	t.Helper()
	for _, raw := range raws {
		if _, err := parseSpec(json.RawMessage(raw)); err == nil {
			t.Errorf("%s parsed", raw)
		}
	}
}

func TestParsesASherpaEngine(t *testing.T) {
	s := mustSpec(t, `{"kind":"sherpa","recipe":"parakeet_tdt","archive":"a-int8","preview_archive":"p"}`)
	if s.recipe != recipeParakeetTdt || !reflect.DeepEqual(s.archives(), []string{"a-int8", "p"}) || s.streamArchive() != "p" || !s.recipe.finalPass() {
		t.Fatalf("%+v", s)
	}
	z := mustSpec(t, `{"kind":"sherpa","recipe":"zipformer_streaming","archive":"z","preview_archive":null}`)
	if !reflect.DeepEqual(z.archives(), []string{"z"}) || z.streamArchive() != "z" || z.recipe.finalPass() {
		t.Fatalf("%+v", z)
	}
}

func TestParsesAParakeetCppEngine(t *testing.T) {
	s := mustSpec(t, `{"kind":"parakeet_cpp","repo":"o/r","revision":"c0ffee","file":"m.gguf","preview_archive":"p"}`)
	if s.recipe != recipeParakeetGguf || s.source != (source{kind: sourceHuggingFace, repo: "o/r", file: "m.gguf"}) ||
		!reflect.DeepEqual(s.archives(), []string{"c0ffee", "p"}) || s.streamArchive() != "p" {
		t.Fatalf("%+v", s)
	}
}

func TestParsesAWhisperCppEngine(t *testing.T) {
	s := mustSpec(t, `{"kind":"whisper_cpp","file":"ggml-tiny.bin"}`)
	if s.recipe != recipeWhisperCpp || s.source != (source{kind: sourceHuggingFace, repo: whisperCppRepo, file: "ggml-tiny.bin"}) ||
		!reflect.DeepEqual(s.archives(), []string{whisperCppRevision}) || !s.recipe.finalPass() || !s.recipe.singleFile() || s.hasPreview() {
		t.Fatalf("%+v", s)
	}
	refuses(t, `{"kind":"whisper_cpp","file":"../ggml-tiny.bin"}`, `{"kind":"whisper_cpp","file":"a/b.bin"}`, `{"kind":"whisper_cpp","file":""}`)
}

func TestParsesAVADEngine(t *testing.T) {
	s := mustSpec(t, `{"kind":"sherpa_vad","recipe":"silero","file":"silero_vad.onnx"}`)
	if s.recipe != recipeSileroVad || s.source != (source{kind: sourceSherpaFile, file: "silero_vad.onnx"}) ||
		!reflect.DeepEqual(s.archives(), []string{"silero_vad"}) || s.recipe.task() != taskVad || s.recipe.finalPass() || s.hasPreview() || s.embeddingArchive() != "" {
		t.Fatalf("%+v", s)
	}
	ten := mustSpec(t, `{"kind":"sherpa_vad","recipe":"ten","file":"ten-vad.onnx"}`)
	if ten.recipe != recipeTenVad || !reflect.DeepEqual(ten.archives(), []string{"ten-vad"}) {
		t.Fatalf("%+v", ten)
	}
	refuses(t,
		`{"kind":"sherpa_vad","recipe":"nope","file":"a.onnx"}`,
		`{"kind":"sherpa_vad","recipe":"silero"}`,
		`{"kind":"sherpa_vad","file":"a.onnx"}`,
		`{"kind":"sherpa_vad","recipe":"silero","file":"../a.onnx"}`,
		`{"kind":"sherpa_vad","recipe":"silero","file":7}`)
}

func TestParsesADiarizationEngine(t *testing.T) {
	s := mustSpec(t, `{"kind":"sherpa_diarization","segmentation":"sherpa-onnx-pyannote-segmentation-3-0","embedding":"3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"}`)
	if s.recipe != recipeDiarization || s.recipe.task() != taskDiarization || s.archive != "sherpa-onnx-pyannote-segmentation-3-0" ||
		s.embeddingArchive() != "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced" ||
		!reflect.DeepEqual(s.archives(), []string{"sherpa-onnx-pyannote-segmentation-3-0", "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced"}) ||
		s.recipe.finalPass() || s.hasPreview() {
		t.Fatalf("%+v", s)
	}
	// Any file name works for the embedding model, with or without .onnx.
	if o := mustSpec(t, `{"kind":"sherpa_diarization","segmentation":"s","embedding":"model"}`); !reflect.DeepEqual(o.archives(), []string{"s", "model"}) {
		t.Fatalf("%v", o.archives())
	}
	refuses(t,
		`{"kind":"sherpa_diarization","segmentation":"s"}`,
		`{"kind":"sherpa_diarization","embedding":"e.onnx"}`,
		`{"kind":"sherpa_diarization","segmentation":"../s","embedding":"e.onnx"}`,
		`{"kind":"sherpa_diarization","segmentation":"s","embedding":"a/e.onnx"}`,
		`{"kind":"sherpa_diarization","segmentation":"e","embedding":"e.onnx"}`)
}

func TestParsesARedecodePreview(t *testing.T) {
	s := mustSpec(t, `{"kind":"sherpa","recipe":"parakeet_tdt","archive":"a","preview_archive":null,"redecode_preview":true}`)
	if !s.redecodePreview || !s.hasPreview() || s.streamArchive() != "" || !reflect.DeepEqual(s.archives(), []string{"a"}) {
		t.Fatalf("%+v", s)
	}
	if p := mustSpec(t, `{"kind":"sherpa","recipe":"parakeet_tdt","archive":"a","preview_archive":null}`); p.redecodePreview || p.hasPreview() {
		t.Fatalf("%+v", p)
	}
	refuses(t,
		`{"kind":"sherpa","recipe":"parakeet_tdt","archive":"a","preview_archive":"p","redecode_preview":true}`,
		`{"kind":"sherpa","recipe":"zipformer_streaming","archive":"a","preview_archive":null,"redecode_preview":true}`)
}

func TestTheCatalogSerializesToSpecsThisEngineReads(t *testing.T) {
	for _, goos := range []string{"linux", "windows"} {
		c, err := catalogFor(goos)
		if err != nil {
			t.Fatal(err)
		}
		var roles struct {
			Models []struct {
				ID    string   `json:"id"`
				Roles []string `json:"roles"`
			} `json:"models"`
		}
		if err := json.Unmarshal(c.raw, &roles); err != nil {
			t.Fatal(err)
		}
		for i, m := range c.models {
			s, err := parseSpec(m.Engine)
			if err != nil {
				t.Fatalf("%s %s: %v", goos, m.ID, err)
			}
			r := roles.Models[i].Roles
			if s.hasPreview() != slices.Contains(r, "streaming") || s.recipe.finalPass() != slices.Contains(r, "final") {
				t.Errorf("%s %s: preview %v final %v roles %v", goos, m.ID, s.hasPreview(), s.recipe.finalPass(), r)
			}
			want := taskSpeech
			if slices.Contains(r, "vad") {
				want = taskVad
			} else if slices.Contains(r, "diarization") {
				want = taskDiarization
			}
			if s.recipe.task() != want {
				t.Errorf("%s %s: task %v", goos, m.ID, s.recipe.task())
			}
		}
	}
}

func TestRejectsOtherKindsAndUnsafeNames(t *testing.T) {
	if _, err := parseSpec(json.RawMessage(`{"kind":"whisper","variant":"tiny"}`)); err == nil || err.Error() != "engine kind whisper does not run in this engine" {
		t.Fatalf("%v", err)
	}
	refuses(t, `{"kind":"nope"}`)
	for _, bad := range []string{"../x", "a/b", ".hidden", ""} {
		refuses(t, `{"kind":"sherpa","recipe":"whisper","archive":"`+bad+`","preview_archive":null}`)
	}
	for _, b := range [][3]string{{"o", "c0ffee", "m.gguf"}, {"o/r/x", "c0ffee", "m.gguf"}, {"../r", "c0ffee", "m.gguf"}, {"o/r", "c0ffee", "../m.gguf"}, {"o/r", "../main", "m.gguf"}, {"o/r", "", "m.gguf"}} {
		refuses(t, `{"kind":"parakeet_cpp","repo":"`+b[0]+`","revision":"`+b[1]+`","file":"`+b[2]+`","preview_archive":null}`)
	}
	if _, err := parseSpec(json.RawMessage(`{"kind":"sherpa","recipe":"nope","archive":"a","preview_archive":null}`)); err == nil || err.Error() != "unknown sherpa recipe nope" {
		t.Fatalf("%v", err)
	}
}
