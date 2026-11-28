// Command sayso-fake-cleanup is a deterministic cleanup runtime of the
// cleanup protocol v1, for the tests of the SDKs. It counts a token for each
// word, and its completion is the text of the prompt after the last "text:"
// line, without filler words, with a capital first letter and a final
// period.
//
// SAYSO_FAKE_SCRIPT is a JSON object that injects faults:
//
//	{"stderr": ["line", ...], "exit_at_start": 3, "crash_at": 4,
//	 "delay_ms": {"complete": 200},
//	 "fail": {"load": {"code": "load_failed", "message": "..."}},
//	 "libraries": "missing", "backend": "cpu"}
package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"
)

type failure struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type script struct {
	Stderr      []string           `json:"stderr"`
	ExitAtStart int                `json:"exit_at_start"`
	CrashAt     int                `json:"crash_at"`
	DelayMs     map[string]int     `json:"delay_ms"`
	Fail        map[string]failure `json:"fail"`
	Libraries   string             `json:"libraries"`
	Backend     string             `json:"backend"`
}

func main() {
	var s script
	if text := os.Getenv("SAYSO_FAKE_SCRIPT"); text != "" {
		if err := json.Unmarshal([]byte(text), &s); err != nil {
			fmt.Fprintln(os.Stderr, "sayso-fake-cleanup: SAYSO_FAKE_SCRIPT:", err)
			os.Exit(2)
		}
	}
	for _, line := range s.Stderr {
		fmt.Fprintln(os.Stderr, line)
	}
	if s.ExitAtStart != 0 {
		os.Exit(s.ExitAtStart)
	}
	os.Exit(serve(&runtime{script: s}, os.Stdin, os.Stdout))
}

const protocolVersion = 1

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

type reply map[string]any

type protoError struct{ code, message string }

func errorf(code, format string, args ...any) *protoError {
	return &protoError{code, fmt.Sprintf(format, args...)}
}

func errorReply(id string, e *protoError) reply {
	return reply{"v": protocolVersion, "id": id, "type": "error", "code": e.code, "message": e.message}
}

type runtime struct {
	script   script
	requests int
	loaded   *loaded
}

type loaded struct {
	id, path string
	nCtx     uint32
}

// serve answers one request at a time until in ends. It gives the exit code.
func serve(r *runtime, in io.Reader, out io.Writer) int {
	reader := bufio.NewReaderSize(in, 1<<20)
	enc := json.NewEncoder(out)
	enc.SetEscapeHTML(false)
	for {
		line, err := reader.ReadBytes('\n')
		if text := bytes.TrimSpace(line); len(text) > 0 {
			var rep reply
			if !utf8.Valid(line) {
				rep = errorReply("", errorf("bad_request", "the line is not UTF-8"))
			} else {
				var crash bool
				rep, crash = r.handleLine(string(text))
				if crash {
					return 1
				}
			}
			if werr := enc.Encode(rep); werr != nil {
				return 1
			}
		}
		if err != nil {
			return 0
		}
	}
}

func idOf(line string) string {
	var v map[string]any
	if json.Unmarshal([]byte(line), &v) != nil {
		return ""
	}
	id, _ := v["id"].(string)
	return id
}

func (r *runtime) handleLine(line string) (reply, bool) {
	var req request
	if err := json.Unmarshal([]byte(line), &req); err != nil {
		return errorReply(idOf(line), errorf("bad_request", "bad request: %v", err)), false
	}
	if req.V == nil || req.ID == nil || req.Type == nil {
		return errorReply(idOf(line), errorf("bad_request", "bad request: a line needs v, id, and type")), false
	}
	r.requests++
	if r.script.CrashAt > 0 && r.requests >= r.script.CrashAt {
		fmt.Fprintf(os.Stderr, "sayso-fake-cleanup: crash at request %d\n", r.requests)
		return nil, true
	}
	id := *req.ID
	if *req.V != protocolVersion && *req.Type != "hello" {
		return errorReply(id, errorf("bad_request", "unsupported protocol version %d", *req.V)), false
	}
	if d := r.script.DelayMs[*req.Type]; d > 0 {
		time.Sleep(time.Duration(d) * time.Millisecond)
	}
	if f, ok := r.script.Fail[*req.Type]; ok {
		return errorReply(id, &protoError{f.Code, f.Message}), false
	}
	out, e := r.handle(&req)
	if e != nil {
		return errorReply(id, e), false
	}
	out["v"], out["id"] = protocolVersion, id
	return out, false
}

