package engine

import (
	"context"
	"encoding/json"
)

// ModelState is a model_state event.
type ModelState struct {
	Model   string `json:"model"`
	State   string `json:"state"`
	LoadMs  uint64 `json:"load_ms,omitempty"`
	Preview bool   `json:"preview,omitempty"`
	Message string `json:"message,omitempty"`
	Reason  string `json:"reason,omitempty"`
}

// DownloadProgress is a download_progress event.
type DownloadProgress struct {
	Model      string  `json:"model"`
	Fraction   float64 `json:"fraction"`
	BytesDone  uint64  `json:"bytes_done"`
	BytesTotal uint64  `json:"bytes_total"`
}

// ModelRef is a model of list_models: an id, with its engine object or
// without one for a model of the built-in catalog.
type ModelRef struct {
	ID     string          `json:"id"`
	Engine json.RawMessage `json:"engine,omitempty"`
}

// CatalogModel is a model of the built-in catalog of an engine.
type CatalogModel struct {
	ID              string          `json:"id"`
	Name            string          `json:"name"`
	Engine          json.RawMessage `json:"engine,omitempty"`
	SizeBytes       uint64          `json:"size_bytes"`
	Languages       []string        `json:"languages"`
	Roles           []string        `json:"roles"`
	Timestamps      string          `json:"timestamps"`
	DetectsLanguage bool            `json:"detects_language"`
	Recommended     bool            `json:"recommended"`
}

// Retired is a model that went out of the catalog.
type Retired struct {
	ID      string `json:"id"`
	Message string `json:"message"`
}

// Word and Segment are the times of a final pass.
type Word struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
}

type Segment struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
	Words   []Word `json:"words,omitempty"`
}

// Final is the reply to transcribe.
type Final struct {
	Text      string    `json:"text"`
	ElapsedMs uint64    `json:"elapsed_ms"`
	Segments  []Segment `json:"segments,omitempty"`
	Language  string    `json:"language,omitempty"`
}

// SpeechPart is a part of audio with speech.
type SpeechPart struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
}

// Turn is a turn of one speaker.
type Turn struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Speaker uint32 `json:"speaker"`
}

// Diarization is the reply to diarize.
type Diarization struct {
	Speakers uint32 `json:"speakers"`
	Turns    []Turn `json:"turns"`
}

// Partial is a partial event of a stream.
type Partial struct {
	Session   uint64 `json:"session"`
	Committed string `json:"committed"`
	Tentative string `json:"tentative"`
}

// Speech is a speech_start or speech_end event of a VAD session. EndMs is
// 0 for speech_start.
type Speech struct {
	Session uint64 `json:"session"`
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
}

func modelFields(model string, spec json.RawMessage) map[string]any {
	f := map[string]any{"model": model}
	if len(spec) > 0 {
		f["engine"] = spec
	}
	return f
}

func decode[T any](m Message, err error) (T, error) {
	var v T
	if err != nil {
		return v, err
	}
	return v, m.Decode(&v)
}

func states(onState func(ModelState)) func(Message) {
	return func(m Message) {
		if onState != nil && m.Type == "model_state" {
			var s ModelState
			if m.Decode(&s) == nil {
				onState(s)
			}
		}
	}
}

// ListModels asks for the state of models. Each one comes as a model_state.
func (e *Engine) ListModels(ctx context.Context, refs []ModelRef) ([]ModelState, error) {
	var out []ModelState
	_, err := e.Call(ctx, "list_models", map[string]any{"models": refs}, states(func(s ModelState) { out = append(out, s) }))
	return out, err
}

// Catalog is the built-in catalog (feature catalog), with the state of each
// model.
func (e *Engine) Catalog(ctx context.Context) ([]CatalogModel, []Retired, []ModelState, error) {
	var st []ModelState
	m, err := e.Call(ctx, "list_models", nil, states(func(s ModelState) { st = append(st, s) }))
	if err != nil {
		return nil, nil, nil, err
	}
	var reply struct {
		Models  []CatalogModel `json:"models"`
		Retired []Retired      `json:"retired"`
	}
	err = m.Decode(&reply)
	return reply.Models, reply.Retired, st, err
}

