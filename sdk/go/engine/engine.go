// Package engine is a client of the Sayso engine protocol v1
// (docs/engine-protocol.md): it starts an engine program and talks NDJSON
// over its stdin and stdout.
package engine

import (
	"bufio"
	"bytes"
	"context"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/watzon/sayso/sdk/go/protocol"
)

// ProtocolVersion is the engine protocol of this client.
const ProtocolVersion = 1

// KnownFeatures are the engine features that this version knows.
var KnownFeatures = protocol.EngineFeatures

// Options say how to start an engine.
type Options struct {
	// Program is the engine program (see Find).
	Program string
	Args    []string
	// ModelsDir is SAYSO_MODELS_DIR. It is required.
	ModelsDir string
	// ModelStore is SAYSO_MODEL_STORE; "" is <ModelsDir>/v1. An engine without
	// the feature store_v1 ignores it.
	ModelStore string
	// Env is more environment ("K=V") on top of the environment of this
	// process.
	Env []string
	// Stderr gets a copy of what the engine writes to stderr.
	Stderr io.Writer
	// HelloTimeout is how long the engine may take to answer hello; 0 is 30 s.
	HelloTimeout time.Duration
}

// Hello is the answer of the engine to hello.
type Hello struct {
	Version  string   `json:"version"`
	Protocol int      `json:"protocol"`
	Features []string `json:"features"`
}

// Has is true when the engine lists the feature.
func (h Hello) Has(feature string) bool { return slices.Contains(h.Features, feature) }

// Message is one line of the engine.
type Message struct {
	Type string
	ID   string
	Raw  map[string]json.RawMessage
}

// Field decodes one field into v; false when it is missing or does not fit.
func (m Message) Field(name string, v any) bool {
	raw, ok := m.Raw[name]
	return ok && json.Unmarshal(raw, v) == nil
}

// Decode decodes the whole message into v.
func (m Message) Decode(v any) error {
	b, _ := json.Marshal(m.Raw)
	return json.Unmarshal(b, v)
}

// String is the text of a string field, or "".
func (m Message) String(name string) string {
	var s string
	m.Field(name, &s)
	return s
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

// StartError is an engine that did not start or did not answer hello. Stderr
// has the last lines that it wrote there.
type StartError struct {
	Program string
	Err     error
	Stderr  []string
}

func (e *StartError) Error() string {
	msg := fmt.Sprintf("the engine %s did not start: %v", e.Program, e.Err)
	if len(e.Stderr) > 0 {
		msg += "\nits last lines on stderr:\n  " + strings.Join(e.Stderr, "\n  ")
	}
	return msg
}

func (e *StartError) Unwrap() error { return e.Err }

// ErrStopped is the error of a request when the engine is not running.
var ErrStopped = errors.New("engine: the engine is not running")

const stderrLines = 20

type pending struct {
	reply   chan Message
	onEvent func(Message)
}

// Engine is a running engine.
type Engine struct {
	program string
	cmd     *exec.Cmd
	stdin   io.WriteCloser
	writeMu sync.Mutex
	hello   Hello

	mu      sync.Mutex
	pending map[string]*pending
	next    uint64
	stderr  []string
	exitErr error

	events chan Message
	queue  *eventQueue
	done   chan struct{}
}

// Start starts the engine and waits for its hello.
func Start(ctx context.Context, opts Options) (*Engine, error) {
	if opts.Program == "" {
		return nil, errors.New("engine: no program")
	}
	if opts.ModelsDir == "" {
		return nil, errors.New("engine: no models folder")
	}
	if err := os.MkdirAll(opts.ModelsDir, 0o755); err != nil {
		return nil, err
	}
	store := opts.ModelStore
	if store == "" {
		store = protocol.ModelStoreDir(opts.ModelsDir)
	}
	cmd := exec.Command(opts.Program, opts.Args...)
	hideWindow(cmd)
	cmd.Env = append(os.Environ(), protocol.ModelsDirEnv+"="+opts.ModelsDir, protocol.ModelStoreEnv+"="+store)
	cmd.Env = append(cmd.Env, opts.Env...)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, &StartError{Program: opts.Program, Err: err}
	}
	e := &Engine{program: opts.Program, cmd: cmd, stdin: stdin, pending: map[string]*pending{}, events: make(chan Message), done: make(chan struct{})}
	e.queue = newEventQueue(e.events, e.done)
	var readers sync.WaitGroup
	readers.Add(2)
	go func() { defer readers.Done(); e.readStderr(stderr, opts.Stderr) }()
	go func() { defer readers.Done(); e.readStdout(stdout) }()
	go func() {
		readers.Wait()
		err := cmd.Wait()
		e.mu.Lock()
		e.exitErr = err
		for id, p := range e.pending {
			close(p.reply)
			delete(e.pending, id)
		}
		e.mu.Unlock()
		e.queue.close()
		close(e.done)
	}()
	timeout := opts.HelloTimeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}
	hctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	m, err := e.Call(hctx, "hello", nil, nil)
	if err == nil {
		err = m.Decode(&e.hello)
		if err == nil && e.hello.Protocol != ProtocolVersion {
			err = fmt.Errorf("the engine speaks protocol %d, this client %d", e.hello.Protocol, ProtocolVersion)
		}
	}
	if err != nil {
		e.Kill()
		<-e.done
		return nil, &StartError{Program: opts.Program, Err: e.why(err), Stderr: e.StderrTail()}
	}
	return e, nil
}

