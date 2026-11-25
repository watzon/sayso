// Package models holds the speech model catalog, the role selection rules,
// the model store (docs/sdk.md, "Model store"), its descriptors, downloads,
// and the migration of the old model folders.
package models

import (
	"fmt"
	"math"
	"runtime"
	"slices"
	"sync"

	"github.com/watzon/sayso/internal/models/languages"
)

// ModelID is a local model id, or "<provider id>:<model>" for a cloud model.
type ModelID string

func (m ModelID) String() string { return string(m) }

// Engine kinds, as the tag `kind` of EngineKind in JSON.
const (
	KindParakeetUnified      = "parakeet_unified"
	KindParakeetEou          = "parakeet_eou"
	KindParakeetTdt          = "parakeet_tdt"
	KindNemotron             = "nemotron"
	KindNemotronMultilingual = "nemotron_multilingual"
	KindCohere               = "cohere"
	KindCanary               = "canary"
	KindSenseVoice           = "sense_voice"
	KindParaformer           = "paraformer"
	KindWhisper              = "whisper"
	KindAppleSpeech          = "apple_speech"
	KindWhisperCpp           = "whisper_cpp"
	KindRemote               = "remote"
	KindSherpa               = "sherpa"
	KindParakeetCpp          = "parakeet_cpp"
	KindSherpaVad            = "sherpa_vad"
	KindSherpaDiarization    = "sherpa_diarization"
	KindFluidVad             = "fluid_vad"
	KindFluidDiarization     = "fluid_diarization"
)

// EngineKind is the engine and the files of a model. Only the fields of its
// Kind are set. JSON matches the serde form of the Rust enum.
type EngineKind struct {
	Kind            string  `json:"kind"`
	StreamingTier   string  `json:"streaming_tier,omitempty"`
	Version         string  `json:"version,omitempty"`
	ChunkMs         uint32  `json:"chunk_ms,omitempty"`
	Variant         string  `json:"variant,omitempty"`
	File            string  `json:"file,omitempty"`
	Provider        string  `json:"provider,omitempty"`
	Model           string  `json:"model,omitempty"`
	Recipe          string  `json:"recipe,omitempty"`
	Archive         string  `json:"archive,omitempty"`
	PreviewArchive  *string `json:"preview_archive,omitempty"`
	RedecodePreview bool    `json:"redecode_preview,omitempty"`
	Repo            string  `json:"repo,omitempty"`
	Revision        string  `json:"revision,omitempty"`
	Segmentation    string  `json:"segmentation,omitempty"`
	Embedding       string  `json:"embedding,omitempty"`
}

// ModelTask is what a model does.
type ModelTask int

const (
	TaskSpeech ModelTask = iota
	TaskVad
	TaskDiarization
)

// Timestamps is the timing detail a model gives.
type Timestamps string

const (
	TimestampsWord    Timestamps = "word"
	TimestampsSegment Timestamps = "segment"
	TimestampsNone    Timestamps = "none"
)

// Family groups models on the Models page.
type Family int

const (
	FamilyParakeet Family = iota
	FamilyWhisper
	FamilyOther
	FamilyCloud
)

func (e EngineKind) Family() Family {
	switch e.Kind {
	case KindParakeetUnified, KindParakeetEou, KindParakeetTdt, KindParakeetCpp:
		return FamilyParakeet
	case KindWhisper, KindWhisperCpp:
		return FamilyWhisper
	case KindSherpa:
		if len(e.Recipe) >= 8 && e.Recipe[:8] == "parakeet" {
			return FamilyParakeet
		}
		if e.Recipe == "whisper" {
			return FamilyWhisper
		}
	case KindRemote:
		return FamilyCloud
	}
	return FamilyOther
}

func (e EngineKind) Runtime() string {
	switch e.Kind {
	case KindWhisper:
		return "WhisperKit"
	case KindRemote:
		return "Cloud"
	case KindAppleSpeech:
		return "macOS"
	case KindWhisperCpp:
		return "whisper.cpp"
	case KindSherpa, KindSherpaVad, KindSherpaDiarization:
		return "sherpa-onnx"
	case KindParakeetCpp:
		return "parakeet.cpp"
	}
	return "FluidAudio"
}

