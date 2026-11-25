// Package localengine is the speech engine for Linux and Windows. It speaks the NDJSON protocol of
// docs/engine-protocol.md (version 1) on stdin and stdout, and runs
// sherpa-onnx models (speech, VAD, and diarization), whisper.cpp models
// (Windows), and parakeet.cpp models (Linux). The native libraries are loaded
// at run time with purego, so the engine builds with CGO_ENABLED=0.
package localengine

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strconv"
	"sync"
)

// ProtocolVersion is the version of the engine protocol.
const ProtocolVersion = 1

// Features are the optional parts of the protocol that this engine has, sent
// in hello. It accepts prepare but does nothing with it, so prepare is not
// listed.
var Features = []string{"catalog", "concurrent_streams", "detected_language", "diarization", "keep_preview", "segments", "store_v1", "unload", "vad"}

// Version is the Sayso release the engine comes with. Release builds set it
// with -ldflags "-X github.com/watzon/sayso/internal/localengine.Version=...".
var Version = "0.16.0"

// codedError is an error with a stable code for the error reply, for example
// too_many_streams.
type codedError struct {
	code    string
	message string
}

func (e *codedError) Error() string { return e.message }

// fields are the fields of one message.
type fields = map[string]any

// Output writes protocol lines. One write call per line, under a lock, so
// lines from different goroutines never mix.
type Output struct {
	mu   sync.Mutex
	sink io.Writer
}

// NewOutput writes protocol lines to sink.
func NewOutput(sink io.Writer) *Output {
	return &Output{sink: sink}
}

// Send writes one message. f can be nil.
func (o *Output) Send(kind string, id *string, f fields) {
	object := make(map[string]any, len(f)+3)
	for k, v := range f {
		object[k] = v
	}
	object["v"] = ProtocolVersion
	object["type"] = kind
	if id != nil {
		object["id"] = *id
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(object); err != nil {
		buf.Reset()
		fmt.Fprintf(&buf, `{"v":1,"type":"error","message":%q}`+"\n", "cannot encode a reply: "+err.Error())
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	// A write error means the parent closed the pipe. The stdin loop then ends the process.
	_, _ = o.sink.Write(buf.Bytes())
}

// OK sends an ok reply.
func (o *Output) OK(id *string, f fields) { o.Send("ok", id, f) }

// Error sends an error reply with a message only.
func (o *Output) Error(message string, id *string) {
	o.Send("error", id, fields{"message": message})
}

// Failure sends an error reply for err, with its code when it has one.
func (o *Output) Failure(err error, id *string) {
	f := fields{"message": err.Error()}
	var coded *codedError
	if errors.As(err, &coded) {
		f["code"] = coded.code
	}
	o.Send("error", id, f)
}

// Log sends a log event.
func (o *Output) Log(level, message string) {
	o.Send("log", nil, fields{"level": level, "message": message})
}

// Request is one parsed request line.
type Request struct {
	ID     *string
	Kind   string
	fields map[string]json.RawMessage
}

var errInvalidLine = errors.New("invalid JSON line")

// ParseRequest reads one request line.
func ParseRequest(line string) (*Request, error) {
	var object map[string]json.RawMessage
	if err := json.Unmarshal([]byte(line), &object); err != nil || object == nil {
		return nil, errInvalidLine
	}
	var kind string
	if raw, ok := object["type"]; !ok || json.Unmarshal(raw, &kind) != nil {
		return nil, errors.New("missing type")
	}
	request := &Request{Kind: kind, fields: object}
	var id string
	if raw, ok := object["id"]; ok && json.Unmarshal(raw, &id) == nil {
		request.ID = &id
	}
	return request, nil
}

// String is a required string field.
func (r *Request) String(key string) (string, error) {
	if s, ok := r.OptionalString(key); ok {
		return s, nil
	}
	return "", fmt.Errorf("missing string field %s", key)
}

// OptionalString is a string field that can be missing.
func (r *Request) OptionalString(key string) (string, bool) {
	raw, ok := r.fields[key]
	if !ok {
		return "", false
	}
	var s string
	if json.Unmarshal(raw, &s) != nil {
		return "", false
	}
	return s, true
}

// Uint is a required non-negative integer field.
func (r *Request) Uint(key string) (uint64, error) {
	if n, ok := r.OptionalUint(key); ok {
		return n, nil
	}
	return 0, fmt.Errorf("missing integer field %s", key)
}

// OptionalUint is a non-negative integer field that can be missing.
func (r *Request) OptionalUint(key string) (uint64, bool) {
	raw, ok := r.fields[key]
	if !ok {
		return 0, false
	}
	n, err := strconv.ParseUint(string(bytes.TrimSpace(raw)), 10, 64)
	return n, err == nil
}

// OptionalBool is a boolean field. Missing or another type is false.
func (r *Request) OptionalBool(key string) bool {
	var b bool
	if raw, ok := r.fields[key]; ok && json.Unmarshal(raw, &b) == nil {
		return b
	}
	return false
}

// OptionalValue is a JSON value that can be missing. null counts as missing.
func (r *Request) OptionalValue(key string) (json.RawMessage, bool) {
	raw, ok := r.fields[key]
	if !ok || string(bytes.TrimSpace(raw)) == "null" {
		return nil, false
	}
	return raw, true
}

// Has reports whether the field is present, also with null.
func (r *Request) Has(key string) bool {
	_, ok := r.fields[key]
	return ok
}

// Strings is a list of strings. A missing field, another type, or an item
// that is not a string is left out.
func (r *Request) Strings(key string) []string {
	raw, ok := r.fields[key]
	if !ok {
		return nil
	}
	var items []json.RawMessage
	if json.Unmarshal(raw, &items) != nil {
		return nil
	}
	var out []string
	for _, item := range items {
		var s string
		if json.Unmarshal(item, &s) == nil {
			out = append(out, s)
		}
	}
	return out
}
