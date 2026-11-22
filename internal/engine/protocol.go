// Package engine is the client of the Sayso engine protocol v1
// (docs/engine-protocol.md): newline-delimited JSON over the stdin and stdout
// of an engine process (SaysoEngine on macOS, sayso-engine elsewhere). It
// also has the client of the cleanup runtime (sayso-cleanup).
package engine

import (
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"math"
)

// ProtocolVersion is the protocol this client speaks.
const ProtocolVersion = 1

// SampleRate is the audio rate of everything after capture.
const SampleRate = 16_000

// Features of an engine.
const (
	FeatureUnload            = "unload"
	FeatureKeepPreview       = "keep_preview"
	FeaturePrepare           = "prepare"
	FeatureLanguageModel     = "language_model"
	FeatureConcurrentStreams = "concurrent_streams"
	FeatureSegments          = "segments"
	FeatureVad               = "vad"
	FeatureDiarization       = "diarization"
	FeatureCatalog           = "catalog"
	FeatureDetectedLanguage  = "detected_language"
	FeatureStoreV1           = "store_v1"
)

// Message is one line from the engine. Raw holds every field.
type Message struct {
	V       int    `json:"v"`
	Type    string `json:"type"`
	ID      string `json:"id,omitempty"`
	Message string `json:"message,omitempty"`
	Code    string `json:"code,omitempty"`
	Reason  string `json:"reason,omitempty"`

	Raw map[string]json.RawMessage `json:"-"`
}

func parseMessage(line []byte) (Message, error) {
	var m Message
	if err := json.Unmarshal(line, &m.Raw); err != nil {
		return m, err
	}
	if err := json.Unmarshal(line, &m); err != nil {
		return m, err
	}
	if m.Type == "" {
		return m, fmt.Errorf("a line without a type")
	}
	return m, nil
}

// Field decodes one field into v; false when it is missing or does not fit.
func (m Message) Field(name string, v any) bool {
	raw, ok := m.Raw[name]
	if !ok {
		return false
	}
	return json.Unmarshal(raw, v) == nil
}

func (m Message) str(name string) string {
	var s string
	m.Field(name, &s)
	return s
}

func (m Message) u64(name string) uint64 {
	var f float64
	m.Field(name, &f)
	if f < 0 {
		return 0
	}
	return uint64(f)
}

// Error is an error reply of the engine.
type Error struct {
	Message string
	Code    string
	Reason  string
}

func (e *Error) Error() string {
	if e.Code != "" {
		return "engine: " + e.Message + " (" + e.Code + ")"
	}
	return "engine: " + e.Message
}

// Hello is the engine's answer to hello.
type Hello struct {
	Version  string
	Protocol int
	Features []string
}

// Has reports whether the engine lists a feature.
func (h Hello) Has(f string) bool {
	for _, x := range h.Features {
		if x == f {
			return true
		}
	}
	return false
}

// Model states.
const (
	StateNotDownloaded = "not_downloaded"
	StateDownloading   = "downloading"
	StateDownloaded    = "downloaded"
	StateOptimizing    = "optimizing"
	StateReady         = "ready"
	StateFailed        = "failed"
)

// ModelStatus is the state of a model (crates/sayso-core/src/stt.rs).
type ModelStatus struct {
	State      string  `json:"state"`
	Fraction   float32 `json:"fraction,omitempty"`
	BytesDone  uint64  `json:"bytes_done,omitempty"`
	BytesTotal uint64  `json:"bytes_total,omitempty"`
	Message    string  `json:"message,omitempty"`
	// Reason is a stable code (incomplete_download, no_gpu, ...).
	Reason string `json:"reason,omitempty"`
	// Preview: the final pass left memory and the live preview stayed (keep_preview).
	Preview bool `json:"-"`
	LoadMs  uint64
}

// IsUsable is true when the model is in memory.
func (s ModelStatus) IsUsable() bool { return s.State == StateReady }

// IsOnDisk is true for ready, downloaded, and optimizing.
func (s ModelStatus) IsOnDisk() bool {
	return s.State == StateReady || s.State == StateDownloaded || s.State == StateOptimizing
}