func (e EngineKind) Timestamps() Timestamps {
	switch e.Kind {
	case KindParakeetUnified, KindParakeetEou, KindParakeetTdt, KindNemotron, KindNemotronMultilingual, KindWhisper, KindAppleSpeech:
		return TimestampsWord
	case KindWhisperCpp:
		return TimestampsSegment
	case KindSherpa:
		switch e.Recipe {
		case "parakeet_tdt", "parakeet_tdt_ctc", "sense_voice":
			return TimestampsWord
		case "whisper":
			return TimestampsSegment
		}
	}
	return TimestampsNone
}

func (e EngineKind) DetectsLanguage() bool {
	switch e.Kind {
	case KindWhisper, KindWhisperCpp, KindSenseVoice:
		return true
	case KindSherpa:
		return e.Recipe == "whisper"
	}
	return false
}

func (e EngineKind) Task() ModelTask {
	switch e.Kind {
	case KindSherpaVad, KindFluidVad:
		return TaskVad
	case KindSherpaDiarization, KindFluidDiarization:
		return TaskDiarization
	}
	return TaskSpeech
}

func (e EngineKind) NeedsGPU() bool { return e.Kind == KindParakeetCpp }

// ModelInfo is one catalog entry. DescriptionKey is the i18n key of the
// description (internal/i18n translates it).
type ModelInfo struct {
	ID                ModelID    `json:"id"`
	Name              string     `json:"name"`
	Vendor            string     `json:"vendor"`
	DescriptionKey    string     `json:"description_key"`
	Engine            EngineKind `json:"engine"`
	SizeBytes         uint64     `json:"size_bytes"`
	Languages         []string   `json:"languages"`
	FinalPass         bool       `json:"final_pass"`
	LivePreview       bool       `json:"live_preview"`
	Vocabulary        bool       `json:"vocabulary"`
	Speed             uint8      `json:"speed"`
	WerPercent        *float32   `json:"wer_percent"`
	PreviewWerPercent *float32   `json:"preview_wer_percent"`
	Punctuation       bool       `json:"punctuation"`
	Rtfx              *uint32    `json:"rtfx"`
	CardURL           string     `json:"card_url,omitempty"`
	Credit            string     `json:"credit,omitempty"`
	Recommended       bool       `json:"recommended"`
	MinMacOS          uint32     `json:"min_macos"`
}

func (m ModelInfo) IsRemote() bool        { return m.Engine.Kind == KindRemote }
func (m ModelInfo) Task() ModelTask       { return m.Engine.Task() }
func (m ModelInfo) IsMultilingual() bool  { return len(m.Languages) > 1 }
func (m ModelInfo) DetectsLanguage() bool { return m.Engine.DetectsLanguage() && len(m.Languages) > 1 }

// PreviewIntervalMs is how often a streaming model gives a preview, when fixed.
func (m ModelInfo) PreviewIntervalMs() (uint32, bool) {
	if !m.LivePreview {
		return 0, false
	}
	switch m.Engine.Kind {
	case KindParakeetUnified, KindParakeetEou:
		return 160, true
	case KindNemotron, KindNemotronMultilingual:
		return m.Engine.ChunkMs, true
	}
	return 0, false
}

// Understands reports whether the model takes `language` (Auto needs a
// multilingual model).
func (m ModelInfo) Understands(language string) bool {
	if language == languages.Auto {
		return m.IsMultilingual()
	}
	return slices.Contains(m.Languages, language)
}

// LanguageFor is the language that goes to the model for a setting.
func (m ModelInfo) LanguageFor(setting string) string {
	switch {
	case slices.Contains(m.Languages, setting):
		return setting
	case m.IsMultilingual():
		return languages.Auto
	case len(m.Languages) > 0:
		return m.Languages[0]
	}
	return languages.Auto
}

// System selects a catalog.
type System int

const (
	MacOS System = iota
	Linux
	Windows
)

