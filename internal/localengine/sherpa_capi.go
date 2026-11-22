package localengine

import (
	"fmt"
	"runtime"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

// The C API of sherpa-onnx 1.13.8 (c-api.h), loaded at run time from
// sherpa-onnx-c-api.dll (Windows) or libsherpa-onnx-c-api.so (Linux). The
// structs follow c-api.h field by field; sherpa_capi_test.go checks their
// sizes and offsets. A zero field means the default of the C API, as the C
// examples make a config with memset.

type cString = *byte

type sherpaOfflineTransducerModelConfig struct{ Encoder, Decoder, Joiner cString }
type sherpaOfflineParaformerModelConfig struct{ Model cString }
type sherpaOfflineNemoEncDecCtcModelConfig struct{ Model cString }

type sherpaOfflineWhisperModelConfig struct {
	Encoder, Decoder, Language, Task cString
	TailPaddings                     int32
	EnableTokenTimestamps            int32
	EnableSegmentTimestamps          int32
}

type sherpaOfflineTdnnModelConfig struct{ Model cString }

type sherpaOfflineSenseVoiceModelConfig struct {
	Model, Language cString
	UseItn          int32
}

type sherpaOfflineMoonshineModelConfig struct {
	Preprocessor, Encoder, UncachedDecoder, CachedDecoder, MergedDecoder cString
}

type sherpaOfflineFireRedAsrModelConfig struct{ Encoder, Decoder cString }
type sherpaOfflineDolphinModelConfig struct{ Model cString }
type sherpaOfflineZipformerCtcModelConfig struct{ Model cString }

type sherpaOfflineCanaryModelConfig struct {
	Encoder, Decoder, SrcLang, TgtLang cString
	UsePnc                             int32
}

type sherpaOfflineWenetCtcModelConfig struct{ Model cString }
type sherpaOfflineOmnilingualAsrCtcModelConfig struct{ Model cString }
type sherpaOfflineMedAsrCtcModelConfig struct{ Model cString }

type sherpaOfflineFunASRNanoModelConfig struct {
	EncoderAdaptor, Llm, Embedding, Tokenizer, SystemPrompt, UserPrompt cString
	MaxNewTokens                                                        int32
	Temperature, TopP                                                   float32
	Seed                                                                int32
	Language                                                            cString
	Itn                                                                 int32
	Hotwords                                                            cString
}

type sherpaOfflineFireRedAsrCtcModelConfig struct{ Model cString }

type sherpaOfflineQwen3ASRModelConfig struct {
	ConvFrontend, Encoder, Decoder, Tokenizer cString
	MaxTotalLen, MaxNewTokens                 int32
	Temperature, TopP                         float32
	Seed                                      int32
	Hotwords                                  cString
}

type sherpaOfflineCohereTranscribeModelConfig struct {
	Encoder, Decoder, Language cString
	UsePunct, UseItn           int32
}

type sherpaOfflineModelConfig struct {
	Transducer       sherpaOfflineTransducerModelConfig
	Paraformer       sherpaOfflineParaformerModelConfig
	NemoCtc          sherpaOfflineNemoEncDecCtcModelConfig
	Whisper          sherpaOfflineWhisperModelConfig
	Tdnn             sherpaOfflineTdnnModelConfig
	Tokens           cString
	NumThreads       int32
	Debug            int32
	Provider         cString
	ModelType        cString
	ModelingUnit     cString
	BpeVocab         cString
	TeleSpeechCtc    cString
	SenseVoice       sherpaOfflineSenseVoiceModelConfig
	Moonshine        sherpaOfflineMoonshineModelConfig
	FireRedAsr       sherpaOfflineFireRedAsrModelConfig
	Dolphin          sherpaOfflineDolphinModelConfig
	ZipformerCtc     sherpaOfflineZipformerCtcModelConfig
	Canary           sherpaOfflineCanaryModelConfig
	WenetCtc         sherpaOfflineWenetCtcModelConfig
	Omnilingual      sherpaOfflineOmnilingualAsrCtcModelConfig
	MedAsr           sherpaOfflineMedAsrCtcModelConfig
	FunAsrNano       sherpaOfflineFunASRNanoModelConfig
	FireRedAsrCtc    sherpaOfflineFireRedAsrCtcModelConfig
	Qwen3Asr         sherpaOfflineQwen3ASRModelConfig
	CohereTranscribe sherpaOfflineCohereTranscribeModelConfig
}

type sherpaFeatureConfig struct{ SampleRate, FeatureDim int32 }

type sherpaOfflineLMConfig struct {
	Model cString
	Scale float32
}

type sherpaHomophoneReplacerConfig struct{ DictDir, Lexicon, RuleFsts cString }

type sherpaOfflineRecognizerConfig struct {
	FeatConfig     sherpaFeatureConfig
	ModelConfig    sherpaOfflineModelConfig
	LmConfig       sherpaOfflineLMConfig
	DecodingMethod cString
	MaxActivePaths int32
	HotwordsFile   cString
	HotwordsScore  float32
	RuleFsts       cString
	RuleFars       cString
	BlankPenalty   float32
	Hr             sherpaHomophoneReplacerConfig
}

type sherpaOnlineTransducerModelConfig struct{ Encoder, Decoder, Joiner cString }
type sherpaOnlineParaformerModelConfig struct{ Encoder, Decoder cString }
type sherpaOnlineZipformer2CtcModelConfig struct{ Model cString }
type sherpaOnlineNemoCtcModelConfig struct{ Model cString }
type sherpaOnlineToneCtcModelConfig struct{ Model cString }

type sherpaOnlineModelConfig struct {
	Transducer    sherpaOnlineTransducerModelConfig
	Paraformer    sherpaOnlineParaformerModelConfig
	Zipformer2Ctc sherpaOnlineZipformer2CtcModelConfig
	Tokens        cString
	NumThreads    int32
	Provider      cString
	Debug         int32
	ModelType     cString
	ModelingUnit  cString
	BpeVocab      cString
	TokensBuf     cString
	TokensBufSize int32
	NemoCtc       sherpaOnlineNemoCtcModelConfig
	TOneCtc       sherpaOnlineToneCtcModelConfig
}

type sherpaOnlineCtcFstDecoderConfig struct {
	Graph     cString
	MaxActive int32
}

type sherpaOnlineRecognizerConfig struct {
	FeatConfig              sherpaFeatureConfig
	ModelConfig             sherpaOnlineModelConfig
	DecodingMethod          cString
	MaxActivePaths          int32
	EnableEndpoint          int32
	Rule1MinTrailingSilence float32
	Rule2MinTrailingSilence float32
	Rule3MinUtteranceLength float32
	HotwordsFile            cString
	HotwordsScore           float32
	CtcFstDecoderConfig     sherpaOnlineCtcFstDecoderConfig
	RuleFsts                cString
	RuleFars                cString
	BlankPenalty            float32
	HotwordsBuf             cString
	HotwordsBufSize         int32
	Hr                      sherpaHomophoneReplacerConfig
}

type sherpaVadDetectorConfig struct {
	Model              cString
	Threshold          float32
	MinSilenceDuration float32
	MinSpeechDuration  float32
	WindowSize         int32
	MaxSpeechDuration  float32
}

type sherpaVadModelConfig struct {
	SileroVad  sherpaVadDetectorConfig
	SampleRate int32
	NumThreads int32
	Provider   cString
	Debug      int32
	TenVad     sherpaVadDetectorConfig
}

type sherpaSpeechSegment struct {
	Start   int32
	Samples *float32
	N       int32
}

type sherpaPyannoteModelConfig struct {
	Model            cString
	WindowShiftRatio float32
}

type sherpaSegmentationModelConfig struct {
	Pyannote   sherpaPyannoteModelConfig
	NumThreads int32
	Debug      int32
	Provider   cString
}

type sherpaSpeakerEmbeddingExtractorConfig struct {
	Model      cString
	NumThreads int32
	Debug      int32
	Provider   cString
}

type sherpaFastClusteringConfig struct {
	NumClusters       int32
	Threshold         float32
	ComputeConfidence int32
}

type sherpaDiarizationConfig struct {
	Segmentation   sherpaSegmentationModelConfig
	Embedding      sherpaSpeakerEmbeddingExtractorConfig
	Clustering     sherpaFastClusteringConfig
	MinDurationOn  float32
	MinDurationOff float32
}

type sherpaDiarizationSegment struct {
	Start, End float32
	Speaker    int32
	Confidence float32
}

// sherpaAPI holds the functions of the C API.
type sherpaAPI struct {
	createOfflineRecognizer  func(*sherpaOfflineRecognizerConfig) uintptr
	destroyOfflineRecognizer func(uintptr)
	createOfflineStream      func(uintptr) uintptr
	destroyOfflineStream     func(uintptr)
	acceptWaveformOffline    func(stream uintptr, rate int32, samples *float32, n int32)
	decodeOfflineStream      func(recognizer, stream uintptr)
	offlineResultJSON        func(stream uintptr) *byte
	destroyOfflineResultJSON func(*byte)

	createOnlineRecognizer  func(*sherpaOnlineRecognizerConfig) uintptr
	destroyOnlineRecognizer func(uintptr)
	createOnlineStream      func(uintptr) uintptr
	destroyOnlineStream     func(uintptr)
	onlineAcceptWaveform    func(stream uintptr, rate int32, samples *float32, n int32)
	isOnlineStreamReady     func(recognizer, stream uintptr) int32
	decodeOnlineStream      func(recognizer, stream uintptr)
	onlineResultJSON        func(recognizer, stream uintptr) *byte
	destroyOnlineResultJSON func(*byte)
	onlineStreamReset       func(recognizer, stream uintptr)
	onlineInputFinished     func(stream uintptr)
	onlineIsEndpoint        func(recognizer, stream uintptr) int32

	createVad         func(*sherpaVadModelConfig, float32) uintptr
	destroyVad        func(uintptr)
	vadAcceptWaveform func(vad uintptr, samples *float32, n int32)
	vadEmpty          func(uintptr) int32
	vadDetected       func(uintptr) int32
	vadPop            func(uintptr)
	vadFront          func(uintptr) *sherpaSpeechSegment
	destroySegment    func(*sherpaSpeechSegment)
	vadFlush          func(uintptr)

	createDiarization     func(*sherpaDiarizationConfig) uintptr
	destroyDiarization    func(uintptr)
	diarizationSampleRate func(uintptr) int32
	diarizationSetConfig  func(uintptr, *sherpaDiarizationConfig)
	diarizationProcess    func(sd uintptr, samples *float32, n int32) uintptr
	resultNumSegments     func(uintptr) int32
	resultSortByStart     func(uintptr) *sherpaDiarizationSegment
	destroyDiarSegment    func(*sherpaDiarizationSegment)
	destroyResult         func(uintptr)
}

var (
	sherpaOnce   sync.Once
	sherpaLoaded *sherpaAPI
	sherpaErr    error
)

// sherpaLibraryNames are the file names of the C API library, by system.
func sherpaLibraryNames() []string {
	switch runtime.GOOS {
	case "windows":
		return []string{"sherpa-onnx-c-api.dll"}
	case "darwin":
		return []string{"libsherpa-onnx-c-api.dylib"}
	}
	return []string{"libsherpa-onnx-c-api.so"}
}

// loadSherpa loads the C API once.
func loadSherpa() (*sherpaAPI, error) {
	sherpaOnce.Do(func() {
		handle, err := openLibrary(sherpaLibraryNames())
		if err != nil {
			sherpaErr = fmt.Errorf("cannot load sherpa-onnx: %w", err)
			return
		}
		api := &sherpaAPI{}
		defer func() {
			if r := recover(); r != nil {
				sherpaErr = fmt.Errorf("the sherpa-onnx library is missing a function: %v", r)
			}
		}()
		for _, f := range []struct {
			fn   any
			name string
		}{
			{&api.createOfflineRecognizer, "SherpaOnnxCreateOfflineRecognizer"},
			{&api.destroyOfflineRecognizer, "SherpaOnnxDestroyOfflineRecognizer"},
			{&api.createOfflineStream, "SherpaOnnxCreateOfflineStream"},
			{&api.destroyOfflineStream, "SherpaOnnxDestroyOfflineStream"},
			{&api.acceptWaveformOffline, "SherpaOnnxAcceptWaveformOffline"},
			{&api.decodeOfflineStream, "SherpaOnnxDecodeOfflineStream"},
			{&api.offlineResultJSON, "SherpaOnnxGetOfflineStreamResultAsJson"},
			{&api.destroyOfflineResultJSON, "SherpaOnnxDestroyOfflineStreamResultJson"},
			{&api.createOnlineRecognizer, "SherpaOnnxCreateOnlineRecognizer"},
			{&api.destroyOnlineRecognizer, "SherpaOnnxDestroyOnlineRecognizer"},
			{&api.createOnlineStream, "SherpaOnnxCreateOnlineStream"},
			{&api.destroyOnlineStream, "SherpaOnnxDestroyOnlineStream"},
			{&api.onlineAcceptWaveform, "SherpaOnnxOnlineStreamAcceptWaveform"},
			{&api.isOnlineStreamReady, "SherpaOnnxIsOnlineStreamReady"},
			{&api.decodeOnlineStream, "SherpaOnnxDecodeOnlineStream"},
			{&api.onlineResultJSON, "SherpaOnnxGetOnlineStreamResultAsJson"},
			{&api.destroyOnlineResultJSON, "SherpaOnnxDestroyOnlineStreamResultJson"},
			{&api.onlineStreamReset, "SherpaOnnxOnlineStreamReset"},
			{&api.onlineInputFinished, "SherpaOnnxOnlineStreamInputFinished"},
			{&api.onlineIsEndpoint, "SherpaOnnxOnlineStreamIsEndpoint"},
			{&api.createVad, "SherpaOnnxCreateVoiceActivityDetector"},
			{&api.destroyVad, "SherpaOnnxDestroyVoiceActivityDetector"},
			{&api.vadAcceptWaveform, "SherpaOnnxVoiceActivityDetectorAcceptWaveform"},
			{&api.vadEmpty, "SherpaOnnxVoiceActivityDetectorEmpty"},
			{&api.vadDetected, "SherpaOnnxVoiceActivityDetectorDetected"},
			{&api.vadPop, "SherpaOnnxVoiceActivityDetectorPop"},
			{&api.vadFront, "SherpaOnnxVoiceActivityDetectorFront"},
			{&api.destroySegment, "SherpaOnnxDestroySpeechSegment"},
			{&api.vadFlush, "SherpaOnnxVoiceActivityDetectorFlush"},
			{&api.createDiarization, "SherpaOnnxCreateOfflineSpeakerDiarization"},
			{&api.destroyDiarization, "SherpaOnnxDestroyOfflineSpeakerDiarization"},
			{&api.diarizationSampleRate, "SherpaOnnxOfflineSpeakerDiarizationGetSampleRate"},
			{&api.diarizationSetConfig, "SherpaOnnxOfflineSpeakerDiarizationSetConfig"},
			{&api.diarizationProcess, "SherpaOnnxOfflineSpeakerDiarizationProcess"},
			{&api.resultNumSegments, "SherpaOnnxOfflineSpeakerDiarizationResultGetNumSegments"},
			{&api.resultSortByStart, "SherpaOnnxOfflineSpeakerDiarizationResultSortByStartTime"},
			{&api.destroyDiarSegment, "SherpaOnnxOfflineSpeakerDiarizationDestroySegment"},
			{&api.destroyResult, "SherpaOnnxOfflineSpeakerDiarizationDestroyResult"},
		} {
			purego.RegisterLibFunc(f.fn, handle, f.name)
		}
		sherpaLoaded = api
	})
	return sherpaLoaded, sherpaErr
}

// cStrings keeps the strings of a config alive. The C API reads them only
// while it makes the object, so they live until then.
type cStrings struct{ held [][]byte }

func (c *cStrings) add(s string) cString {
	b := append([]byte(s), 0)
	c.held = append(c.held, b)
	return &b[0]
}

// goString copies a NUL-terminated C string.
func goString(p *byte) string {
	if p == nil {
		return ""
	}
	n := 0
	for *(*byte)(unsafe.Add(unsafe.Pointer(p), n)) != 0 {
		n++
	}
	return string(unsafe.Slice(p, n))
}

// samplesPtr is the first sample of s, or nil.
func samplesPtr(s []float32) *float32 {
	if len(s) == 0 {
		return nil
	}
	return &s[0]
}

// unsafeSlice is n values that C wrote from p on.
func unsafeSlice[T any](p *T, n int) []T { return unsafe.Slice(p, n) }
