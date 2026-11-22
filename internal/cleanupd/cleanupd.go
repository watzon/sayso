// Package cleanupd is the cleanup runtime: the request loop and the state of
// cmd/sayso-cleanup. It speaks protocol 1 of the cleanup runtime (NDJSON on
// stdin and stdout, one request at a time), the same protocol as the Rust
// runtime in native/cleanup, so internal/engine's CleanupClient and the SDKs
// drive either one. See native/cleanup/NOTES.md, "Protocol".
//
// The model work is behind Engine and Model, so this package does not load
// llama.cpp: cleanupd/llama implements them with yzma.
package cleanupd

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"strings"
	"time"
	"unicode/utf8"
)

// ProtocolVersion is the protocol of the cleanup runtime.
const ProtocolVersion = 1

// Error codes of the protocol.
const (
	BadRequest   = "bad_request"
	NotLoaded    = "not_loaded"
	WrongModel   = "wrong_model"
	LoadFailed   = "load_failed"
	DecodeFailed = "decode_failed"
)

// Stops of a completion. Only StopEnd is a whole answer.
const (
	StopEnd     = "end"
	StopLimit   = "limit"
	StopContext = "context"
)

// Error is an error reply.
type Error struct{ Code, Message string }

func (e *Error) Error() string { return e.Code + ": " + e.Message }

func errorf(code, format string, args ...any) *Error {
	return &Error{Code: code, Message: fmt.Sprintf(format, args...)}
}

// Engine loads models. The backend of llama.cpp starts at the first Open.
type Engine interface {
	// Backend is "metal" or "cpu".
	Backend() string
	// Version names the llama.cpp build.
	Version() string
	// HasLibraries is true when the llama.cpp libraries can be found (the
	// hello says "libraries": "found" or "missing").
	HasLibraries() bool
	// Open loads a model file with one context of nCtx tokens.
	Open(path string, nCtx uint32) (Model, error)
	// Close frees the backend. Every model must be closed before.
	Close()
}

// Model is a loaded model with its context.
type Model interface {
	// Tokens is the token count of text (no special tokens added, special
	// tokens in the text parsed).
	Tokens(text string) (int, error)
	// Complete decodes greedily (see Runtime.complete for the rules).
	Complete(prompt string, end []string, maxTokens, nCtx uint32) (Completion, error)
	// Close frees the context, then the model.
	Close()
}

// Completion is the result of a completion. Text is bytes, not text: one token
// can be half a character.
type Completion struct {
	Text      []byte
	Stop      string
	TokensIn  uint32
	TokensOut uint32
}

type loaded struct {
	id, path string
	nCtx     uint32
	model    Model
}

// Runtime is the state of the runtime: at most one loaded model.
type Runtime struct {
	engine Engine
	loaded *loaded
	closed bool
}

// NewRuntime makes a runtime on engine.
func NewRuntime(engine Engine) *Runtime { return &Runtime{engine: engine} }

// Unload frees the loaded model: its context, then the model.
func (r *Runtime) Unload() {
	if r.loaded != nil {
		r.loaded.model.Close()
		slog.Info("unloaded", "model", r.loaded.id)
		r.loaded = nil
	}
}

// Close frees the model (context, then model) and then the backend. A second
// call does nothing. ggml-metal asserts at the exit of the process when the
// backend still holds resources, so the runtime calls this before it exits.
func (r *Runtime) Close() {
	if r.closed {
		return
	}
	r.closed = true
	r.Unload()
	r.engine.Close()
}

// request is one request line. The pointers tell a missing field from a zero.
type request struct {
	V         *uint32   `json:"v"`
	ID        *string   `json:"id"`
	Type      *string   `json:"type"`
	ModelID   *string   `json:"model_id"`
	Path      *string   `json:"path"`
	NCtx      *uint32   `json:"n_ctx"`
	Text      *string   `json:"text"`
	Prompt    *string   `json:"prompt"`
	End       *[]string `json:"end"`
	MaxTokens *uint32   `json:"max_tokens"`
}