// CurrentSystem is the catalog system of this binary.
func CurrentSystem() System {
	switch runtime.GOOS {
	case "darwin":
		return MacOS
	case "windows":
		return Windows
	}
	return Linux
}

const mb = 1_000_000

// DefaultModel is the final model of a new config.
func DefaultModel() ModelID { return DefaultModelFor(CurrentSystem()) }

func DefaultModelFor(s System) ModelID {
	switch s {
	case MacOS:
		return "parakeet-unified-en"
	case Windows:
		return "parakeet-tdt-v3"
	}
	return "parakeet-tdt-v2"
}

// PreviewFallback is the preferred streaming model for "auto" previews.
func PreviewFallback() ModelID { return PreviewFallbackFor(CurrentSystem()) }

func PreviewFallbackFor(s System) ModelID {
	switch s {
	case MacOS:
		return "parakeet-eou-120m"
	case Windows:
		return "parakeet-tdt-v3"
	}
	return "kroko-en"
}

func byOS(s System, mac, windows, linux string) string {
	switch s {
	case MacOS:
		return mac
	case Windows:
		return windows
	}
	return linux
}

// SuggestedFor is the final model to suggest for a dictation language.
func SuggestedFor(language string, runsHere func(ModelInfo) bool) ModelID {
	return suggestedFor(CurrentSystem(), language, runsHere)
}

func suggestedFor(s System, language string, runsHere func(ModelInfo) bool) ModelID {
	var id string
	switch {
	case language == "en" || language == languages.Auto:
		return DefaultModelFor(s)
	case slices.Contains(languages.ParakeetV3(), language):
		id = byOS(s, "parakeet-ultra", "parakeet-tdt-v3", "parakeet-tdt-v3")
	case language == "ja":
		id = byOS(s, "parakeet-ja", "whisper-large-v3-turbo", "sensevoice-small")
	case language == "zh" || language == "yue" || language == "ko":
		id = byOS(s, "sensevoice-small", "whisper-large-v3-turbo", "sensevoice-small")
	default:
		id = "whisper-large-v3-turbo"
	}
	usable := func(m ModelInfo) bool { return m.FinalPass && m.Understands(language) && runsHere(m) }
	cat := CatalogFor(s)
	for _, m := range cat {
		if m.ID == ModelID(id) && usable(m) {
			return m.ID
		}
	}
	for _, m := range cat {
		if usable(m) {
			return m.ID
		}
	}
	return ModelID(id)
}

var cards = map[string]string{
	"cohere-transcribe":       "CohereLabs/cohere-transcribe-03-2026",
	"parakeet-unified-en":     "nvidia/parakeet-unified-en-0.6b",
	"parakeet-tdt-v2":         "nvidia/parakeet-tdt-0.6b-v2",
	"parakeet-tdt-v3":         "nvidia/parakeet-tdt-0.6b-v3",
	"nemotron-en":             "nvidia/nemotron-speech-streaming-en-0.6b",
	"canary-1b-v2":            "nvidia/canary-1b-v2",
	"whisper-large-v3":        "openai/whisper-large-v3",
	"parakeet-tdt-ctc-110m":   "nvidia/parakeet-tdt_ctc-110m",
	"whisper-distil-large-v3": "distil-whisper/distil-large-v3",
	"whisper-large-v3-turbo":  "openai/whisper-large-v3-turbo",
	"whisper-large-v2":        "openai/whisper-large-v2",
	"parakeet-eou-120m":       "nvidia/parakeet_realtime_eou_120m-v1",
	"parakeet-ultra":          "moondream/parakeet-ultra",
	"fluid-speech-mini":       "FluidVoice-Labs/fluid-speech-mini-coreml",
	"parakeet-redux":          "moondream/parakeet-redux",
	"phonon-2":                "FermionResearch/Phonon-2",
	"parakeet-ja":             "nvidia/parakeet-tdt_ctc-0.6b-ja",
	"nemotron-multilingual":   "nvidia/nemotron-3.5-asr-streaming-0.6b",
	"whisper-medium":          "openai/whisper-medium",
	"whisper-small":           "openai/whisper-small",
	"whisper-base":            "openai/whisper-base",
	"whisper-tiny":            "openai/whisper-tiny",
	"sensevoice-small":        "FunAudioLLM/SenseVoiceSmall",
}

