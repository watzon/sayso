package engine

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"sync"
	"time"
)

// The client of the cleanup runtime, sayso-cleanup (docs/cleanup-models.md,
// "Runtime and protocol"; crates/sayso-cleanup-client/src/runtime.rs).
// NDJSON on stdin/stdout, one request at a time. llama.cpp cannot stop a
// decode cheaply, so a request past its deadline kills the process; the next
// request starts it again.

const (
	// CleanupProtocolVersion is the protocol of sayso-cleanup.
	CleanupProtocolVersion = 1
	// CleanupLoadLimit is the time a load may take (the first start on a Mac compiles Metal shaders).
	CleanupLoadLimit = 15 * time.Second
	// CleanupIdleLimit stops the process after no request for this long.
	CleanupIdleLimit  = 10 * time.Minute
	cleanupShortLimit = 5 * time.Second
)

// Cleanup error codes.
const (
	CleanupBadRequest   = "bad_request"
	CleanupNotLoaded    = "not_loaded"
	CleanupWrongModel   = "wrong_model"
	CleanupLoadFailed   = "load_failed"
	CleanupDecodeFailed = "decode_failed"
)

// Stops of a completion. Only StopEnd is a whole answer.
const (
	StopEnd     = "end"
	StopLimit   = "limit"
	StopContext = "context"
)

// CleanupError is an error reply of the runtime.
type CleanupError struct{ Code, Message string }

func (e *CleanupError) Error() string { return "cleanup runtime: " + e.Code + ": " + e.Message }

// ErrCleanupTimeout is a request that passed its deadline (the process was killed).
var ErrCleanupTimeout = errors.New("cleanup runtime: the request timed out")

// ErrCleanupDisabled is a request while AI is off or the model is blocked.
var ErrCleanupDisabled = errors.New("cleanup runtime: disabled")

// CleanupHello is the runtime's answer to hello.
type CleanupHello struct {
	Version  uint32 `json:"version"`
	LlamaCpp string `json:"llama_cpp"`
	Backend  string `json:"backend"`
}

// Completion is the reply to complete.
type Completion struct {
	Text      string `json:"text"`
	Stop      string `json:"stop"`
	TokensIn  uint32 `json:"tokens_in"`
	TokensOut uint32 `json:"tokens_out"`
	Ms        uint64 `json:"ms"`
}

type cleanupProcess struct {
	cmd     *exec.Cmd
	stdin   io.WriteCloser
	lines   chan []byte
	model   string
	hello   CleanupHello
	lastUse time.Time
}

func (p *cleanupProcess) kill() {
	if p.cmd.Process != nil {
		_ = p.cmd.Process.Kill()
	}
	p.stdin.Close()
	_ = p.cmd.Wait()
}

// CleanupClient runs sayso-cleanup on demand.
type CleanupClient struct {
	path string
	mu   sync.Mutex
	proc *cleanupProcess
	next uint64
	// enabled is false while AI is off.
	enabled bool
	blocked map[string]bool
	idle    *time.Timer
}

// CleanupProgram is the runtime's name beside the app.
func CleanupProgram() string {
	if runtime.GOOS == "windows" {
		return "sayso-cleanup.exe"
	}
	return "sayso-cleanup"
}

// DefaultCleanupPath is the runtime next to the running executable.
func DefaultCleanupPath() string {
	exe, err := os.Executable()
	if err != nil {
		return CleanupProgram()
	}
	return filepath.Join(filepath.Dir(exe), CleanupProgram())
}

// NewCleanupClient makes a client for the runtime at path ("" uses the default).
func NewCleanupClient(path string) *CleanupClient {
	if path == "" {
		path = DefaultCleanupPath()
	}
	return &CleanupClient{path: path, enabled: true, blocked: map[string]bool{}}
}

// IsRunning reports whether the process runs.
func (c *CleanupClient) IsRunning() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.proc != nil
}

// Stop kills the process (before a model file is deleted, when AI goes off).
func (c *CleanupClient) Stop() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.stopLocked()
}

func (c *CleanupClient) stopLocked() {
	if c.proc != nil {
		c.proc.kill()
		c.proc = nil
	}
	if c.idle != nil {
		c.idle.Stop()
		c.idle = nil
	}
}

// SetEnabled turns the runtime on and off; off stops it.
func (c *CleanupClient) SetEnabled(on bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.enabled = on
	if !on {
		c.stopLocked()
	}
}

// Block refuses a model (while it is deleted) and stops the runtime when it has it.
func (c *CleanupClient) Block(model string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.blocked[model] = true
	if c.proc != nil && c.proc.model == model {
		c.stopLocked()
	}
}

// Unblock allows a model again.
func (c *CleanupClient) Unblock(model string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.blocked, model)
}

func (c *CleanupClient) start() (*cleanupProcess, error) {
	cmd := exec.Command(c.path)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	cmd.Stderr = nil
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("cannot start the cleanup runtime %s: %w", c.path, err)
	}
	p := &cleanupProcess{cmd: cmd, stdin: stdin, lines: make(chan []byte, 4)}
	go func() {
		r := bufio.NewReaderSize(stdout, 1<<20)
		for {
			line, err := r.ReadBytes('\n')
			if len(line) > 0 {
				p.lines <- line
			}
			if err != nil {
				close(p.lines)
				return
			}
		}
	}()
	c.proc = p
	reply, err := c.requestLocked("hello", nil, cleanupShortLimit)
	if err != nil {
		c.stopLocked()
		return nil, err
	}
	_ = json.Unmarshal(reply, &p.hello)
	return p, nil
}

