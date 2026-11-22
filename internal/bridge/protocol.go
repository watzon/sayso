// Package bridge is the Host API of Sayso (docs/extensions.md, "Host API"):
// the messages of crates/sayso-protocol, the host of crates/sayso-host (the
// socket or named pipe, the identity check, apps.toml, the approval
// question, the capabilities, jobs, and live sessions).
//
// Each message is one JSON object on one line, with "v":1, a string type,
// and an optional string id. The host writes the keys in the order of the
// Rust host (serde_json with preserve_order): type, the fields of the
// message, v, then id.
package bridge

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"slices"
	"strconv"
	"strings"
	"unicode"
)

// Protocol is the version of the protocol, in v of each message and in
// protocol of hello.
const Protocol = 1

// Capability is a name of the protocol. A name that this version does not
// know is kept as it is (Capability::Other in Rust).
type Capability string

const (
	CapModelsRead       Capability = "models.read"
	CapModelsDownload   Capability = "models.download"
	CapTranscribeFile   Capability = "transcribe.file"
	CapSessionLive      Capability = "session.live"
	CapAudioMic         Capability = "audio.mic"
	CapAudioSystem      Capability = "audio.system"
	CapAiEnhance        Capability = "ai.enhance"
	CapDictationObserve Capability = "dictation.observe"
)

// AllCapabilities are the capabilities that this version knows.
var AllCapabilities = []Capability{CapModelsRead, CapModelsDownload, CapTranscribeFile, CapSessionLive, CapAudioMic, CapAudioSystem, CapAiEnhance, CapDictationObserve}

// Known is false for a name that this version does not know.
func (c Capability) Known() bool { return slices.Contains(AllCapabilities, c) }

// ErrorCode is the code of an error reply.
type ErrorCode string

const (
	CodeNotApproved        ErrorCode = "not_approved"
	CodeCapability         ErrorCode = "capability"
	CodeBusy               ErrorCode = "busy"
	CodeDeclined           ErrorCode = "declined"
	CodeModelNotDownloaded ErrorCode = "model_not_downloaded"
	CodeWrongRole          ErrorCode = "wrong_role"
	CodeUnsupported        ErrorCode = "unsupported"
	CodeBadRequest         ErrorCode = "bad_request"
	CodeTooManyStreams     ErrorCode = "too_many_streams"
	CodeUnknownModel       ErrorCode = "unknown_model"
	CodeModelRetired       ErrorCode = "model_retired"
	CodeUnavailable        ErrorCode = "unavailable"
	CodeRefused            ErrorCode = "refused"
	CodeTimeout            ErrorCode = "timeout"
	CodePermission         ErrorCode = "permission"
	CodeHostNotRunning     ErrorCode = "host_not_running"
	CodeConnectionLost     ErrorCode = "connection_lost"
	CodeHostClosing        ErrorCode = "host_closing"
)

// Role is the role of a model.
type Role string

const (
	RoleFinal       Role = "final"
	RoleStreaming   Role = "streaming"
	RoleVad         Role = "vad"
	RoleDiarization Role = "diarization"
)

// ModelState is the state of a model.
type ModelState string

const (
	StateNotDownloaded ModelState = "not_downloaded"
	StateDownloading   ModelState = "downloading"
	StateDownloaded    ModelState = "downloaded"
	StateOptimizing    ModelState = "optimizing"
	StateReady         ModelState = "ready"
	StateFailed        ModelState = "failed"
)

// TimestampLevel is what times a model gives.
type TimestampLevel string

const (
	TimestampsWord    TimestampLevel = "word"
	TimestampsSegment TimestampLevel = "segment"
	TimestampsNone    TimestampLevel = "none"
)

// Source is an audio source of a live session.
type Source string

const (
	SourceMic    Source = "mic"
	SourceSystem Source = "system"
	SourceClient Source = "client"
)

// SourceEndReason is why a source ended.
type SourceEndReason string

const (
	EndPermission SourceEndReason = "permission"
	EndFailed     SourceEndReason = "failed"
)

// F64 is a float that JSON writes as serde_json does: an integral value
// keeps ".0".
type F64 float64

func (f F64) MarshalJSON() ([]byte, error) {
	v := float64(f)
	if math.IsNaN(v) || math.IsInf(v, 0) {
		return []byte("null"), nil
	}
	if v == math.Trunc(v) && math.Abs(v) < 1e16 {
		return []byte(strconv.FormatFloat(v, 'f', 1, 64)), nil
	}
	return json.Marshal(v)
}