const fluidMiniCredit = "Fluid Speech Mini by FluidVoice. Built on NVIDIA Parakeet TDT 0.6B v3 and Moondream Parakeet Ultra."

func speedLevel(rtfx uint32) uint8 {
	switch {
	case rtfx >= 120:
		return 5
	case rtfx >= 60:
		return 4
	case rtfx >= 30:
		return 3
	case rtfx >= 12:
		return 2
	}
	return 1
}

type entry struct {
	id, name, vendor, desc string
	engine                 EngineKind
	sizeMB                 uint64
	langs                  []string
	speed                  uint8
}

func (e entry) build(mac bool) ModelInfo {
	m := ModelInfo{
		ID: ModelID(e.id), Name: e.name, Vendor: e.vendor, DescriptionKey: e.desc, Engine: e.engine,
		SizeBytes: e.sizeMB * mb, Languages: e.langs, FinalPass: true, Speed: e.speed,
		Punctuation: e.id != "parakeet-eou-120m",
	}
	if meas, ok := measured[e.id]; ok {
		w := meas.wer
		m.WerPercent = &w
		if mac {
			r := meas.rtfx
			m.Rtfx = &r
			m.Speed = speedLevel(r)
		}
	}
	if pw, ok := previewWer[e.id]; ok {
		m.PreviewWerPercent = &pw
	}
	if c, ok := cards[e.id]; ok {
		m.CardURL = "https://huggingface.co/" + c
	}
	if e.id == "fluid-speech-mini" {
		m.Credit = fluidMiniCredit
	}
	if mac {
		m.MinMacOS = 14
	}
	return m
}

func roleModel(id, name, vendor, desc string, engine EngineKind, size uint64) ModelInfo {
	return ModelInfo{
		ID: ModelID(id), Name: name, Vendor: vendor, DescriptionKey: desc, Engine: engine, SizeBytes: size,
		Languages: languages.All(), Speed: 5,
	}
}

func str(s string) *string { return &s }

