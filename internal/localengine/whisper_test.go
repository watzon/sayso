package localengine

import (
	"path/filepath"
	"testing"
	"unsafe"
)

func TestWhisperOptions(t *testing.T) {
	for in, want := range map[string]string{"auto": "", "": "", "de": "de", "en-US": "en", " EN_gb ": "en"} {
		if got := whisperLanguage(in); got != want {
			t.Errorf("whisperLanguage(%q) = %q, want %q", in, got, want)
		}
	}
	if got := whisperPrompt(nil); got != "" {
		t.Errorf("%q", got)
	}
	if got := whisperPrompt([]string{"Sayso", " ", "Kubernetes"}); got != "Sayso, Kubernetes" {
		t.Errorf("%q", got)
	}
}

func TestWhisperTagsGoAway(t *testing.T) {
	for in, want := range map[string]string{
		" [BLANK_AUDIO]":       "",
		"Hello [MUSIC] world.": "Hello world.",
		"See [note 1] here":    "See [note 1] here",
		"open [bracket":        "open [bracket",
	} {
		if got := cleanWhisperText(in); got != want {
			t.Errorf("clean(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestSilenceIsNearZero(t *testing.T) {
	if !whisperIsSilence(make([]float32, 100)) || !whisperIsSilence(nil) || whisperIsSilence(filled(100, 0.01)) {
		t.Fatal("silence check")
	}
}

func TestShortAudioIsPaddedTo1_25s(t *testing.T) {
	if n := len(whisperPadded(make([]float32, 100))); n != 20_000 {
		t.Fatalf("%d", n)
	}
	if n := len(whisperPadded(make([]float32, 30_000))); n != 30_000 {
		t.Fatalf("%d", n)
	}
}

func TestAMissingWhisperFileIsAnErrorNotACrash(t *testing.T) {
	if _, err := loadWhisperCpp(filepath.Join(t.TempDir(), "ggml-tiny.bin")); err == nil {
		t.Fatal("no error")
	}
}

func TestTheBestGPUIsDedicatedThenLargest(t *testing.T) {
	if _, ok := bestGPU(nil); ok {
		t.Fatal("a GPU in an empty list")
	}
	i, ok := bestGPU([]gpuInfo{{false, 16 << 30}, {true, 4 << 30}, {true, 8 << 30}, {true, 8 << 30}})
	if !ok || i != 2 {
		t.Fatalf("%d", i)
	}
	if !firstGPUIsDedicated([]int32{0, ggmlDeviceGPU, ggmlDeviceIGPU}) || firstGPUIsDedicated([]int32{ggmlDeviceIGPU, ggmlDeviceGPU}) || firstGPUIsDedicated(nil) {
		t.Fatal("parakeet GPU check")
	}
}

// Offsets of whisper.h v1.8.3 on 64-bit targets, worked out from the C rules
// (bool 1, int and float 4, pointers and size_t 8, natural alignment).
func TestWhisperStructsMatchTheCLayout(t *testing.T) {
	var p whisperFullParams
	var c whisperContextParams
	for name, got := range map[string][2]uintptr{
		"full.n_threads":        {unsafe.Offsetof(p.NThreads), 4},
		"full.translate":        {unsafe.Offsetof(p.Translate), 20},
		"full.token_timestamps": {unsafe.Offsetof(p.TokenTimestamps), 28},
		"full.suppress_regex":   {unsafe.Offsetof(p.SuppressRegex), 64},
		"full.initial_prompt":   {unsafe.Offsetof(p.InitialPrompt), 72},
		"full.language":         {unsafe.Offsetof(p.Language), 104},
		"full.suppress_blank":   {unsafe.Offsetof(p.SuppressBlank), 113},
		"full.suppress_nst":     {unsafe.Offsetof(p.SuppressNst), 114},
		"full.no_speech_thold":  {unsafe.Offsetof(p.NoSpeechThold), 140},
		"full.beam_size":        {unsafe.Offsetof(p.BeamSize), 148},
		"full.new_segment_cb":   {unsafe.Offsetof(p.NewSegmentCb), 160},
		"full.grammar_penalty":  {unsafe.Offsetof(p.GrammarPenalty), 264},
		"full.vad_model_path":   {unsafe.Offsetof(p.VadModelPath), 272},
		"full.vad_params":       {unsafe.Offsetof(p.VadParams), 280},
		"full size":             {unsafe.Sizeof(p), 304},
		"ctx.gpu_device":        {unsafe.Offsetof(c.GpuDevice), 4},
		"ctx.dtw_aheads":        {unsafe.Offsetof(c.DtwAheads), 24},
		"ctx size":              {unsafe.Sizeof(c), 48},
	} {
		if got[0] != got[1] {
			t.Errorf("%s: %d, C has %d", name, got[0], got[1])
		}
	}
}

func TestWhisperOptionsMatchTheRustEngine(t *testing.T) {
	var p whisperFullParams
	lang, prompt := new(byte), new(byte)
	setWhisperOptions(&p, 6, lang, prompt)
	if p.Strategy != whisperSamplingBeamSearch || p.BeamSize != 3 || p.BeamPatience != -1 || p.NThreads != 6 ||
		p.Language != lang || p.InitialPrompt != prompt || !p.SuppressBlank || !p.SuppressNst || !p.NoContext ||
		p.NoSpeechThold != 0.2 || p.Translate || p.PrintProgress || p.PrintRealtime || p.PrintSpecial || p.PrintTimestamps {
		t.Fatalf("%+v", p)
	}
	setWhisperOptions(&p, 6, nil, nil)
	if p.Language != nil {
		t.Fatal("auto must be a NULL language")
	}
}
