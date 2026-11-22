package engine

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
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
	// CleanupProtocolVersion is the protocol of sayso-cleanup.
	CleanupProtocolVersion = 1
	// CleanupLoadLimit is the time a load may take (the first start on a Mac compiles Metal shaders).
	CleanupLoadLimit = 15 * time.Second
	// CleanupIdleLimit stops the process after no request for this long.
	CleanupIdleLimit  = 10 * time.Minute
	cleanupShortLimit = 5 * time.Second
	// cleanupExitLimit is the time a graceful stop waits for the exit before
	// it kills the process.
	cleanupExitLimit = 5 * time.Second
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

// ErrCleanupNotInstalled is a runtime binary that cannot start.
var ErrCleanupNotInstalled = errors.New("the cleanup runtime is not installed")

// ErrCleanupNoLibraries is a runtime that says in its hello that it has no
// llama.cpp libraries (they download to the user cache; see internal/llamalib).
var ErrCleanupNoLibraries = errors.New("the cleanup runtime has no llama.cpp libraries yet")

// ErrCleanupCancelled is a request whose context ended (the process was killed).
var ErrCleanupCancelled = errors.New("cleanup runtime: the request was cancelled")

// CleanupHello is the runtime's answer to hello.
type CleanupHello struct {
	Version  uint32 `json:"version"`
	LlamaCpp string `json:"llama_cpp"`
	Backend  string `json:"backend"`
	// Libraries is "found" or "missing": where the runtime loads llama.cpp
	// from shared libraries (cmd/sayso-cleanup). Empty for a runtime that
	// links llama.cpp (native/cleanup).
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

type cleanupProcess struct {
	cmd     *exec.Cmd
	stdin   io.WriteCloser
	lines   chan []byte
	exited  chan struct{} // closed when the process has exited
	stderr  *tailBuffer
	model   string
	hello   CleanupHello
	lastUse time.Time
}

func (p *cleanupProcess) kill() {
	if p.cmd.Process != nil {
		_ = p.cmd.Process.Kill()
	}
	p.stdin.Close()
	<-p.exited
}

// stop closes stdin, so the runtime frees its model and the backend and
// exits, and kills it when it has not exited within limit.
func (p *cleanupProcess) stop(limit time.Duration) {
	p.stdin.Close()
	select {
	case <-p.exited:
	case <-time.After(limit):
		slog.Warn("the cleanup runtime did not exit; killing it", "pid", p.cmd.Process.Pid)
		p.kill()
	}
}

// died is the error of a process that stopped on its own, with the end of
// its stderr (for example why libffi or llama.cpp did not load).
func (p *cleanupProcess) died() error {
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
		slog.Debug("cleanup runtime: " + string(t.line[:i]))
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
	// idleLimit is CleanupIdleLimit; tests make it short.
	idleLimit time.Duration
}

// CleanupProgram is the runtime's name beside the app.
func CleanupProgram() string {
	if runtime.GOOS == "windows" {
		return "sayso-cleanup.exe"
	}
	return "sayso-cleanup"
}

// CleanupPathFor is where a package puts the runtime of the app at exe. On
// macOS it is next to the app in Contents/MacOS, and its libraries are in
// Contents/Frameworks/llama. Elsewhere it is in llama/ next to the app, with
// its libraries, so that their ggml never meets the ggml of the speech
// engine, which loads libraries from the folder of the app.
func CleanupPathFor(exe, goos string) string {
	program := "sayso-cleanup"
	if goos == "windows" {
		program += ".exe"
	}
	if goos == "darwin" {
		return filepath.Join(filepath.Dir(exe), program)
	}
	return filepath.Join(filepath.Dir(exe), "llama", program)
}

// DefaultCleanupPath is SAYSO_CLEANUP_PATH, else the runtime of the package
// of the running executable.
func DefaultCleanupPath() string {
	if p := os.Getenv("SAYSO_CLEANUP_PATH"); p != "" {
		return p
	}
	exe, err := os.Executable()
	if err != nil {
		return CleanupProgram()
	}
	if real, err := filepath.EvalSymlinks(exe); err == nil {
		exe = real
	}
	return CleanupPathFor(exe, runtime.GOOS)
}

// NewCleanupClient makes a client for the runtime at path ("" uses the default).
func NewCleanupClient(path string) *CleanupClient {
	if path == "" {
		path = DefaultCleanupPath()
	}
	// The process starts in the folder of the program, so a relative path
	// would be read from there.
	if strings.ContainsRune(path, filepath.Separator) || strings.ContainsRune(path, '/') {
		if abs, err := filepath.Abs(path); err == nil {
			path = abs
		}
	}
	return &CleanupClient{path: path, enabled: true, blocked: map[string]bool{}, idleLimit: CleanupIdleLimit}
}

// IsRunning reports whether the process runs.
func (c *CleanupClient) IsRunning() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.proc != nil
}

// Stop ends the process gracefully: the runtime frees the context, the model,
// and the backend, and exits (before a model file is deleted, when AI goes
// off, and when the app quits). It waits for a request that runs. A second
// call does nothing.
func (c *CleanupClient) Stop() {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.stopLocked()
}