func appleCatalog() []ModelInfo {
	en := languages.English()
	tdt := func(v string) EngineKind { return EngineKind{Kind: KindParakeetTdt, Version: v} }
	whisper := func(v string) EngineKind { return EngineKind{Kind: KindWhisper, Variant: v} }
	b := func(e entry) ModelInfo { return e.build(true) }
	with := func(m ModelInfo, f func(*ModelInfo)) ModelInfo { f(&m); return m }

	list := []ModelInfo{
		with(b(entry{"parakeet-unified-en", "Parakeet Unified EN", "NVIDIA", "core.model.parakeet_unified_en",
			EngineKind{Kind: KindParakeetUnified, StreamingTier: "70_2_2"}, 595 + 578 + 98, en, 5}),
			func(m *ModelInfo) { m.LivePreview, m.Vocabulary, m.Recommended = true, true, true }),
		b(entry{"parakeet-ultra", "Parakeet Ultra", "NVIDIA", "core.model.parakeet_ultra", tdt("ultra"), 632, languages.ParakeetV3(), 5}),
		with(b(entry{"fluid-speech-mini", "Fluid Speech Mini", "FluidVoice", "core.model.fluid_speech_mini", tdt("fluid_mini"), 253, en, 5}),
			func(m *ModelInfo) { m.MinMacOS = 15 }),
		b(entry{"parakeet-tdt-v3", "Parakeet TDT v3", "NVIDIA", "core.model.parakeet_tdt_v3", tdt("v3"), 483, languages.ParakeetV3(), 5}),
		with(b(entry{"parakeet-redux", "Parakeet Redux", "NVIDIA", "core.model.parakeet_redux", tdt("redux"), 220, languages.ParakeetV3(), 5}),
			func(m *ModelInfo) { m.MinMacOS = 15 }),
		b(entry{"parakeet-tdt-v2", "Parakeet TDT v2", "NVIDIA", "core.model.parakeet_tdt_v2", tdt("v2"), 465, en, 5}),
		with(b(entry{"phonon-2", "Phonon-2", "Fermion Research", "core.model.phonon_2", tdt("phonon2"), 357, en, 5}),
			func(m *ModelInfo) { m.MinMacOS = 15 }),
		b(entry{"parakeet-tdt-ctc-110m", "Parakeet TDT-CTC 110M", "NVIDIA", "core.model.parakeet_tdt_ctc_110m", tdt("tdt_ctc_110m"), 227, en, 5}),
		b(entry{"parakeet-ja", "Parakeet Japanese", "NVIDIA", "core.model.parakeet_ja", tdt("ja"), 625, []string{"ja"}, 3}),
		with(b(entry{"parakeet-eou-120m", "Parakeet EOU 120M", "NVIDIA", "core.model.parakeet_eou_120m", EngineKind{Kind: KindParakeetEou}, 222, en, 5}),
			func(m *ModelInfo) { m.LivePreview = true }),
		b(entry{"whisper-large-v3-turbo", "Whisper large-v3 turbo", "OpenAI", "core.model.whisper_large_v3_turbo", whisper("openai_whisper-large-v3-v20240930_turbo_632MB"), 646, languages.Whisper(), 3}),
		b(entry{"whisper-large-v3", "Whisper large-v3", "OpenAI", "core.model.whisper_large_v3", whisper("openai_whisper-large-v3_947MB"), 948, languages.Whisper(), 2}),
		b(entry{"whisper-large-v2", "Whisper large-v2", "OpenAI", "core.model.whisper_large_v2", whisper("openai_whisper-large-v2_949MB"), 952, languages.Whisper(), 2}),
		b(entry{"whisper-distil-large-v3", "Distil-Whisper large-v3", "Hugging Face", "core.model.whisper_distil_large_v3", whisper("distil-whisper_distil-large-v3_594MB"), 595, en, 4}),
		b(entry{"whisper-medium", "Whisper medium", "OpenAI", "core.model.whisper_medium", whisper("openai_whisper-medium"), 1530, languages.Whisper(), 2}),
		b(entry{"whisper-small", "Whisper small", "OpenAI", "core.model.whisper_small", whisper("openai_whisper-small_216MB"), 217, languages.Whisper(), 4}),
		b(entry{"whisper-base", "Whisper base", "OpenAI", "core.model.whisper_base", whisper("openai_whisper-base"), 147, languages.Whisper(), 5}),
		b(entry{"whisper-tiny", "Whisper tiny", "OpenAI", "core.model.whisper_tiny", whisper("openai_whisper-tiny"), 77, languages.Whisper(), 5}),
		with(b(entry{"apple-speech", "Apple Speech", "Apple", "core.model.apple_speech", EngineKind{Kind: KindAppleSpeech}, 0, languages.AppleSpeech(), 5}),
			func(m *ModelInfo) { m.MinMacOS = 26 }),
		with(b(entry{"nemotron-multilingual", "Nemotron Multilingual", "NVIDIA", "core.model.nemotron_multilingual", EngineKind{Kind: KindNemotronMultilingual, ChunkMs: 1120}, 664, languages.Nemotron(), 4}),
			func(m *ModelInfo) { m.LivePreview, m.Vocabulary = true, true }),
		with(b(entry{"nemotron-en", "Nemotron Streaming EN", "NVIDIA", "core.model.nemotron_en", EngineKind{Kind: KindNemotron, ChunkMs: 1120}, 627, en, 4}),
			func(m *ModelInfo) { m.LivePreview = true }),
		with(b(entry{"cohere-transcribe", "Cohere Transcribe", "Cohere", "core.model.cohere_transcribe", EngineKind{Kind: KindCohere}, 2190, languages.Cohere(), 3}),
			func(m *ModelInfo) { m.MinMacOS = 15 }),
		with(b(entry{"canary-1b-v2", "Canary 1B v2", "NVIDIA", "core.model.canary_1b_v2", EngineKind{Kind: KindCanary}, 569, en, 2}),
			func(m *ModelInfo) { m.MinMacOS = 15 }),
		b(entry{"sensevoice-small", "SenseVoice Small", "Alibaba", "core.model.sensevoice_small", EngineKind{Kind: KindSenseVoice}, 945, languages.SenseVoice(), 5}),
		b(entry{"paraformer-zh", "Paraformer Large", "Alibaba", "core.model.paraformer_zh", EngineKind{Kind: KindParaformer}, 644, []string{"zh"}, 5}),
		with(roleModel("silero-vad", "Silero VAD", "Silero", "core.model.silero_vad", EngineKind{Kind: KindFluidVad}, 1_063_427),
			func(m *ModelInfo) {
				m.CardURL, m.MinMacOS = "https://huggingface.co/FluidInference/silero-vad-coreml", 14
			}),
		with(roleModel("speaker-diarization", "Speaker Diarization", "pyannote", "core.model.speaker_diarization", EngineKind{Kind: KindFluidDiarization}, 21_787_007),
			func(m *ModelInfo) {
				m.CardURL, m.MinMacOS = "https://huggingface.co/FluidInference/speaker-diarization-coreml", 14
				m.Credit = "Core ML conversion by Fluid Inference of pyannote speaker-diarization-community-1, with " +
					"WeSpeaker and BUT Speech@FIT parts. CC BY 4.0. The files are changed."
			}),
	}
	return list
}

