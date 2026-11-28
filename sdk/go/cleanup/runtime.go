package cleanup

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

// The client of the cleanup runtime, sayso-cleanup (cmd/sayso-cleanup;
// docs/cleanup-models.md, "Runtime and protocol"). NDJSON on stdin/stdout, one
// request at a time. llama.cpp cannot stop a decode cheaply, so a request past
// its deadline, or whose context ends, kills the process; the next request
// starts it again. Every other stop is graceful: the client closes stdin, and
// the runtime frees the context, the model, and the llama.cpp backend before it
// exits.

const (
	// ProtocolVersion is the protocol of sayso-cleanup.
	ProtocolVersion = 1
	// LoadLimit is the time a load may take (the first start on a Mac compiles Metal shaders).
	LoadLimit = 15 * time.Second
	// IdleLimit stops the process after no request for this long.
	IdleLimit  = 10 * time.Minute
	shortLimit = 5 * time.Second
	// exitLimit is the time a graceful stop waits for the exit before
	// it kills the process.
	exitLimit = 5 * time.Second
)

// Cleanup error codes.
const (
	CodeBadRequest   = "bad_request"
	CodeNotLoaded    = "not_loaded"
	CodeWrongModel   = "wrong_model"
	CodeLoadFailed   = "load_failed"
	CodeDecodeFailed = "decode_failed"
)

// Stops of a completion. Only StopEnd is a whole answer.
const (
	StopEnd     = "end"
	StopLimit   = "limit"
	StopContext = "context"
)

// RuntimeError is an error reply of the runtime.
type RuntimeError struct{ Code, Message string }

func (e *RuntimeError) Error() string { return "cleanup runtime: " + e.Code + ": " + e.Message }

// ErrTimeout is a request that passed its deadline (the process was killed).
var ErrTimeout = errors.New("cleanup runtime: the request timed out")

// ErrDisabled is a request while AI is off or the model is blocked.
var ErrDisabled = errors.New("cleanup runtime: disabled")

// ErrNotInstalled is a runtime binary that cannot start.
var ErrNotInstalled = errors.New("the cleanup runtime is not installed")

// ErrNoLibraries is a runtime that says in its hello that it has no
// llama.cpp libraries (they download to the user cache; see internal/llamalib).
var ErrNoLibraries = errors.New("the cleanup runtime has no llama.cpp libraries yet")

// ErrCancelled is a request whose context ended (the process was killed).
var ErrCancelled = errors.New("cleanup runtime: the request was cancelled")

// Hello is the runtime's answer to hello.
type Hello struct {
	Version  uint32 `json:"version"`
	LlamaCpp string `json:"llama_cpp"`
	Backend  string `json:"backend"`
	// Libraries is "found" or "missing": where the runtime loads llama.cpp
	// from shared libraries (cmd/sayso-cleanup). Empty for a runtime that
	// links llama.cpp statically.
	Libraries string `json:"libraries,omitempty"`
}

// Completion is the reply to complete.
type Completion struct {
	Text      string `json:"text"`
	Stop      string `json:"stop"`
	TokensIn  uint32 `json:"tokens_in"`
	TokensOut uint32 `json:"tokens_out"`
	Ms        uint64 `json:"ms"`
}

type process struct {
	cmd     *exec.Cmd
	stdin   io.WriteCloser
	lines   chan []byte
	exited  chan struct{} // closed when the process has exited
	stderr  *tailBuffer
	model   string
	hello   Hello
	lastUse time.Time
}

func (p *process) kill() {
	if p.cmd.Process != nil {
		_ = p.cmd.Process.Kill()
	}
	p.stdin.Close()
	<-p.exited
}

// stop closes stdin, so the runtime frees its model and the backend and
// exits, and kills it when it has not exited within limit.
func (p *process) stop(limit time.Duration) {
	p.stdin.Close()
	select {
	case <-p.exited:
	case <-time.After(limit):
		Log.Warn("the cleanup runtime did not exit; killing it", "pid", p.cmd.Process.Pid)
		p.kill()
	}
}

// died is the error of a process that stopped on its own, with the end of
// its stderr (for example why libffi or llama.cpp did not load).
func (p *process) died() error {
	<-p.exited
	msg := "cleanup runtime: the process stopped"
	if p.cmd.ProcessState != nil {
		msg += " (" + p.cmd.ProcessState.String() + ")"
	}
	if tail := strings.TrimSpace(p.stderr.String()); tail != "" {
		msg += ": " + tail
	}
	return errors.New(msg)
}

// tailBuffer keeps the last bytes written to it, and logs each line at the
// debug level.
type tailBuffer struct {
	mu   sync.Mutex
	buf  []byte
	line []byte
}

const tailBytes = 2048