// Word is a word with its times.
type Word struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
}

// Segment is a part of a transcript with its times.
type Segment struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
	Words   []Word `json:"words,omitempty"`
}

// SpeechPart is a part of audio with speech.
type SpeechPart struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
}

// Turn is a part of audio of one speaker. The speaker label reads from a
// string or a number.
type Turn struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Speaker string `json:"speaker"`
}

func (t *Turn) UnmarshalJSON(b []byte) error {
	var raw struct {
		StartMs *uint64         `json:"start_ms"`
		EndMs   *uint64         `json:"end_ms"`
		Speaker json.RawMessage `json:"speaker"`
	}
	if err := json.Unmarshal(b, &raw); err != nil {
		return err
	}
	if raw.StartMs == nil || raw.EndMs == nil || raw.Speaker == nil {
		return errors.New("missing field of a turn")
	}
	t.StartMs, t.EndMs = *raw.StartMs, *raw.EndMs
	var text string
	if json.Unmarshal(raw.Speaker, &text) == nil {
		t.Speaker = text
		return nil
	}
	var n uint64
	if err := json.Unmarshal(raw.Speaker, &n); err != nil {
		return fmt.Errorf("speaker: %w", err)
	}
	t.Speaker = strconv.FormatUint(n, 10)
	return nil
}

// ModelInfo is a model in list_models.
type ModelInfo struct {
	ID         string         `json:"id"`
	Name       string         `json:"name"`
	Roles      []Role         `json:"roles"`
	State      ModelState     `json:"state"`
	Languages  []string       `json:"languages"`
	Timestamps TimestampLevel `json:"timestamps"`
	SizeBytes  uint64         `json:"size_bytes"`
	Choice     []Role         `json:"choice,omitempty"`
}

// ---- Requests ----

// Request is a request of an app.
type Request interface{ Kind() string }

// Hello is the first request of a client.
type Hello struct {
	SDK          *string      `json:"sdk,omitempty"`
	Protocol     uint32       `json:"protocol"`
	Features     []string     `json:"features"`
	App          *AppName     `json:"app,omitempty"`
	Capabilities []Capability `json:"capabilities,omitempty"`
}

// AppName is app of hello.
type AppName struct {
	Name string `json:"name"`
}

type ListModels struct {
	Role *Role `json:"role,omitempty"`
}

type DownloadModel struct {
	Model  string `json:"model"`
	Reason string `json:"reason"`
}

type TranscribeFile struct {
	Path             string  `json:"path"`
	Model            *string `json:"model,omitempty"`
	VadModel         *string `json:"vad_model,omitempty"`
	Language         *string `json:"language,omitempty"`
	Diarize          *bool   `json:"diarize,omitempty"`
	DiarizationModel *string `json:"diarization_model,omitempty"`
	Speakers         *uint32 `json:"speakers,omitempty"`
}

type CancelJob struct {
	Job uint64 `json:"job"`
}

type SessionStart struct {
	Sources          []Source `json:"sources"`
	Model            *string  `json:"model,omitempty"`
	PreviewModel     *string  `json:"preview_model,omitempty"`
	VadModel         *string  `json:"vad_model,omitempty"`
	Language         *string  `json:"language,omitempty"`
	Diarize          *bool    `json:"diarize,omitempty"`
	DiarizationModel *string  `json:"diarization_model,omitempty"`
	RecordTo         *string  `json:"record_to,omitempty"`
	SilenceLimitMs   *uint64  `json:"silence_limit_ms,omitempty"`
}

type SessionAudio struct {
	Session uint64 `json:"session"`
	PCM     string `json:"pcm"`
}

type SessionStop struct {
	Session uint64 `json:"session"`
}

type Enhance struct {
	Text     *string `json:"text,omitempty"`
	Style    *string `json:"style,omitempty"`
	Prompt   *string `json:"prompt,omitempty"`
	Provider *string `json:"provider,omitempty"`
}

type Subscribe struct {
	Events []string `json:"events"`
}

// Bye asks the host to reply ok and close the connection.
type Bye struct{}

func (Hello) Kind() string          { return "hello" }
func (ListModels) Kind() string     { return "list_models" }
func (DownloadModel) Kind() string  { return "download_model" }
func (TranscribeFile) Kind() string { return "transcribe_file" }
func (CancelJob) Kind() string      { return "cancel_job" }
func (SessionStart) Kind() string   { return "session_start" }
func (SessionAudio) Kind() string   { return "session_audio" }
func (SessionStop) Kind() string    { return "session_stop" }
func (Enhance) Kind() string        { return "enhance" }
func (Subscribe) Kind() string      { return "subscribe" }
func (Bye) Kind() string            { return "bye" }