// The sherpa-onnx streaming archive that gives the preview of the Linux models.
const PreviewArchive = "sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06"

// Speaker embedding and segmentation files of sherpa-onnx diarization, with sizes.
var (
	SpeakerEmbedding = struct {
		Name string
		Size uint64
	}{"3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx", 28_281_164}
	SpeakerSegmentation = struct {
		Name string
		Size uint64
	}{"sherpa-onnx-pyannote-segmentation-3-0", 6_958_444}
)

func onnxCatalog(s System) []ModelInfo {
	en := languages.English()
	b := func(e entry) ModelInfo { return e.build(false) }
	with := func(m ModelInfo, f func(*ModelInfo)) ModelInfo { f(&m); return m }
	sherpa := func(recipe, archive string, preview bool) EngineKind {
		k := EngineKind{Kind: KindSherpa, Recipe: recipe, Archive: archive}
		if preview {
			k.PreviewArchive = str(PreviewArchive)
		}
		return k
	}
	tdtV2 := with(b(entry{"parakeet-tdt-v2", "Parakeet TDT v2", "NVIDIA", "core.model.parakeet_tdt_v2_onnx",
		sherpa("parakeet_tdt", "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8", true), 482 + 57, en, 4}),
		func(m *ModelInfo) { m.LivePreview = true })
	tdtV3 := with(b(entry{"parakeet-tdt-v3", "Parakeet TDT v3", "NVIDIA", "core.model.parakeet_tdt_v3_portable",
		EngineKind{Kind: KindSherpa, Recipe: "parakeet_tdt", Archive: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8", RedecodePreview: true},
		487, languages.ParakeetV3(), 4}),
		func(m *ModelInfo) { m.LivePreview = true })
	const fluidRepo = "FluidVoice-Labs/fluid-speech-mini-gguf"
	fluid := with(b(entry{"fluid-speech-mini", "Fluid Speech Mini", "FluidVoice", "core.model.fluid_speech_mini_onnx",
		EngineKind{Kind: KindParakeetCpp, Repo: fluidRepo, Revision: "490bcd5de891f5efd0e732434dfb0a4b0899aead",
			File: "fluid-speech-mini-q2k-q4k.gguf", PreviewArchive: str(PreviewArchive)},
		347 + 57, en, 5}),
		func(m *ModelInfo) {
			w := float32(6.56)
			m.LivePreview, m.WerPercent, m.CardURL = true, &w, "https://huggingface.co/"+fluidRepo
			m.Credit = fluidMiniCredit + " Runs on parakeet.cpp and ggml (MIT License)."
		})

	var list []ModelInfo
	if s == Windows {
		list = append(list, with(tdtV3, func(m *ModelInfo) { m.Recommended = true }), tdtV2)
	} else {
		list = append(list, with(tdtV2, func(m *ModelInfo) { m.Recommended = true }), fluid, tdtV3)
	}
	list = append(list,
		b(entry{"parakeet-tdt-ctc-110m", "Parakeet TDT-CTC 110M", "NVIDIA", "core.model.parakeet_tdt_ctc_110m_onnx",
			sherpa("parakeet_tdt_ctc", "sherpa-onnx-nemo-parakeet_tdt_ctc_110m-en-36000-int8", false), 104, en, 5}),
		with(b(entry{"kroko-en", "Kroko Streaming EN", "Banafo", "core.model.kroko_en", sherpa("zipformer_streaming", PreviewArchive, false), 57, en, 5}),
			func(m *ModelInfo) { m.FinalPass, m.LivePreview = false, true }),
	)
	if s == Windows {
		ggml := func(id, name, desc, file string, size uint64, speed uint8) ModelInfo {
			return with(b(entry{id, name, "OpenAI", desc, EngineKind{Kind: KindWhisperCpp, File: file}, size, languages.Whisper(), speed}),
				func(m *ModelInfo) { m.Vocabulary = true })
		}
		list = append(list,
			ggml("whisper-large-v3-turbo", "Whisper large-v3 turbo", "core.model.whisper_large_v3_turbo_portable", "ggml-large-v3-turbo-q5_0.bin", 574, 3),
			ggml("whisper-large-v3", "Whisper large-v3", "core.model.whisper_large_v3_portable", "ggml-large-v3-q5_0.bin", 1081, 1),
			ggml("whisper-medium", "Whisper medium", "core.model.whisper_medium_portable", "ggml-medium-q5_0.bin", 539, 2),
			ggml("whisper-small", "Whisper small", "core.model.whisper_small", "ggml-small.bin", 488, 4),
			ggml("whisper-base", "Whisper base", "core.model.whisper_base", "ggml-base.bin", 148, 5),
			ggml("whisper-tiny", "Whisper tiny", "core.model.whisper_tiny_portable", "ggml-tiny.bin", 78, 5),
		)
	} else {
		list = append(list,
			b(entry{"whisper-large-v3-turbo", "Whisper large-v3 turbo", "OpenAI", "core.model.whisper_large_v3_turbo_onnx", sherpa("whisper", "sherpa-onnx-whisper-turbo", false), 563, languages.Whisper(), 2}),
			b(entry{"whisper-distil-large-v3", "Distil-Whisper large-v3", "Hugging Face", "core.model.whisper_distil_large_v3_onnx", sherpa("whisper", "sherpa-onnx-whisper-distil-large-v3", false), 529, en, 3}),
			b(entry{"whisper-small", "Whisper small", "OpenAI", "core.model.whisper_small", sherpa("whisper", "sherpa-onnx-whisper-small", false), 639, languages.Whisper(), 3}),
			b(entry{"whisper-base", "Whisper base", "OpenAI", "core.model.whisper_base", sherpa("whisper", "sherpa-onnx-whisper-base", false), 207, languages.Whisper(), 4}),
			b(entry{"whisper-tiny", "Whisper tiny", "OpenAI", "core.model.whisper_tiny_portable", sherpa("whisper", "sherpa-onnx-whisper-tiny", false), 116, languages.Whisper(), 5}),
		)
	}
	list = append(list,
		b(entry{"sensevoice-small", "SenseVoice Small", "Alibaba", "core.model.sensevoice_small", sherpa("sense_voice", "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09", false), 165, languages.SenseVoice(), 5}),
		b(entry{"moonshine-base-en", "Moonshine Base EN", "Useful Sensors", "core.model.moonshine_base_en", sherpa("moonshine", "sherpa-onnx-moonshine-base-en-quantized-2026-02-27", false), 111, en, 5}),
		with(roleModel("silero-vad", "Silero VAD", "Silero", "core.model.silero_vad", EngineKind{Kind: KindSherpaVad, Recipe: "silero", File: "silero_vad.onnx"}, 643_854),
			func(m *ModelInfo) { m.CardURL = "https://github.com/snakers4/silero-vad" }),
		with(roleModel("speaker-diarization", "Speaker Diarization", "pyannote", "core.model.speaker_diarization_onnx",
			EngineKind{Kind: KindSherpaDiarization, Segmentation: SpeakerSegmentation.Name, Embedding: SpeakerEmbedding.Name},
			SpeakerSegmentation.Size+SpeakerEmbedding.Size),
			func(m *ModelInfo) { m.CardURL = "https://huggingface.co/pyannote/segmentation-3.0" }),
	)
	return list
}

// The catalogs are built once per system and shared: callers only read
// them. Their capacity is their length, so an append copies.
var (
	catalogMu      sync.Mutex
	catalogs       = map[System][]ModelInfo{}
	speechCatalogs = map[System][]ModelInfo{}
)

// LocalModelsFor is every local model of a system: speech, VAD, and diarization.
func LocalModelsFor(s System) []ModelInfo {
	catalogMu.Lock()
	defer catalogMu.Unlock()
	return localModelsLocked(s)
}

func localModelsLocked(s System) []ModelInfo {
	if c, ok := catalogs[s]; ok {
		return c
	}
	var c []ModelInfo
	if s == MacOS {
		c = appleCatalog()
	} else {
		c = onnxCatalog(s)
	}
	c = c[:len(c):len(c)]
	catalogs[s] = c
	return c
}

// LocalModels is LocalModelsFor the current system.
func LocalModels() []ModelInfo { return LocalModelsFor(CurrentSystem()) }

// CatalogFor is the speech models only. The overlay and the controller ask
// for it many times a second (Find), so it is built once.
func CatalogFor(s System) []ModelInfo {
	catalogMu.Lock()
	defer catalogMu.Unlock()
	if c, ok := speechCatalogs[s]; ok {
		return c
	}
	var out []ModelInfo
	for _, m := range localModelsLocked(s) {
		if m.Task() == TaskSpeech {
			out = append(out, m)
		}
	}
	out = out[:len(out):len(out)]
	speechCatalogs[s] = out
	return out
}

// Catalog is the speech models of the current system.
func Catalog() []ModelInfo { return CatalogFor(CurrentSystem()) }

// ModelsFor lists the local models of one task.
func ModelsFor(task ModelTask) []ModelInfo {
	var out []ModelInfo
	for _, m := range LocalModels() {
		if m.Task() == task {
			out = append(out, m)
		}
	}
	return out
}

// FindLocal finds any local model (speech, VAD, diarization).
func FindLocal(id ModelID) (ModelInfo, bool) {
	all := LocalModels()
	for i := range all {
		if all[i].ID == id {
			return all[i], true
		}
	}
	return ModelInfo{}, false
}

// Find finds a local speech model.
func Find(id ModelID) (ModelInfo, bool) {
	cat := Catalog()
	for i := range cat {
		if cat[i].ID == id {
			return cat[i], true
		}
	}
	return ModelInfo{}, false
}

// FindWith finds a local speech model, else a model of the speech providers'
// catalog (config.Speech.Catalog()).
func FindWith(speechCatalog []ModelInfo, id ModelID) (ModelInfo, bool) {
	if m, ok := Find(id); ok {
		return m, true
	}
	for _, m := range speechCatalog {
		if m.ID == id {
			return m, true
		}
	}
	return ModelInfo{}, false
}

// FormatSize gives "1.2 GB", "250 MB", or "12 KB".
func FormatSize(bytes uint64) string {
	switch {
	case bytes >= 1_000_000_000:
		return fmt.Sprintf("%.1f GB", float64(bytes)/1e9)
	case bytes >= 1_000_000:
		return fmt.Sprintf("%d MB", uint64(math.Round(float64(bytes)/1e6)))
	}
	return fmt.Sprintf("%d KB", uint64(math.Round(float64(bytes)/1e3)))
}
