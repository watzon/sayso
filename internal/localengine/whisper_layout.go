package localengine

import "unsafe"

// The structs of whisper.h in whisper.cpp v1.8.3 (whisper-rs-sys 0.15.0), the
// version the Rust engine links. Go lays these field types out like C on the
// 64-bit targets the engine ships for; whisper_layout_test.go pins the
// offsets that the binding writes.

const (
	whisperSamplingGreedy     = 0
	whisperSamplingBeamSearch = 1
)

type whisperAheads struct {
	NHeads uintptr
	Heads  unsafe.Pointer
}

type whisperContextParams struct {
	UseGpu             bool
	FlashAttn          bool
	GpuDevice          int32
	DtwTokenTimestamps bool
	DtwAheadsPreset    int32
	DtwNTop            int32
	DtwAheads          whisperAheads
	DtwMemSize         uintptr
}

type whisperVadParams struct {
	Threshold            float32
	MinSpeechDurationMs  int32
	MinSilenceDurationMs int32
	MaxSpeechDurationS   float32
	SpeechPadMs          int32
	SamplesOverlap       float32
}

type whisperFullParams struct {
	Strategy        int32
	NThreads        int32
	NMaxTextCtx     int32
	OffsetMs        int32
	DurationMs      int32
	Translate       bool
	NoContext       bool
	NoTimestamps    bool
	SingleSegment   bool
	PrintSpecial    bool
	PrintProgress   bool
	PrintRealtime   bool
	PrintTimestamps bool
	TokenTimestamps bool
	TholdPt         float32
	TholdPtsum      float32
	MaxLen          int32
	SplitOnWord     bool
	MaxTokens       int32
	DebugMode       bool
	AudioCtx        int32
	TdrzEnable      bool

	SuppressRegex      *byte
	InitialPrompt      *byte
	CarryInitialPrompt bool
	PromptTokens       *int32
	PromptNTokens      int32
	Language           *byte
	DetectLanguage     bool
	SuppressBlank      bool
	SuppressNst        bool

	Temperature    float32
	MaxInitialTs   float32
	LengthPenalty  float32
	TemperatureInc float32
	EntropyThold   float32
	LogprobThold   float32
	NoSpeechThold  float32

	GreedyBestOf     int32
	BeamSize         int32
	BeamPatience     float32
	NewSegmentCb     uintptr
	NewSegmentCbData uintptr
	ProgressCb       uintptr
	ProgressCbData   uintptr
	EncoderBeginCb   uintptr
	EncoderBeginData uintptr
	AbortCb          uintptr
	AbortCbData      uintptr
	LogitsFilterCb   uintptr
	LogitsFilterData uintptr
	GrammarRules     uintptr
	NGrammarRules    uintptr
	IStartRule       uintptr
	GrammarPenalty   float32
	Vad              bool
	VadModelPath     *byte
	VadParams        whisperVadParams
}

// setWhisperOptions sets the parameters of transcribe-rs 0.3.12
// (WhisperEngine::transcribe_with), as the Rust engine does, on params that
// whisper_full_default_params_by_ref(BEAM_SEARCH) made. language and prompt
// are NUL-terminated or nil, and must outlive the decode.
func setWhisperOptions(p *whisperFullParams, threads int32, language, prompt *byte) {
	p.Strategy = whisperSamplingBeamSearch
	p.BeamSize = 3
	p.BeamPatience = -1
	p.Language = language
	p.DetectLanguage = false
	p.Translate = false
	p.PrintSpecial = false
	p.PrintProgress = false
	p.PrintRealtime = false
	p.PrintTimestamps = false
	p.SuppressBlank = true
	p.SuppressNst = true
	p.NoSpeechThold = 0.2
	p.NoContext = true
	p.NThreads = threads
	if prompt != nil {
		p.InitialPrompt = prompt
	}
}
