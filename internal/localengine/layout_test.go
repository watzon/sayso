package localengine

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func modelFolder(t *testing.T, names ...string) string {
	t.Helper()
	dir := t.TempDir()
	for _, n := range names {
		if err := os.WriteFile(filepath.Join(dir, n), []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Mkdir(filepath.Join(dir, "test_wavs"), 0o755); err != nil {
		t.Fatal(err)
	}
	return dir
}

func base(p string) string { return filepath.Base(p) }

func TestParakeetPrefersInt8AndFallsBackToCTC(t *testing.T) {
	f, err := findOfflineFiles(recipeParakeetTdt, modelFolder(t, "encoder.onnx", "encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"))
	if err != nil || f.kind != offlineTransducer || base(f.encoder) != "encoder.int8.onnx" {
		t.Fatalf("%+v %v", f, err)
	}
	f, err = findOfflineFiles(recipeParakeetTdtCtc, modelFolder(t, "model.int8.onnx", "tokens.txt"))
	if err != nil || f.kind != offlineNemoCtc {
		t.Fatalf("%+v %v", f, err)
	}
}

func TestWhisperSenseVoiceAndMoonshine(t *testing.T) {
	f, err := findOfflineFiles(recipeWhisper, modelFolder(t, "tiny-encoder.onnx", "tiny-encoder.int8.onnx", "tiny-decoder.onnx", "tiny-decoder.int8.onnx", "tiny-tokens.txt"))
	if err != nil || f.kind != offlineWhisper || base(f.decoder) != "tiny-decoder.int8.onnx" || base(f.tokens) != "tiny-tokens.txt" {
		t.Fatalf("whisper %+v %v", f, err)
	}
	if f, err := findOfflineFiles(recipeSenseVoice, modelFolder(t, "model.int8.onnx", "tokens.txt")); err != nil || f.kind != offlineSenseVoice {
		t.Fatalf("sensevoice %+v %v", f, err)
	}
	if f, err := findOfflineFiles(recipeMoonshine, modelFolder(t, "encoder_model.ort", "decoder_model_merged.ort", "tokens.txt", "LICENSE")); err != nil || f.kind != offlineMoonshineV2 {
		t.Fatalf("moonshine v2 %+v %v", f, err)
	}
	f, err = findOfflineFiles(recipeMoonshine, modelFolder(t, "preprocess.onnx", "encode.int8.onnx", "uncached_decode.int8.onnx", "cached_decode.int8.onnx", "tokens.txt"))
	if err != nil || f.kind != offlineMoonshineV1 || base(f.uncachedDecoder) != "uncached_decode.int8.onnx" || base(f.cachedDecoder) != "cached_decode.int8.onnx" {
		t.Fatalf("moonshine v1 %+v %v", f, err)
	}
}

func TestPathsListsEveryFileOfTheModel(t *testing.T) {
	f, err := findOfflineFiles(recipeWhisper, modelFolder(t, "encoder.onnx", "decoder.onnx", "tokens.txt"))
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, p := range f.paths() {
		names = append(names, base(p))
	}
	if !reflect.DeepEqual(names, []string{"encoder.onnx", "decoder.onnx", "tokens.txt"}) {
		t.Fatalf("%q", names)
	}
}

func TestStreamingFilesAndMissingFiles(t *testing.T) {
	f, err := findOnlineFiles(modelFolder(t, "encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt", "README.md"))
	if err != nil || base(f.joiner) != "joiner.onnx" {
		t.Fatalf("%+v %v", f, err)
	}
	_, err = findOnlineFiles(modelFolder(t, "encoder.onnx", "tokens.txt"))
	if err == nil || !strings.HasPrefix(err.Error(), "no decoder model file") {
		t.Fatalf("%v", err)
	}
	if _, err := findOfflineFiles(recipeSenseVoice, modelFolder(t, "model.onnx")); err == nil {
		t.Fatal("no tokens, no error")
	}
}

func TestVADAndDiarizationFiles(t *testing.T) {
	dir := modelFolder(t, "silero_vad.onnx")
	if p, err := vadFile(dir, "silero_vad.onnx"); err != nil || base(p) != "silero_vad.onnx" {
		t.Fatalf("%q %v", p, err)
	}
	if _, err := vadFile(dir, "ten-vad.onnx"); err == nil {
		t.Fatal("a file that is not there")
	}
	if err := os.WriteFile(filepath.Join(dir, "empty.onnx"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := vadFile(dir, "empty.onnx"); err == nil || !strings.HasSuffix(err.Error(), "is empty") {
		t.Fatalf("%v", err)
	}
	seg := modelFolder(t, "model.int8.onnx", "model.onnx", "LICENSE")
	emb := modelFolder(t, "speaker.onnx")
	f, err := findDiarizationFiles(seg, emb, "speaker.onnx")
	if err != nil || base(f.segmentation) != "model.onnx" || base(f.embedding) != "speaker.onnx" {
		t.Fatalf("%+v %v", f, err)
	}
	f, err = findDiarizationFiles(modelFolder(t, "model.int8.onnx"), emb, "speaker.onnx")
	if err != nil || base(f.segmentation) != "model.int8.onnx" {
		t.Fatalf("%+v %v", f, err)
	}
	if _, err := findDiarizationFiles(seg, emb, "other.onnx"); err == nil {
		t.Fatal("missing embedding")
	}
	if _, err := findDiarizationFiles(modelFolder(t, "LICENSE"), emb, "speaker.onnx"); err == nil {
		t.Fatal("missing segmentation")
	}
}