// reply is one reply line: v, id, type, and the fields of the type.
type reply map[string]any

func errorReply(id string, e *Error) reply {
	return reply{"v": ProtocolVersion, "id": id, "type": "error", "code": e.Code, "message": e.Message}
}

// Serve reads requests until the end of in. Each request is handled, and its
// reply written, before the next line is read. A write error ends it.
func Serve(r *Runtime, in io.Reader, out io.Writer) error {
	reader := bufio.NewReaderSize(in, 1<<20)
	enc := json.NewEncoder(out)
	enc.SetEscapeHTML(false)
	for {
		line, err := reader.ReadBytes('\n')
		if len(line) > 0 {
			var rep reply
			switch text := bytes.TrimSpace(line); {
			case !utf8.Valid(line):
				rep = errorReply("", errorf(BadRequest, "the line is not UTF-8"))
			case len(text) == 0:
				rep = nil
			default:
				rep = r.handleLine(string(text))
			}
			if rep != nil {
				// Encode writes the line with its new line in one write.
				if werr := enc.Encode(rep); werr != nil {
					return werr
				}
			}
		}
		if errors.Is(err, io.EOF) {
			return nil
		}
		if err != nil {
			return err
		}
	}
}

// handleLine handles one line. It never panics: a panic while it handles the
// request becomes decode_failed, and the runtime stays usable.
func (r *Runtime) handleLine(line string) reply {
	var req request
	if err := json.Unmarshal([]byte(line), &req); err != nil {
		return errorReply(idOf(line), errorf(BadRequest, "bad request: %v", err))
	}
	if req.V == nil || req.ID == nil || req.Type == nil {
		return errorReply(idOf(line), errorf(BadRequest, "bad request: a line needs v, id, and type"))
	}
	id := *req.ID
	if *req.V != ProtocolVersion && *req.Type != "hello" {
		return errorReply(id, errorf(BadRequest, "unsupported protocol version %d", *req.V))
	}
	return r.guarded(id, func() (reply, *Error) { return r.handle(&req) })
}

func (r *Runtime) guarded(id string, work func() (reply, *Error)) (rep reply) {
	defer func() {
		if p := recover(); p != nil {
			slog.Error("panic while handling a request", "panic", p)
			rep = errorReply(id, errorf(DecodeFailed, "panic: %v", p))
		}
	}()
	out, e := work()
	if e != nil {
		return errorReply(id, e)
	}
	out["v"], out["id"] = ProtocolVersion, id
	return out
}

func missing(field string) *Error {
	return errorf(BadRequest, "bad request: missing field `%s`", field)
}

func (r *Runtime) handle(req *request) (reply, *Error) {
	switch *req.Type {
	case "hello":
		libs := "missing"
		if r.engine.HasLibraries() {
			libs = "found"
		}
		return reply{"type": "hello", "version": ProtocolVersion, "llama_cpp": r.engine.Version(), "backend": r.engine.Backend(), "libraries": libs}, nil
	case "load":
		switch {
		case req.ModelID == nil:
			return nil, missing("model_id")
		case req.Path == nil:
			return nil, missing("path")
		case req.NCtx == nil:
			return nil, missing("n_ctx")
		}
		return r.load(*req.ModelID, *req.Path, *req.NCtx)
	case "count":
		switch {
		case req.ModelID == nil:
			return nil, missing("model_id")
		case req.Text == nil:
			return nil, missing("text")
		}
		l, e := r.model(*req.ModelID)
		if e != nil {
			return nil, e
		}
		n, err := l.model.Tokens(*req.Text)
		if err != nil {
			return nil, errorf(DecodeFailed, "%v", err)
		}
		return reply{"type": "count", "tokens": n}, nil
	case "complete":
		switch {
		case req.ModelID == nil:
			return nil, missing("model_id")
		case req.Prompt == nil:
			return nil, missing("prompt")
		case req.End == nil:
			return nil, missing("end")
		case req.MaxTokens == nil:
			return nil, missing("max_tokens")
		}
		l, e := r.model(*req.ModelID)
		if e != nil {
			return nil, e
		}
		return r.complete(l, *req.Prompt, *req.End, *req.MaxTokens)
	case "unload":
		r.Unload()
		return reply{"type": "unloaded"}, nil
	}
	return nil, errorf(BadRequest, "bad request: unknown variant `%s`", *req.Type)
}