// RequestTypes are the types of requests, in the order of the protocol.
var RequestTypes = []string{"hello", "list_models", "download_model", "transcribe_file", "cancel_job", "session_start", "session_audio", "session_stop", "enhance", "subscribe", "bye"}

// requestFields are the fields that a request must have.
var requestFields = map[string][]string{
	"hello":           {"protocol"},
	"download_model":  {"model", "reason"},
	"transcribe_file": {"path"},
	"cancel_job":      {"job"},
	"session_start":   {"sources"},
	"session_audio":   {"session", "pcm"},
	"session_stop":    {"session"},
	"subscribe":       {"events"},
}

func newRequest(kind string) Request {
	switch kind {
	case "hello":
		return &Hello{}
	case "list_models":
		return &ListModels{}
	case "download_model":
		return &DownloadModel{}
	case "transcribe_file":
		return &TranscribeFile{}
	case "cancel_job":
		return &CancelJob{}
	case "session_start":
		return &SessionStart{}
	case "session_audio":
		return &SessionAudio{}
	case "session_stop":
		return &SessionStop{}
	case "enhance":
		return &Enhance{}
	case "subscribe":
		return &Subscribe{}
	case "bye":
		return &Bye{}
	}
	return nil
}

// SessionCapabilities are the capabilities that a live session with these
// sources needs.
func SessionCapabilities(sources []Source) []Capability {
	needed := []Capability{CapSessionLive}
	if slices.Contains(sources, SourceMic) {
		needed = append(needed, CapAudioMic)
	}
	if slices.Contains(sources, SourceSystem) {
		needed = append(needed, CapAudioSystem)
	}
	return needed
}

// RequestCapabilities are the capabilities that the user must grant before
// the host runs the request.
func RequestCapabilities(r Request) []Capability {
	switch r := r.(type) {
	case *ListModels:
		return []Capability{CapModelsRead}
	case *DownloadModel:
		return []Capability{CapModelsDownload}
	case *TranscribeFile:
		return []Capability{CapTranscribeFile}
	case *SessionStart:
		return SessionCapabilities(r.Sources)
	case *SessionAudio:
		return []Capability{CapSessionLive}
	case *Enhance:
		return []Capability{CapAiEnhance}
	case *Subscribe:
		return []Capability{CapDictationObserve}
	}
	return nil
}

// ---- Replies ----

// Reply is ok with fields, or error.
type Reply interface{ Kind() string }

// OK is an ok reply. Body is a struct whose fields are the fields of the
// reply, or nil for no fields.
type OK struct{ Body any }

// ErrorReply is an error reply. Message is English, for people.
type ErrorReply struct {
	Code       *ErrorCode  `json:"code,omitempty"`
	Message    string      `json:"message"`
	Capability *Capability `json:"capability,omitempty"`
	Reason     *string     `json:"reason,omitempty"`
}

func (OK) Kind() string          { return "ok" }
func (*ErrorReply) Kind() string { return "error" }

func (e *ErrorReply) Error() string {
	if e.Code != nil {
		return fmt.Sprintf("%s (%s)", e.Message, *e.Code)
	}
	return e.Message
}

func (o OK) MarshalJSON() ([]byte, error) {
	if o.Body == nil {
		return []byte("{}"), nil
	}
	return json.Marshal(o.Body)
}

// ReplyOK is ok with the fields of body (nil: no fields).
func ReplyOK(body any) Reply { return OK{Body: body} }

// ReplyError is an error reply. An empty code is no code.
func ReplyError(code ErrorCode, message string) Reply {
	e := &ErrorReply{Message: message}
	if code != "" {
		e.Code = &code
	}
	return e
}

type HelloReply struct {
	HostVersion  string       `json:"host_version"`
	Protocol     uint32       `json:"protocol"`
	Features     []string     `json:"features"`
	Capabilities []Capability `json:"capabilities"`
	Denied       []Capability `json:"denied"`
}

type ListModelsReply struct {
	Models []ModelInfo `json:"models"`
}

type JobReply struct {
	Job uint64 `json:"job"`
}

type SessionReply struct {
	Session  uint64   `json:"session"`
	Excludes []string `json:"excludes"`
}

type TextReply struct {
	Text string `json:"text"`
}