func (e *Engine) why(err error) error {
	if errors.Is(err, ErrStopped) {
		e.mu.Lock()
		defer e.mu.Unlock()
		if e.exitErr != nil {
			return fmt.Errorf("it exited: %v", e.exitErr)
		}
		return errors.New("it exited")
	}
	return err
}

func (e *Engine) readStderr(r io.Reader, copyTo io.Writer) {
	s := bufio.NewScanner(r)
	s.Buffer(make([]byte, 64*1024), 1<<20)
	for s.Scan() {
		line := s.Text()
		if copyTo != nil {
			fmt.Fprintln(copyTo, line)
		}
		e.mu.Lock()
		e.stderr = append(e.stderr, line)
		if len(e.stderr) > stderrLines {
			e.stderr = e.stderr[len(e.stderr)-stderrLines:]
		}
		e.mu.Unlock()
	}
}

func (e *Engine) readStdout(r io.Reader) {
	br := bufio.NewReaderSize(r, 1<<20)
	for {
		line, err := br.ReadBytes('\n')
		if line = bytes.TrimSpace(line); len(line) > 0 {
			e.handleLine(line)
		}
		if err != nil {
			return
		}
	}
}

func parse(line []byte) (Message, bool) {
	var m Message
	if json.Unmarshal(line, &m.Raw) != nil {
		return m, false
	}
	m.Type, m.ID = m.String("type"), m.String("id")
	return m, m.Type != ""
}

func isReply(kind string) bool {
	return kind == "hello" || kind == "ok" || kind == "final" || kind == "error"
}

func (e *Engine) handleLine(line []byte) {
	m, ok := parse(line)
	if !ok {
		return
	}
	if m.ID != "" {
		e.mu.Lock()
		p := e.pending[m.ID]
		if p != nil && isReply(m.Type) {
			delete(e.pending, m.ID)
		}
		e.mu.Unlock()
		if p != nil && isReply(m.Type) {
			p.reply <- m
			return
		}
		if p != nil && p.onEvent != nil {
			p.onEvent(m)
		}
	}
	if !isReply(m.Type) || m.ID == "" {
		e.queue.push(m)
	}
}

// Hello is the hello of the engine.
func (e *Engine) Hello() Hello { return e.hello }

// Has is true when the engine lists the feature.
func (e *Engine) Has(feature string) bool { return e.hello.Has(feature) }