func (r *Runtime) load(id, path string, nCtx uint32) (reply, *Error) {
	if nCtx == 0 {
		return nil, errorf(BadRequest, "n_ctx must be at least 1")
	}
	if l := r.loaded; l != nil && l.id == id && l.path == path && l.nCtx == nCtx {
		return reply{"type": "loaded", "model_id": id, "load_ms": 0}, nil
	}
	// Free the memory of the old model before the new one loads.
	r.Unload()
	started := time.Now()
	if fi, err := os.Stat(path); err != nil || !fi.Mode().IsRegular() {
		return nil, errorf(LoadFailed, "no model file at %s", path)
	}
	m, err := r.engine.Open(path, nCtx)
	if err != nil {
		return nil, errorf(LoadFailed, "%v", err)
	}
	ms := time.Since(started).Milliseconds()
	slog.Info("loaded", "model", id, "path", path, "n_ctx", nCtx, "ms", ms)
	r.loaded = &loaded{id: id, path: path, nCtx: nCtx, model: m}
	return reply{"type": "loaded", "model_id": id, "load_ms": ms}, nil
}

func (r *Runtime) model(id string) (*loaded, *Error) {
	switch {
	case r.loaded == nil:
		return nil, errorf(NotLoaded, "no model is loaded")
	case r.loaded.id != id:
		return nil, errorf(WrongModel, "%s is loaded, not %s", r.loaded.id, id)
	}
	return r.loaded, nil
}

// complete runs one completion. The rules (as native/cleanup):
//   - the prompt is tokenized with the BOS rule of the model; no tokens is bad_request;
//   - the prompt plus maxTokens over the context gives stop "context" with no text;
//   - maxTokens 0 gives stop "limit";
//   - decoding is greedy from an empty KV cache, and stops at an end-of-generation
//     token or the first `end` string ("end", the text cut before it), at
//     maxTokens ("limit"), or when the context is full ("context").
func (r *Runtime) complete(l *loaded, prompt string, end []string, maxTokens uint32) (reply, *Error) {
	started := time.Now()
	c, err := l.model.Complete(prompt, end, maxTokens, l.nCtx)
	if err != nil {
		var e *Error
		if errors.As(err, &e) {
			return nil, e
		}
		return nil, errorf(DecodeFailed, "%v", err)
	}
	return reply{"type": "completion", "text": strings.ToValidUTF8(string(c.Text), "�"), "stop": c.Stop,
		"tokens_in": c.TokensIn, "tokens_out": c.TokensOut, "ms": time.Since(started).Milliseconds()}, nil
}

// idOf is the id of a line that is not a valid request, when it has one.
func idOf(line string) string {
	var v map[string]any
	if json.Unmarshal([]byte(line), &v) != nil {
		return ""
	}
	id, _ := v["id"].(string)
	return id
}

// FindEnd is the first index in text[from:] where one of ends starts, or -1.
// An empty end string never matches.
func FindEnd(text []byte, ends []string, from int) int {
	best := -1
	for _, e := range ends {
		if e == "" || from > len(text) {
			continue
		}
		if i := bytes.Index(text[from:], []byte(e)); i >= 0 && (best < 0 || i+from < best) {
			best = i + from
		}
	}
	return best
}

// LongestEnd is the length of the longest end string.
func LongestEnd(ends []string) int {
	n := 0
	for _, e := range ends {
		n = max(n, len(e))
	}
	return n
}