func (t *tailBuffer) Write(b []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.buf = append(t.buf, b...)
	if len(t.buf) > tailBytes {
		t.buf = t.buf[len(t.buf)-tailBytes:]
	}
	t.line = append(t.line, b...)
	for {
		i := strings.IndexByte(string(t.line), '\n')
		if i < 0 {
			break
		}
		Log.Debug("cleanup runtime: " + string(t.line[:i]))
		t.line = t.line[i+1:]
	}
	if len(t.line) > tailBytes {
		t.line = t.line[:0]
	}
	return len(b), nil
}

func (t *tailBuffer) String() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return string(t.buf)
}

// Client runs sayso-cleanup on demand.
type Client struct {
	path string
	mu   sync.Mutex
	proc *process
	next uint64
	// enabled is false while AI is off.
	enabled bool
	blocked map[string]bool
	idle    *time.Timer
	// idleLimit is IdleLimit; tests make it short.
	idleLimit time.Duration
}

// Program is the runtime's name beside the app.
func Program() string {
	if runtime.GOOS == "windows" {
		return "sayso-cleanup.exe"
	}
	return "sayso-cleanup"
}

// PathFor is where a package puts the runtime of the app at exe. On
// macOS it is next to the app in Contents/MacOS, and its libraries are in
// Contents/Frameworks/llama. Elsewhere it is in llama/ next to the app, with
// its libraries, so that their ggml never meets the ggml of the speech
// engine, which loads libraries from the folder of the app.
func PathFor(exe, goos string) string {
	program := "sayso-cleanup"
	if goos == "windows" {
		program += ".exe"
	}
	if goos == "darwin" {
		return filepath.Join(filepath.Dir(exe), program)
	}
	return filepath.Join(filepath.Dir(exe), "llama", program)
}

// DefaultPath is SAYSO_CLEANUP_PATH, else the runtime of the package
// of the running executable.
func DefaultPath() string {
	if p := os.Getenv("SAYSO_CLEANUP_PATH"); p != "" {
		return p
	}
	exe, err := os.Executable()
	if err != nil {
		return Program()
	}
	if real, err := filepath.EvalSymlinks(exe); err == nil {
		exe = real
	}
	return PathFor(exe, runtime.GOOS)
}

// NewClient makes a client for the runtime at path ("" uses the default).
func NewClient(path string) *Client {
	if path == "" {
		path = DefaultPath()
	}
	// The process starts in the folder of the program, so a relative path
	// would be read from there.
	if strings.ContainsRune(path, filepath.Separator) || strings.ContainsRune(path, '/') {
		if abs, err := filepath.Abs(path); err == nil {
			path = abs
		}
	}
	return &Client{path: path, enabled: true, blocked: map[string]bool{}, idleLimit: IdleLimit}
}

// IsRunning reports whether the process runs.
func (c *Client) IsRunning() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.proc != nil
}

// Stop ends the process gracefully: the runtime frees the context, the model,
// and the backend, and exits (before a model file is deleted, when AI goes
// off, and when the app quits). It waits for a request that runs. A second
// call does nothing.
func (c *Client) Stop() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.stopLocked()
}

func (c *Client) stopLocked() {
	if c.proc != nil {
		c.proc.stop(exitLimit)
		c.proc = nil
	}
	c.disarmIdle()
}

// killLocked kills the process at once: a request passed its deadline.
func (c *Client) killLocked() {
	if c.proc != nil {
		c.proc.kill()
		c.proc = nil
	}
	c.disarmIdle()
}

func (c *Client) disarmIdle() {
	if c.idle != nil {
		c.idle.Stop()
		c.idle = nil
	}
}

// SetEnabled turns the runtime on and off; off stops it.
func (c *Client) SetEnabled(on bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.enabled = on
	if !on {
		c.stopLocked()
	}
}

// Block refuses a model (while it is deleted) and stops the runtime when it has it.
func (c *Client) Block(model string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.blocked[model] = true
	if c.proc != nil && c.proc.model == model {
		c.stopLocked()
	}
}

// Unblock allows a model again.
func (c *Client) Unblock(model string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.blocked, model)
}

// start starts the process and waits for its hello; it kills the process when
// ctx ends first.
func (c *Client) start(ctx context.Context) (*process, error) {
	cmd := exec.Command(c.path)
	// Not the folder of the app: llama.cpp looks for ggml backends in the
	// working folder too.
	cmd.Dir = filepath.Dir(c.path)
	hideWindow(cmd)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	tail := &tailBuffer{}
	cmd.Stderr = tail
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("%w: %s: %v", ErrNotInstalled, c.path, err)
	}
	p := &process{cmd: cmd, stdin: stdin, lines: make(chan []byte, 4), exited: make(chan struct{}), stderr: tail}
	go func() {
		_ = cmd.Wait()
		close(p.exited)
	}()
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
	reply, err := c.requestLocked(ctx, "hello", nil, shortLimit)
	if err != nil {
		c.killLocked()
		return nil, err
	}
	_ = json.Unmarshal(reply, &p.hello)
	return p, nil
}