type EnhanceReply struct {
	Text     string `json:"text"`
	Provider string `json:"provider"`
	Model    string `json:"model"`
}

type SubscribeReply struct {
	Events []string `json:"events"`
}

// ---- Events ----

// Event is a message of the host that is not a reply.
type Event interface{ Kind() string }

type ModelStateEvent struct {
	Model   string     `json:"model"`
	State   ModelState `json:"state"`
	LoadMs  *uint64    `json:"load_ms,omitempty"`
	Preview *bool      `json:"preview,omitempty"`
	Message *string    `json:"message,omitempty"`
	Reason  *string    `json:"reason,omitempty"`
}

type DownloadProgress struct {
	Model      string `json:"model"`
	Fraction   F64    `json:"fraction"`
	BytesDone  uint64 `json:"bytes_done"`
	BytesTotal uint64 `json:"bytes_total"`
}

type JobProgress struct {
	Job      uint64 `json:"job"`
	Fraction F64    `json:"fraction"`
}

// SegmentEvent has the fields of its segment (serde flatten).
type SegmentEvent struct {
	Job     *uint64 `json:"job,omitempty"`
	Session *uint64 `json:"session,omitempty"`
	Source  *Source `json:"source,omitempty"`
	Segment
}

type SpeakersEvent struct {
	Job     *uint64 `json:"job,omitempty"`
	Session *uint64 `json:"session,omitempty"`
	Source  *Source `json:"source,omitempty"`
	Turns   []Turn  `json:"turns"`
}

type JobDone struct {
	Job  uint64 `json:"job"`
	Text string `json:"text"`
}

type JobFailed struct {
	Job     uint64     `json:"job"`
	Message string     `json:"message"`
	Code    *ErrorCode `json:"code,omitempty"`
}

type PartialEvent struct {
	Session   uint64  `json:"session"`
	Source    *Source `json:"source,omitempty"`
	Committed string  `json:"committed"`
	Tentative string  `json:"tentative"`
}

type SourceGap struct {
	Session uint64 `json:"session"`
	Source  Source `json:"source"`
	Ms      uint64 `json:"ms"`
}

type SourceEnded struct {
	Session uint64          `json:"session"`
	Source  Source          `json:"source"`
	Reason  SourceEndReason `json:"reason"`
}

type PartFailed struct {
	Session uint64     `json:"session"`
	Source  Source     `json:"source"`
	StartMs uint64     `json:"start_ms"`
	EndMs   uint64     `json:"end_ms"`
	Message string     `json:"message"`
	Code    *ErrorCode `json:"code,omitempty"`
}

type SessionEnded struct {
	Session uint64     `json:"session"`
	Text    string     `json:"text"`
	Code    *ErrorCode `json:"code,omitempty"`
}

type SilenceLimit struct {
	Session   uint64 `json:"session"`
	SilenceMs uint64 `json:"silence_ms"`
}

// DictationDone is the event of a dictation that ended (dictation.observe).
type DictationDone struct {
	Text       string  `json:"text"`
	Transcript string  `json:"transcript"`
	App        *string `json:"app,omitempty"`
	Site       *string `json:"site,omitempty"`
	Style      *string `json:"style,omitempty"`
	Time       string  `json:"time"`
}

type ApprovalWaiting struct{}
type HostClosing struct{}

func (ModelStateEvent) Kind() string  { return "model_state" }
func (DownloadProgress) Kind() string { return "download_progress" }
func (JobProgress) Kind() string      { return "job_progress" }
func (SegmentEvent) Kind() string     { return "segment" }
func (SpeakersEvent) Kind() string    { return "speakers" }
func (JobDone) Kind() string          { return "job_done" }
func (JobFailed) Kind() string        { return "job_failed" }
func (PartialEvent) Kind() string     { return "partial" }
func (SourceGap) Kind() string        { return "source_gap" }
func (SourceEnded) Kind() string      { return "source_ended" }
func (PartFailed) Kind() string       { return "part_failed" }
func (SessionEnded) Kind() string     { return "session_ended" }
func (SilenceLimit) Kind() string     { return "silence_limit" }
func (DictationDone) Kind() string    { return "dictation_done" }
func (ApprovalWaiting) Kind() string  { return "approval_waiting" }
func (HostClosing) Kind() string      { return "host_closing" }

// EventTypes are the types of events.
var EventTypes = []string{"model_state", "download_progress", "job_progress", "segment", "speakers", "job_done", "job_failed", "partial", "source_gap", "source_ended", "part_failed", "session_ended", "silence_limit", "dictation_done", "approval_waiting", "host_closing"}