// Events are the events of the engine (and errors without an id), in order.
// It closes when the engine exits; events that nobody read by then are
// dropped.
func (e *Engine) Events() <-chan Message { return e.events }

// Done closes when the engine exits.
func (e *Engine) Done() <-chan struct{} { return e.done }

// StderrTail is the last lines of stderr.
func (e *Engine) StderrTail() []string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return slices.Clone(e.stderr)
}

func (e *Engine) write(fields map[string]any) error {
	b, err := json.Marshal(fields)
	if err != nil {
		return err
	}
	e.writeMu.Lock()
	defer e.writeMu.Unlock()
	select {
	case <-e.done:
		return ErrStopped
	default:
	}
	if _, err := e.stdin.Write(append(b, '\n')); err != nil {
		return ErrStopped
	}
	return nil
}

// Call sends a request and waits for its reply. onEvent gets the events with
// the id of the request, before the reply. An error reply is an *Error.
func (e *Engine) Call(ctx context.Context, kind string, fields map[string]any, onEvent func(Message)) (Message, error) {
	e.mu.Lock()
	e.next++
	id := "r" + strconv.FormatUint(e.next, 10)
	p := &pending{reply: make(chan Message, 1), onEvent: onEvent}
	e.pending[id] = p
	e.mu.Unlock()
	msg := map[string]any{"v": 1, "type": kind, "id": id}
	for k, v := range fields {
		msg[k] = v
	}
	if err := e.write(msg); err != nil {
		e.forget(id)
		return Message{}, err
	}
	select {
	case m, ok := <-p.reply:
		if !ok {
			return Message{}, ErrStopped
		}
		if m.Type == "error" {
			return m, &Error{Message: m.String("message"), Code: m.String("code"), Reason: m.String("reason")}
		}
		return m, nil
	case <-ctx.Done():
		e.forget(id)
		return Message{}, ctx.Err()
	}
}

func (e *Engine) forget(id string) {
	e.mu.Lock()
	delete(e.pending, id)
	e.mu.Unlock()
}

// Close closes stdin and waits up to 5 s for the engine to exit, then kills
// it.
func (e *Engine) Close() error {
	e.writeMu.Lock()
	e.stdin.Close()
	e.writeMu.Unlock()
	select {
	case <-e.done:
	case <-time.After(5 * time.Second):
		e.Kill()
		<-e.done
	}
	return nil
}

// Kill stops the engine at once.
func (e *Engine) Kill() {
	if e.cmd.Process != nil {
		e.cmd.Process.Kill()
	}
}

// PCM is base64 of little-endian 16-bit samples, the pcm of stream_audio.
func PCM(samples []int16) string {
	b := make([]byte, 2*len(samples))
	for i, s := range samples {
		binary.LittleEndian.PutUint16(b[2*i:], uint16(s))
	}
	return base64.StdEncoding.EncodeToString(b)
}

// eventQueue passes events on in order without ever blocking the reader.
type eventQueue struct {
	mu     sync.Mutex
	cond   *sync.Cond
	items  []Message
	closed bool
}

func newEventQueue(out chan<- Message, done <-chan struct{}) *eventQueue {
	q := &eventQueue{}
	q.cond = sync.NewCond(&q.mu)
	go func() {
		defer close(out)
		for {
			q.mu.Lock()
			for len(q.items) == 0 && !q.closed {
				q.cond.Wait()
			}
			if len(q.items) == 0 {
				q.mu.Unlock()
				return
			}
			m := q.items[0]
			q.items = q.items[1:]
			q.mu.Unlock()
			select {
			case out <- m:
			case <-done:
				return
			}
		}
	}()
	return q
}

func (q *eventQueue) push(m Message) {
	q.mu.Lock()
	q.items = append(q.items, m)
	q.mu.Unlock()
	q.cond.Signal()
}

func (q *eventQueue) close() {
	q.mu.Lock()
	q.closed = true
	q.mu.Unlock()
	q.cond.Signal()
}