func (c *CleanupClient) stopLocked() {
	if c.proc != nil {
		c.proc.stop(cleanupExitLimit)
		c.proc = nil
	}
	c.disarmIdle()
}

// killLocked kills the process at once: a request passed its deadline.
func (c *CleanupClient) killLocked() {
	if c.proc != nil {
		c.proc.kill()
		c.proc = nil
	}
	c.disarmIdle()
}

func (c *CleanupClient) disarmIdle() {
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

// start starts the process and waits for its hello; it kills the process when
// ctx ends first.
func (c *CleanupClient) start(ctx context.Context) (*cleanupProcess, error) {
	cmd := exec.Command(c.path)
	// Not the folder of the app: llama.cpp looks for ggml backends in the
	// working folder too.
	cmd.Dir = filepath.Dir(c.path)
	hideConsoleWindow(cmd)
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
		return nil, fmt.Errorf("%w: %s: %v", ErrCleanupNotInstalled, c.path, err)
	}
	p := &cleanupProcess{cmd: cmd, stdin: stdin, lines: make(chan []byte, 4), exited: make(chan struct{}), stderr: tail}
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
	reply, err := c.requestLocked(ctx, "hello", nil, cleanupShortLimit)
	if err != nil {
		c.killLocked()
		return nil, err
	}
	_ = json.Unmarshal(reply, &p.hello)
	return p, nil
}

// requestLocked sends one request and waits for its reply, killing the
// process when the deadline passes or ctx ends. Hold c.mu.
func (c *CleanupClient) requestLocked(ctx context.Context, typ string, fields map[string]any, limit time.Duration) ([]byte, error) {
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
				return nil, &CleanupError{head.Code, head.Message}
			}
			return line, nil
		case <-timer:
			c.killLocked()
			return nil, ErrCleanupTimeout
		case <-ctx.Done():
			c.killLocked()
			return nil, fmt.Errorf("%w: %v", ErrCleanupCancelled, ctx.Err())
		}
	}
}

func (c *CleanupClient) armIdle() {
	if c.idle != nil {
		c.idle.Stop()
	}
	limit := c.idleLimit
	c.idle = time.AfterFunc(limit, func() {
		c.mu.Lock()
		defer c.mu.Unlock()
		if c.proc != nil && time.Since(c.proc.lastUse) >= limit {
			slog.Info("cleanup runtime idle; stopping it", "after", limit)
			c.stopLocked()
		}
	})
}

// EnsureLoaded starts the runtime when needed and loads the model file
// (the app checks its hash before). nCtx is the context size.
func (c *CleanupClient) EnsureLoaded(model, path string, nCtx uint32) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.ensureLoadedLocked(context.Background(), model, path, nCtx)
}

// ensureLoadedLocked starts the process and loads the model; when ctx ends
// first, it kills the process (a cold load can take seconds).
func (c *CleanupClient) ensureLoadedLocked(ctx context.Context, model, path string, nCtx uint32) error {
	if !c.enabled || c.blocked[model] {
		return ErrCleanupDisabled
	}
	if c.proc == nil {
		p, err := c.start(ctx)
		if err != nil {
			return err
		}
		if p.hello.Libraries == "missing" {
			c.stopLocked()
			return ErrCleanupNoLibraries
		}
	}
	defer c.armIdle()
	if c.proc.model == model {
		return nil
	}
	if _, err := c.requestLocked(ctx, "load", map[string]any{"model_id": model, "path": path, "n_ctx": nCtx}, CleanupLoadLimit); err != nil {
		return err
	}
	c.proc.model = model
	return nil
}

// CleanupSession runs requests on one loaded model. It is valid only inside With.
type CleanupSession struct {
	c           *CleanupClient
	ctx         context.Context
	model, path string
	nCtx        uint32
}

// With loads the model and runs f with a session, holding the runtime for f.
func (c *CleanupClient) With(model, path string, nCtx uint32, f func(*CleanupSession) error) error {
	return c.WithContext(context.Background(), model, path, nCtx, f)
}

// WithContext is With, and the requests of the session end (the process is
// killed) when ctx ends.
func (c *CleanupClient) WithContext(ctx context.Context, model, path string, nCtx uint32, f func(*CleanupSession) error) error {
	if ctx == nil {
		ctx = context.Background()
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return fmt.Errorf("%w: %v", ErrCleanupCancelled, err)
	}
	if err := c.ensureLoadedLocked(ctx, model, path, nCtx); err != nil {
		return err
	}
	defer c.armIdle()
	return f(&CleanupSession{c: c, ctx: ctx, model: model, path: path, nCtx: nCtx})
}

// retry sends a request; a wrong_model or not_loaded reply (a restart after a
// timeout) loads the model again and sends it once more.
func (s *CleanupSession) retry(typ string, fields map[string]any, limit time.Duration) ([]byte, error) {
	reply, err := s.c.requestLocked(s.ctx, typ, fields, limit)
	var ce *CleanupError
	if errors.Is(err, ErrStopped) || (errors.As(err, &ce) && (ce.Code == CleanupWrongModel || ce.Code == CleanupNotLoaded)) {
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