func newEvent(kind string) Event {
	switch kind {
	case "model_state":
		return &ModelStateEvent{}
	case "download_progress":
		return &DownloadProgress{}
	case "job_progress":
		return &JobProgress{}
	case "segment":
		return &SegmentEvent{}
	case "speakers":
		return &SpeakersEvent{}
	case "job_done":
		return &JobDone{}
	case "job_failed":
		return &JobFailed{}
	case "partial":
		return &PartialEvent{}
	case "source_gap":
		return &SourceGap{}
	case "source_ended":
		return &SourceEnded{}
	case "part_failed":
		return &PartFailed{}
	case "session_ended":
		return &SessionEnded{}
	case "silence_limit":
		return &SilenceLimit{}
	case "dictation_done":
		return &DictationDone{}
	case "approval_waiting":
		return &ApprovalWaiting{}
	case "host_closing":
		return &HostClosing{}
	}
	return nil
}

// Text joins the committed and the tentative text with a space where the
// script needs one.
func (p PartialEvent) Text() string {
	committed := strings.TrimRightFunc(p.Committed, unicode.IsSpace)
	tentative := strings.TrimLeftFunc(p.Tentative, unicode.IsSpace)
	if committed == "" || tentative == "" {
		return committed + tentative
	}
	last := []rune(committed)[len([]rune(committed))-1]
	first := []rune(tentative)[0]
	closes := strings.ContainsRune(",.;:!?)]}%'’…、。，！？", first)
	sep := " "
	if closes || (IsUnspaced(last) && IsUnspaced(first)) {
		sep = ""
	}
	return committed + sep + tentative
}

// IsUnspaced is true for a character of a script that writes no spaces
// between words.
func IsUnspaced(c rune) bool {
	return (c >= 0x3040 && c <= 0x30FF) || (c >= 0x3400 && c <= 0x4DBF) || (c >= 0x4E00 && c <= 0x9FFF) || (c >= 0xF900 && c <= 0xFAFF) || (c >= 0x0E00 && c <= 0x0E7F)
}

// ---- Codec ----

// Message is a message with its id.
type Message[T any] struct {
	ID   *string
	Body T
}

// Unknown is a message of a type that this version does not know.
type Unknown struct {
	ID     *string
	Kind   string
	Fields map[string]json.RawMessage
}

// NotAMessage is a line that is not a JSON object with a string type.
type NotAMessage struct{ Message string }

func (e *NotAMessage) Error() string { return e.Message }

// BadFields is a known type with a required field that is missing or has the
// wrong type.
type BadFields struct {
	ID      *string
	Kind    string
	Message string
}

func (e *BadFields) Error() string { return fmt.Sprintf("bad %s message: %s", e.Kind, e.Message) }

// envelope reads the type and id of a line and gives the fields without v
// and id.
func envelope(line string) (kind string, id *string, fields map[string]json.RawMessage, err error) {
	trimmed := strings.TrimSpace(line)
	if err := json.Unmarshal([]byte(trimmed), &fields); err != nil || fields == nil {
		var any any
		if json.Unmarshal([]byte(trimmed), &any) != nil {
			msg := "the line is not JSON"
			if err != nil {
				msg += ": " + err.Error()
			}
			return "", nil, nil, &NotAMessage{msg}
		}
		return "", nil, nil, &NotAMessage{"the line is not a JSON object"}
	}
	rawKind, ok := fields["type"]
	if !ok || json.Unmarshal(rawKind, &kind) != nil {
		return "", nil, nil, &NotAMessage{"the message has no string type"}
	}
	if rawID, ok := fields["id"]; ok {
		var s string
		if json.Unmarshal(rawID, &s) == nil {
			id = &s
		}
		delete(fields, "id")
	}
	delete(fields, "v")
	return kind, id, fields, nil
}

func fill(kind string, fields map[string]json.RawMessage, required []string, into any) error {
	for _, key := range required {
		if v, ok := fields[key]; !ok || string(v) == "null" {
			return fmt.Errorf("missing field `%s`", key)
		}
	}
	b, _ := json.Marshal(fields)
	return json.Unmarshal(b, into)
}