// requestLocked sends one request and waits for its reply, killing the
// process when the deadline passes. Hold c.mu.
func (c *CleanupClient) requestLocked(typ string, fields map[string]any, limit time.Duration) ([]byte, error) {
	p := c.proc
	if p == nil {
		return nil, ErrStopped
	}
	c.next++
	id := strconv.FormatUint(c.next, 10)
	m := map[string]any{"v": CleanupProtocolVersion, "id": id, "type": typ}
	for k, v := range fields {
		m[k] = v
	}
	b, _ := json.Marshal(m)
	if _, err := p.stdin.Write(append(b, '\n')); err != nil {
		c.stopLocked()
		return nil, fmt.Errorf("cleanup runtime: %w", err)
	}
	var timer <-chan time.Time
	if limit > 0 {
		t := time.NewTimer(limit)
		defer t.Stop()
		timer = t.C
	}
	for {
		select {
		case line, ok := <-p.lines:
			if !ok {
				c.stopLocked()
				return nil, errors.New("cleanup runtime: the process stopped")
			}
			var head struct {
				ID      string `json:"id"`
				Type    string `json:"type"`
				Code    string `json:"code"`
				Message string `json:"message"`
			}
			if json.Unmarshal(line, &head) != nil || head.ID != id {
				continue // not our reply
			}
			p.lastUse = time.Now()
			if head.Type == "error" {
				return nil, &CleanupError{head.Code, head.Message}
			}
			return line, nil
		case <-timer:
			c.stopLocked()
			return nil, ErrCleanupTimeout
		}
	}
}

func (c *CleanupClient) armIdle() {
	if c.idle != nil {
		c.idle.Stop()
	}
	c.idle = time.AfterFunc(CleanupIdleLimit, func() {
		c.mu.Lock()
		defer c.mu.Unlock()
		if c.proc != nil && time.Since(c.proc.lastUse) >= CleanupIdleLimit {
			c.stopLocked()
		}
	})
}

// EnsureLoaded starts the runtime when needed and loads the model file
// (the app checks its hash before). nCtx is the context size.
func (c *CleanupClient) EnsureLoaded(model, path string, nCtx uint32) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.ensureLoadedLocked(model, path, nCtx)
}

func (c *CleanupClient) ensureLoadedLocked(model, path string, nCtx uint32) error {
	if !c.enabled || c.blocked[model] {
		return ErrCleanupDisabled
	}
	if c.proc == nil {
		if _, err := c.start(); err != nil {
			return err
		}
	}
	defer c.armIdle()
	if c.proc.model == model {
		return nil
	}
	if _, err := c.requestLocked("load", map[string]any{"model_id": model, "path": path, "n_ctx": nCtx}, CleanupLoadLimit); err != nil {
		return err
	}
	c.proc.model = model
	return nil
}

// CleanupSession runs requests on one loaded model. It is valid only inside With.
type CleanupSession struct {
	c           *CleanupClient
	model, path string
	nCtx        uint32
}

// With loads the model and runs f with a session, holding the runtime for f.
func (c *CleanupClient) With(model, path string, nCtx uint32, f func(*CleanupSession) error) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := c.ensureLoadedLocked(model, path, nCtx); err != nil {
		return err
	}
	defer c.armIdle()
	return f(&CleanupSession{c: c, model: model, path: path, nCtx: nCtx})
}

// retry sends a request; a wrong_model or not_loaded reply (a restart after a
// timeout) loads the model again and sends it once more.
func (s *CleanupSession) retry(typ string, fields map[string]any, limit time.Duration) ([]byte, error) {
	reply, err := s.c.requestLocked(typ, fields, limit)
	var ce *CleanupError
	if errors.Is(err, ErrStopped) || (errors.As(err, &ce) && (ce.Code == CleanupWrongModel || ce.Code == CleanupNotLoaded)) {
		if s.c.proc != nil {
			s.c.proc.model = ""
		}
		if err := s.c.ensureLoadedLocked(s.model, s.path, s.nCtx); err != nil {
			return nil, err
		}
		return s.c.requestLocked(typ, fields, limit)
	}
	return reply, err
}

// Count counts the tokens of text. limit 0 means no deadline.
func (s *CleanupSession) Count(text string, limit time.Duration) (uint32, error) {
	reply, err := s.retry("count", map[string]any{"model_id": s.model, "text": text}, limit)
	if err != nil {
		return 0, err
	}
	var r struct {
		Tokens uint32 `json:"tokens"`
	}
	err = json.Unmarshal(reply, &r)
	return r.Tokens, err
}

// Complete runs greedy decoding until an end token, an `end` string, or maxTokens.
func (s *CleanupSession) Complete(prompt string, end []string, maxTokens uint32, limit time.Duration) (Completion, error) {
	if end == nil {
		end = []string{}
	}
	reply, err := s.retry("complete", map[string]any{"model_id": s.model, "prompt": prompt, "end": end, "max_tokens": maxTokens}, limit)
	if err != nil {
		return Completion{}, err
	}
	var r Completion
	err = json.Unmarshal(reply, &r)
	return r, err
}

// Hello is the hello of the running process, if any.
func (c *CleanupClient) Hello() (CleanupHello, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.proc == nil {
		return CleanupHello{}, false
	}
	return c.proc.hello, true
}