// Reason codes.
const (
	ReasonIncompleteDownload = "incomplete_download"
	ReasonDownloadFailed     = "download_failed"
	ReasonLoadFailed         = "load_failed"
	ReasonNoGPU              = "no_gpu"
	ReasonFormatChanged      = "format_changed"
	ReasonUnsupportedCPU     = "unsupported_cpu"
	ReasonDeviceNotEligible  = "device_not_eligible"
	ReasonIntelligenceOff    = "intelligence_off"
	ReasonModelNotReady      = "model_not_ready"
	ReasonNeedsMacOS26       = "needs_macos_26"
	ReasonUnavailable        = "unavailable"
	ReasonTimeout            = "timeout"
	ReasonRefused            = "refused"
	ReasonFailed             = "failed"
)

// CatalogModel is one model of an engine's built-in catalog.
type CatalogModel struct {
	ID              string          `json:"id"`
	Name            string          `json:"name"`
	Engine          json.RawMessage `json:"engine,omitempty"`
	SizeBytes       uint64          `json:"size_bytes,omitempty"`
	Languages       []string        `json:"languages,omitempty"`
	Roles           []string        `json:"roles,omitempty"`
	Timestamps      string          `json:"timestamps,omitempty"`
	DetectsLanguage bool            `json:"detects_language,omitempty"`
	Recommended     bool            `json:"recommended,omitempty"`
}

// Retired is a model taken out of a catalog.
type Retired struct {
	ID      string `json:"id"`
	Message string `json:"message"`
}

// Word and Segment are the timing of a final pass.
type Word struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
}

type Segment struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
	Words   []Word `json:"words"`
}

// Final is the result of transcribe.
type Final struct {
	Text      string
	ElapsedMs uint64
	Segments  []Segment
	// Language is the detected language, or "".
	Language string
}

// SpeechPart is audio with speech, found by VAD.
type SpeechPart struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
}

// Turn is one speaker turn of diarization.
type Turn struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Speaker uint32 `json:"speaker"`
}

// Diarization is who speaks when.
type Diarization struct {
	Speakers uint32 `json:"speakers"`
	Turns    []Turn `json:"turns"`
}

// LanguageModelState is the reply to language_model.
type LanguageModelState struct {
	Available bool
	Model     string
	Message   string
	Reason    string
}

// Generated is the reply to generate.
type Generated struct{ Text, Model string }

// EventKind is the kind of an Event.
type EventKind int

const (
	EventModelStatus EventKind = iota
	EventPartial
	EventSpeechStart
	EventSpeechEnd
	EventStarted
	EventStartFailed
	EventCrashed
	EventRestarted
	EventLog
	EventPartState
)

// Event is something the engine reports without a request.
type Event struct {
	Kind      EventKind
	Model     string
	Status    ModelStatus
	Session   uint64
	Committed string
	Tentative string
	StartMs   uint64
	EndMs     uint64
	Level     string
	Message   string
	Part      string // part_state
	State     string // part_state
}

// ToPCM16 turns samples in [-1, 1] into 16-bit PCM (scaled by 32767, clamped).
func ToPCM16(samples []float32) []int16 {
	out := make([]int16, len(samples))
	for i, s := range samples {
		out[i] = int16(math.Round(float64(min(max(s, -1), 1) * 32767)))
	}
	return out
}

// EncodeWAV makes a complete 16 kHz mono 16-bit WAV file.
func EncodeWAV(pcm []int16) []byte {
	dataLen := uint32(len(pcm) * 2)
	out := make([]byte, 0, 44+len(pcm)*2)
	le32 := func(v uint32) { out = binary.LittleEndian.AppendUint32(out, v) }
	le16 := func(v uint16) { out = binary.LittleEndian.AppendUint16(out, v) }
	out = append(out, "RIFF"...)
	le32(36 + dataLen)
	out = append(out, "WAVEfmt "...)
	le32(16)
	le16(1)
	le16(1)
	le32(SampleRate)
	le32(SampleRate * 2)
	le16(2)
	le16(16)
	out = append(out, "data"...)
	le32(dataLen)
	for _, s := range pcm {
		le16(uint16(s))
	}
	return out
}

// PCMBase64 is the stream_audio payload: base64 of little-endian 16-bit PCM.
func PCMBase64(pcm []int16) string {
	b := make([]byte, len(pcm)*2)
	for i, s := range pcm {
		binary.LittleEndian.PutUint16(b[i*2:], uint16(s))
	}
	return base64.StdEncoding.EncodeToString(b)
}