// requestLocked sends one request and waits for its reply, killing the
// process when the deadline passes or ctx ends. Hold c.mu.
func (c *Client) requestLocked(ctx context.Context, typ string, fields map[string]any, limit time.Duration) ([]byte, error) {
	p := c.proc
	if p == nil {
		return nil, ErrStopped
	}
	c.next++
	id := strconv.FormatUint(c.next, 10)
	m := map[string]any{"v": ProtocolVersion, "id": id, "type": typ}
	for k, v := range fields {
		m[k] = v
	}
	b, _ := json.Marshal(m)
	if _, err := p.stdin.Write(append(b, '\n')); err != nil {
		c.killLocked()
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
				err := p.died()
				c.killLocked()
				return nil, err
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
				return nil, &RuntimeError{head.Code, head.Message}
			}
			return line, nil
		case <-timer:
			c.killLocked()
			return nil, ErrTimeout
		case <-ctx.Done():
			c.killLocked()
			return nil, fmt.Errorf("%w: %v", ErrCancelled, ctx.Err())
		}
	}
}

func (c *Client) armIdle() {
	if c.idle != nil {
		c.idle.Stop()
	}
	limit := c.idleLimit
	c.idle = time.AfterFunc(limit, func() {
		c.mu.Lock()
		defer c.mu.Unlock()
		if c.proc != nil && time.Since(c.proc.lastUse) >= limit {
			Log.Info("cleanup runtime idle; stopping it", "after", limit)
			c.stopLocked()
		}
	})
}

// EnsureLoaded starts the runtime when needed and loads the model file
// (the app checks its hash before). nCtx is the context size.
func (c *Client) EnsureLoaded(model, path string, nCtx uint32) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.ensureLoadedLocked(context.Background(), model, path, nCtx)
}

// ensureLoadedLocked starts the process and loads the model; when ctx ends
// first, it kills the process (a cold load can take seconds).
func (c *Client) ensureLoadedLocked(ctx context.Context, model, path string, nCtx uint32) error {
	if !c.enabled || c.blocked[model] {
		return ErrDisabled
	}
	if c.proc == nil {
		p, err := c.start(ctx)
		if err != nil {
			return err
		}
		if p.hello.Libraries == "missing" {
			c.stopLocked()
			return ErrNoLibraries
		}
	}
	defer c.armIdle()
	if c.proc.model == model {
		return nil
	}
	if _, err := c.requestLocked(ctx, "load", map[string]any{"model_id": model, "path": path, "n_ctx": nCtx}, LoadLimit); err != nil {
		return err
	}
	c.proc.model = model
	return nil
}

// Session runs requests on one loaded model. It is valid only inside With.
type Session struct {
	c           *Client
	ctx         context.Context
	model, path string
	nCtx        uint32
}

// With loads the model and runs f with a session, holding the runtime for f.
func (c *Client) With(model, path string, nCtx uint32, f func(*Session) error) error {
	return c.WithContext(context.Background(), model, path, nCtx, f)
}

// WithContext is With, and the requests of the session end (the process is
// killed) when ctx ends.
func (c *Client) WithContext(ctx context.Context, model, path string, nCtx uint32, f func(*Session) error) error {
	if ctx == nil {
		ctx = context.Background()
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return fmt.Errorf("%w: %v", ErrCancelled, err)
	}
	if err := c.ensureLoadedLocked(ctx, model, path, nCtx); err != nil {
		return err
	}
	defer c.armIdle()
	return f(&Session{c: c, ctx: ctx, model: model, path: path, nCtx: nCtx})
}

// retry sends a request; a wrong_model or not_loaded reply (a restart after a
// timeout) loads the model again and sends it once more.
func (s *Session) retry(typ string, fields map[string]any, limit time.Duration) ([]byte, error) {
	reply, err := s.c.requestLocked(s.ctx, typ, fields, limit)
	var ce *RuntimeError
	if errors.Is(err, ErrStopped) || (errors.As(err, &ce) && (ce.Code == CodeWrongModel || ce.Code == CodeNotLoaded)) {
		if s.c.proc != nil {
			s.c.proc.model = ""
		}
		if err := s.c.ensureLoadedLocked(s.ctx, s.model, s.path, s.nCtx); err != nil {
			return nil, err
		}
		return s.c.requestLocked(s.ctx, typ, fields, limit)
	}
	return reply, err
}

// Count counts the tokens of text. limit 0 means no deadline.
func (s *Session) Count(text string, limit time.Duration) (uint32, error) {
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
func (s *Session) Complete(prompt string, end []string, maxTokens uint32, limit time.Duration) (Completion, error) {
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
func (c *Client) Hello() (Hello, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.proc == nil {
		return Hello{}, false
	}
	return c.proc.hello, true
}

// Unload frees the model of the runtime and keeps the runtime running.
func (c *Client) Unload() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.proc == nil || c.proc.model == "" {
		return nil
	}
	if _, err := c.requestLocked(context.Background(), "unload", nil, shortLimit); err != nil {
		return err
	}
	c.proc.model = ""
	return nil
}