func missing(field string) *protoError {
	return errorf("bad_request", "bad request: missing field `%s`", field)
}

func (r *runtime) handle(req *request) (reply, *protoError) {
	switch *req.Type {
	case "hello":
		libs, backend := "found", "cpu"
		if r.script.Libraries != "" {
			libs = r.script.Libraries
		}
		if r.script.Backend != "" {
			backend = r.script.Backend
		}
		return reply{"type": "hello", "version": protocolVersion, "llama_cpp": "fake", "backend": backend, "libraries": libs}, nil
	case "load":
		switch {
		case req.ModelID == nil:
			return nil, missing("model_id")
		case req.Path == nil:
			return nil, missing("path")
		case req.NCtx == nil:
			return nil, missing("n_ctx")
		case *req.NCtx == 0:
			return nil, errorf("bad_request", "n_ctx must be at least 1")
		}
		if fi, err := os.Stat(*req.Path); err != nil || !fi.Mode().IsRegular() {
			return nil, errorf("load_failed", "no model file at %s", *req.Path)
		}
		r.loaded = &loaded{id: *req.ModelID, path: *req.Path, nCtx: *req.NCtx}
		return reply{"type": "loaded", "model_id": *req.ModelID, "load_ms": 3}, nil
	case "count":
		switch {
		case req.ModelID == nil:
			return nil, missing("model_id")
		case req.Text == nil:
			return nil, missing("text")
		}
		if _, e := r.model(*req.ModelID); e != nil {
			return nil, e
		}
		return reply{"type": "count", "tokens": tokens(*req.Text)}, nil
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
		return complete(l, *req.Prompt, *req.End, *req.MaxTokens)
	case "unload":
		r.loaded = nil
		return reply{"type": "unloaded"}, nil
	}
	return nil, errorf("bad_request", "bad request: unknown variant `%s`", *req.Type)
}

func (r *runtime) model(id string) (*loaded, *protoError) {
	switch {
	case r.loaded == nil:
		return nil, errorf("not_loaded", "no model is loaded")
	case r.loaded.id != id:
		return nil, errorf("wrong_model", "%s is loaded, not %s", r.loaded.id, id)
	}
	return r.loaded, nil
}

func tokens(text string) int { return len(strings.Fields(text)) }

var fillers = map[string]bool{"um": true, "uh": true, "erm": true, "hmm": true}

// answer is the cleaned text of the last "text:" part of the prompt.
func answer(prompt string) string {
	text := prompt
	if i := strings.LastIndex(text, "text:"); i >= 0 {
		text = text[i+len("text:"):]
	}
	if i := strings.Index(text, "\nout:"); i >= 0 {
		text = text[:i]
	}
	var words []string
	for _, w := range strings.Fields(text) {
		if !fillers[strings.ToLower(strings.Trim(w, ",."))] {
			words = append(words, w)
		}
	}
	out := strings.Join(words, " ")
	if out == "" {
		return ""
	}
	first, size := utf8.DecodeRuneInString(out)
	out = string(unicode.ToUpper(first)) + out[size:]
	if !strings.ContainsAny(out[len(out)-1:], ".!?") {
		out += "."
	}
	return " " + out
}

func complete(l *loaded, prompt string, end []string, maxTokens uint32) (reply, *protoError) {
	in := tokens(prompt)
	if in == 0 {
		return nil, errorf("bad_request", "the prompt has no tokens")
	}
	stop := "end"
	text := answer(prompt)
	switch {
	case uint32(in)+maxTokens > l.nCtx:
		stop, text = "context", ""
	case maxTokens == 0:
		stop, text = "limit", ""
	case uint32(tokens(text)) > maxTokens:
		stop, text = "limit", " "+strings.Join(strings.Fields(text)[:maxTokens], " ")
	}
	for _, e := range end {
		if i := strings.Index(text, e); e != "" && i >= 0 {
			text = text[:i]
		}
	}
	return reply{"type": "completion", "text": text, "stop": stop, "tokens_in": in, "tokens_out": tokens(text), "ms": 1}, nil
}
