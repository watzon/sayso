package speech

import "fmt"

// Engine is the speech engine as jobs and live sessions use it. Its calls
// block until the engine answers. FromEngine gives one for an engine of the
// engine package; tests give fakes.
type Engine interface {
	// Features are the features of the hello of the engine.
	Features() []string
	// Status is the last state of a model that the engine reported.
	Status(model string) ModelStatus
	Load(model string) error
	Transcribe(samples []float32, model, language string) (string, error)
	TranscribeSegments(samples []float32, model, language string) ([]Segment, error)
	DetectSpeech(model string, samples []float32) ([]SpeechPart, error)
	Diarize(model string, samples []float32, speakers *uint32) ([]RawTurn, error)
	StartStream(session uint64, model, language string) error
	StartVad(session uint64, model string) error
	PushAudio(session uint64, samples []float32) error
	// EndStream ends a preview stream without waiting.
	EndStream(session uint64)
	// EndVad ends a VAD session after its last speech events.
	EndVad(session uint64) error
	// Subscribe gives the events of the engine until stop runs.
	Subscribe() (events <-chan EngineEvent, stop func())
}

// The states of a model.
const (
	StateNotDownloaded = "not_downloaded"
	StateDownloading   = "downloading"
	StateDownloaded    = "downloaded"
	StateOptimizing    = "optimizing"
	StateLoading       = "loading"
	StateReady         = "ready"
	StateFailed        = "failed"
)

// ModelStatus is the state of a model.
type ModelStatus struct {
	State   string
	Message string
	Reason  string
}

// IsUsable is true when the model is in memory.
func (s ModelStatus) IsUsable() bool { return s.State == StateReady }

// IsOnDisk is true for ready, downloaded, and optimizing.
func (s ModelStatus) IsOnDisk() bool {
	return s.State == StateReady || s.State == StateDownloaded || s.State == StateOptimizing
}

// EventKind is the kind of an EngineEvent.
type EventKind int

const (
	EventModelStatus EventKind = iota
	EventPartial
	EventSpeechStart
	EventSpeechEnd
)

// EngineEvent is an event of the engine.
type EngineEvent struct {
	Kind      EventKind
	Model     string
	Status    ModelStatus
	Session   uint64
	Committed string
	Tentative string
	StartMs   uint64
	EndMs     uint64
}

// RawTurn is a turn of the engine: its speakers are numbers from 0.
type RawTurn struct {
	StartMs, EndMs uint64
	Speaker        uint32
}

// EngineError is an error reply of the engine. Its code is a code of the
// protocol, or empty.
type EngineError struct {
	Code    ErrorCode
	Message string
}

func (e *EngineError) Error() string {
	if e.Code == "" {
		return e.Message
	}
	return fmt.Sprintf("%s (%s)", e.Message, e.Code)
}