// DecodeRequest reads one line of an app. It gives a *Message[Request], an
// *Unknown, or an error (*NotAMessage or *BadFields).
func DecodeRequest(line string) (any, error) {
	kind, id, fields, err := envelope(line)
	if err != nil {
		return nil, err
	}
	req := newRequest(kind)
	if req == nil {
		delete(fields, "type")
		return &Unknown{ID: id, Kind: kind, Fields: fields}, nil
	}
	if err := fill(kind, fields, requestFields[kind], req); err != nil {
		return nil, &BadFields{ID: id, Kind: kind, Message: err.Error()}
	}
	return &Message[Request]{ID: id, Body: req}, nil
}

// DecodeHost reads one line of the host: a reply (OKFields or *ErrorReply)
// or an event. It gives a *Message[any], an *Unknown, or an error.
func DecodeHost(line string) (any, error) {
	kind, id, fields, err := envelope(line)
	if err != nil {
		return nil, err
	}
	switch kind {
	case "ok":
		delete(fields, "type")
		return &Message[any]{ID: id, Body: OKFields(fields)}, nil
	case "error":
		var e ErrorReply
		if err := fill(kind, fields, []string{"message"}, &e); err != nil {
			return nil, &BadFields{ID: id, Kind: kind, Message: err.Error()}
		}
		return &Message[any]{ID: id, Body: &e}, nil
	}
	ev := newEvent(kind)
	if ev == nil {
		delete(fields, "type")
		return &Unknown{ID: id, Kind: kind, Fields: fields}, nil
	}
	if err := fill(kind, fields, nil, ev); err != nil {
		return nil, &BadFields{ID: id, Kind: kind, Message: err.Error()}
	}
	return &Message[any]{ID: id, Body: ev}, nil
}

// OKFields are the fields of an ok reply that DecodeHost read.
type OKFields map[string]json.RawMessage

// Parse reads the fields as v, for example a *HelloReply.
func (f OKFields) Parse(v any) error {
	b, _ := json.Marshal(map[string]json.RawMessage(f))
	return json.Unmarshal(b, v)
}

// Encode writes one message (a Request, Reply, or Event) as one line of
// JSON, without the newline.
func Encode(id *string, body interface{ Kind() string }) string {
	inner, err := json.Marshal(body)
	if err != nil {
		panic(fmt.Sprintf("a message serializes to JSON: %v", err))
	}
	var b bytes.Buffer
	b.WriteString(`{"type":`)
	kind, _ := json.Marshal(body.Kind())
	b.Write(kind)
	inner = bytes.TrimSpace(inner)
	if len(inner) > 2 && inner[0] == '{' {
		b.WriteByte(',')
		b.Write(inner[1 : len(inner)-1])
	}
	b.WriteString(`,"v":1`)
	if id != nil {
		b.WriteString(`,"id":`)
		idb, _ := json.Marshal(*id)
		b.Write(idb)
	}
	b.WriteByte('}')
	return b.String()
}

// ---- PCM ----

// SampleRate is the rate of PCM in the protocol.
const SampleRate = 16000

// EncodeI16 is base64 of little-endian 16-bit samples.
func EncodeI16(samples []int16) string {
	b := make([]byte, len(samples)*2)
	for i, s := range samples {
		b[2*i] = byte(uint16(s))
		b[2*i+1] = byte(uint16(s) >> 8)
	}
	return base64.StdEncoding.EncodeToString(b)
}

// EncodeF32 writes floats in [-1, 1] as 16-bit PCM.
func EncodeF32(samples []float32) string {
	out := make([]int16, len(samples))
	for i, s := range samples {
		out[i] = floatToI16(s)
	}
	return EncodeI16(out)
}

func floatToI16(s float32) int16 {
	v := math.Round(float64(s) * 32768)
	return int16(max(-32768, min(32767, v)))
}

// DecodeI16 reads base64 of 16-bit PCM.
func DecodeI16(pcm string) ([]int16, error) {
	b, err := base64.StdEncoding.DecodeString(pcm)
	if err != nil {
		return nil, fmt.Errorf("pcm is not base64: %v", err)
	}
	if len(b)%2 != 0 {
		return nil, fmt.Errorf("pcm has %d bytes, but 16-bit samples need an even number", len(b))
	}
	out := make([]int16, len(b)/2)
	for i := range out {
		out[i] = int16(uint16(b[2*i]) | uint16(b[2*i+1])<<8)
	}
	return out, nil
}

// DecodeF32 reads base64 of 16-bit PCM as floats.
func DecodeF32(pcm string) ([]float32, error) {
	s, err := DecodeI16(pcm)
	if err != nil {
		return nil, err
	}
	out := make([]float32, len(s))
	for i, v := range s {
		out[i] = float32(v) / 32768
	}
	return out, nil
}