// Download downloads a model. onProgress and onState get its events.
func (e *Engine) Download(ctx context.Context, model string, spec json.RawMessage, sizeBytes uint64, onProgress func(DownloadProgress), onState func(ModelState)) error {
	f := modelFields(model, spec)
	if sizeBytes > 0 {
		f["size_bytes"] = sizeBytes
	}
	_, err := e.Call(ctx, "download", f, func(m Message) {
		switch m.Type {
		case "download_progress":
			var p DownloadProgress
			if onProgress != nil && m.Decode(&p) == nil {
				onProgress(p)
			}
		case "model_state":
			states(onState)(m)
		}
	})
	return err
}

// Delete deletes the files of a model.
func (e *Engine) Delete(ctx context.Context, model string, spec json.RawMessage) error {
	_, err := e.Call(ctx, "delete", modelFields(model, spec), nil)
	return err
}

// Load loads a model and gives its load time.
func (e *Engine) Load(ctx context.Context, model string, spec json.RawMessage) (uint64, error) {
	var loadMs uint64
	_, err := e.Call(ctx, "load", modelFields(model, spec), states(func(s ModelState) {
		if s.State == "ready" {
			loadMs = s.LoadMs
		}
	}))
	return loadMs, err
}

// Unload takes a model out of memory.
func (e *Engine) Unload(ctx context.Context, model string, keepPreview bool) error {
	f := map[string]any{"model": model}
	if keepPreview {
		f["keep_preview"] = true
	}
	_, err := e.Call(ctx, "unload", f, nil)
	return err
}

// TranscribeRequest is a transcribe request. Path is a 16 kHz mono 16-bit
// WAV file.
type TranscribeRequest struct {
	Model      string
	Path       string
	Language   string
	Vocabulary []string
	Segments   bool
}

// Transcribe runs the final pass on a WAV file.
func (e *Engine) Transcribe(ctx context.Context, r TranscribeRequest) (Final, error) {
	vocab := r.Vocabulary
	if vocab == nil {
		vocab = []string{}
	}
	f := map[string]any{"model": r.Model, "path": r.Path, "language": r.Language, "vocabulary": vocab}
	if r.Segments {
		f["segments"] = true
	}
	return decode[Final](e.Call(ctx, "transcribe", f, nil))
}

// StreamStart opens a stream.
func (e *Engine) StreamStart(ctx context.Context, session uint64, model, language string) error {
	f := map[string]any{"session": session, "model": model}
	if language != "" {
		f["language"] = language
	}
	_, err := e.Call(ctx, "stream_start", f, nil)
	return err
}

// StreamAudio sends audio to a stream or a VAD session. It has no reply.
func (e *Engine) StreamAudio(session uint64, samples []int16) error {
	return e.write(map[string]any{"v": 1, "type": "stream_audio", "session": session, "pcm": PCM(samples)})
}

// StreamEnd closes a stream and gives its text ("" for a VAD session).
func (e *Engine) StreamEnd(ctx context.Context, session uint64) (string, error) {
	m, err := e.Call(ctx, "stream_end", map[string]any{"session": session}, nil)
	return m.String("text"), err
}

// Vad finds the speech in a WAV file.
func (e *Engine) Vad(ctx context.Context, model, path string) ([]SpeechPart, error) {
	reply, err := decode[struct {
		Speech []SpeechPart `json:"speech"`
	}](e.Call(ctx, "vad", map[string]any{"model": model, "path": path}, nil))
	return reply.Speech, err
}

// VadStart opens a VAD session.
func (e *Engine) VadStart(ctx context.Context, session uint64, model string) error {
	_, err := e.Call(ctx, "vad_start", map[string]any{"session": session, "model": model}, nil)
	return err
}

// Diarize finds who speaks when in a WAV file. speakers 0 lets the engine
// find the number.
func (e *Engine) Diarize(ctx context.Context, model, path string, speakers uint32) (Diarization, error) {
	f := map[string]any{"model": model, "path": path}
	if speakers > 0 {
		f["speakers"] = speakers
	}
	return decode[Diarization](e.Call(ctx, "diarize", f, nil))
}

// Shutdown asks the engine to exit.
func (e *Engine) Shutdown(ctx context.Context) error {
	_, err := e.Call(ctx, "shutdown", nil, nil)
	return err
}
