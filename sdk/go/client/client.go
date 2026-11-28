// Package client is the raw client of the Sayso Host API v1
// (sdk/protocol/host-api.md): one connection to Sayso over its socket or
// named pipe, with a method for each request and the events on a channel.
//
// Errors are *protocol.Error. A connection that ends fails its open requests
// with host_closing (after the event host_closing) or connection_lost, and
// gives each job and live session that it started one end: job_failed or
// session_ended with that code.
package client

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"strconv"
	"sync"

	"github.com/watzon/sayso/sdk/go/protocol"
)

// MaxLine is the longest line that the client reads.
const MaxLine = 16 << 20

// UnknownEvent is an event of a type that this version does not know.
type UnknownEvent struct {
	Type   string
	Fields map[string]json.RawMessage
}

func (u UnknownEvent) Kind() string { return u.Type }

type pending struct {
	kind    string
	reply   chan any
	onEvent func(protocol.Event)
}

// Client is one connection to the host.
type Client struct {
	conn    net.Conn
	writeMu sync.Mutex

	mu       sync.Mutex
	pending  map[string]*pending
	next     uint64
	jobs     map[uint64]bool
	sessions map[uint64]bool
	end      *protocol.Error
	closing  bool
	byeSent  bool

	events *queue
	done   chan struct{}
}

// Dial connects to the host at socket; "" is protocol.ClientSocketPath. A
// host that does not answer is the error host_not_running.
func Dial(ctx context.Context, socket string) (*Client, error) {
	if socket == "" {
		p, err := protocol.ClientSocketPath()
		if err != nil {
			return nil, protocol.Errorf(protocol.CodeHostNotRunning, "cannot find the socket of Sayso: %v", err)
		}
		socket = p
	}
	conn, err := dial(ctx, socket)
	if err != nil {
		return nil, protocol.Errorf(protocol.CodeHostNotRunning, "Sayso does not run (%s): %v", socket, err)
	}
	c := &Client{conn: conn, pending: map[string]*pending{}, jobs: map[uint64]bool{}, sessions: map[uint64]bool{}, done: make(chan struct{})}
	c.events = newQueue()
	go c.readLoop()
	return c, nil
}

// Events are the events that belong to no request: of jobs, of live
// sessions, dictation_done, and host_closing. It closes after the end of
// the connection, after the ends of its jobs and sessions.
func (c *Client) Events() <-chan protocol.Event { return c.events.out }

// Done closes at the end of the connection.
func (c *Client) Done() <-chan struct{} { return c.done }

// Err is why the connection ended: host_closing, connection_lost, or nil
// after Bye or Close. It is nil while the connection is open.
func (c *Client) Err() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.end == nil {
		return nil
	}
	return c.end
}

func (c *Client) readLoop() {
	r := bufio.NewReaderSize(c.conn, 64*1024)
	var buf bytes.Buffer
	for {
		chunk, err := r.ReadSlice('\n')
		buf.Write(chunk)
		if errors.Is(err, bufio.ErrBufferFull) {
			if buf.Len() > MaxLine {
				break
			}
			continue
		}
		if line := bytes.TrimSpace(buf.Bytes()); len(line) > 0 {
			c.handleLine(string(line))
		}
		buf.Reset()
		if err != nil {
			break
		}
	}
	c.finish()
}

func (c *Client) handleLine(line string) {
	decoded, err := protocol.DecodeHost(line)
	if err != nil {
		return
	}
	switch d := decoded.(type) {
	case *protocol.Unknown:
		ev := UnknownEvent{Type: d.Kind, Fields: d.Fields}
		if d.ID != nil {
			if p := c.lookup(*d.ID); p != nil && p.onEvent != nil {
				p.onEvent(ev)
			}
			return
		}
		c.events.push(ev)
	case *protocol.Message[any]:
		switch body := d.Body.(type) {
		case protocol.OKFields, *protocol.ErrorReply:
			if d.ID == nil {
				return
			}
			c.mu.Lock()
			p := c.pending[*d.ID]
			delete(c.pending, *d.ID)
			if ok, isOK := body.(protocol.OKFields); isOK && p != nil {
				c.track(p.kind, ok)
			}
			c.mu.Unlock()
			if p != nil {
				p.reply <- body
			}
		case protocol.Event:
			c.event(d.ID, body)
		}
	}
}

// track keeps the jobs and sessions that the replies start, before their
// first event can come. c.mu is held.
func (c *Client) track(kind string, ok protocol.OKFields) {
	switch kind {
	case "transcribe_file":
		var r protocol.JobReply
		if ok.Parse(&r) == nil {
			c.jobs[r.Job] = true
		}
	case "session_start":
		var r protocol.SessionReply
		if ok.Parse(&r) == nil {
			c.sessions[r.Session] = true
		}
	}
}

func (c *Client) lookup(id string) *pending {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.pending[id]
}

func (c *Client) event(id *string, ev protocol.Event) {
	c.mu.Lock()
	switch e := ev.(type) {
	case *protocol.JobDone:
		delete(c.jobs, e.Job)
	case *protocol.JobFailed:
		delete(c.jobs, e.Job)
	case *protocol.SessionEnded:
		delete(c.sessions, e.Session)
	case *protocol.HostClosing:
		c.closing = true
	}
	c.mu.Unlock()
	if id != nil {
		if p := c.lookup(*id); p != nil {
			if p.onEvent != nil {
				p.onEvent(ev)
			}
			return
		}
	}
	c.events.push(ev)
}

// finish ends the connection once: it fails the open requests and ends the
// open jobs and sessions.
func (c *Client) finish() {
	c.conn.Close()
	c.mu.Lock()
	if c.end == nil {
		switch {
		case c.closing:
			c.end = protocol.Errorf(protocol.CodeHostClosing, "Sayso quit")
		case c.byeSent:
		default:
			c.end = protocol.Errorf(protocol.CodeConnectionLost, "the connection to Sayso ended")
		}
	}
	end := c.end
	pend := c.pending
	c.pending = map[string]*pending{}
	jobs, sessions := c.jobs, c.sessions
	c.jobs, c.sessions = map[uint64]bool{}, map[uint64]bool{}
	c.mu.Unlock()
	if end == nil {
		end = protocol.Errorf(protocol.CodeConnectionLost, "the connection is closed")
	}
	for _, p := range pend {
		p.reply <- &protocol.ErrorReply{Code: &end.Code, Message: end.Message}
	}
	code := end.Code
	for job := range jobs {
		c.events.push(&protocol.JobFailed{Job: job, Message: end.Message, Code: &code})
	}
	for session := range sessions {
		c.events.push(&protocol.SessionEnded{Session: session, Code: &code})
	}
	c.events.close()
	close(c.done)
}

func (c *Client) write(id *string, req protocol.Request) error {
	line := protocol.Encode(id, req) + "\n"
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	select {
	case <-c.done:
		return c.endError()
	default:
	}
	if _, err := io.WriteString(c.conn, line); err != nil {
		return c.endError()
	}
	return nil
}

func (c *Client) endError() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.end != nil {
		return c.end
	}
	return protocol.Errorf(protocol.CodeConnectionLost, "the connection to Sayso ended")
}

// Request sends a request and waits for its reply. onEvent gets the events
// with the id of the request. An error reply is a *protocol.Error.
func (c *Client) Request(ctx context.Context, req protocol.Request, onEvent func(protocol.Event)) (protocol.OKFields, error) {
	c.mu.Lock()
	c.next++
	id := strconv.FormatUint(c.next, 10)
	p := &pending{kind: req.Kind(), reply: make(chan any, 1), onEvent: onEvent}
	c.pending[id] = p
	c.mu.Unlock()
	if err := c.write(&id, req); err != nil {
		c.forget(id)
		return nil, err
	}
	select {
	case r := <-p.reply:
		switch r := r.(type) {
		case protocol.OKFields:
			return r, nil
		case *protocol.ErrorReply:
			return nil, r.Err()
		}
		return nil, fmt.Errorf("a reply of type %T", r)
	case <-ctx.Done():
		c.forget(id)
		return nil, ctx.Err()
	}
}

func (c *Client) forget(id string) {
	c.mu.Lock()
	delete(c.pending, id)
	c.mu.Unlock()
}

func parse[T any](fields protocol.OKFields, err error) (T, error) {
	var v T
	if err != nil {
		return v, err
	}
	if err := fields.Parse(&v); err != nil {
		return v, fmt.Errorf("a reply that does not parse: %w", err)
	}
	return v, nil
}

// Hello is the first request. onWaiting runs when the host asks the user.
func (c *Client) Hello(ctx context.Context, hello protocol.Hello, onWaiting func()) (protocol.HelloReply, error) {
	if hello.Protocol == 0 {
		hello.Protocol = protocol.Protocol
	}
	if hello.Features == nil {
		hello.Features = []string{}
	}
	return parse[protocol.HelloReply](c.Request(ctx, &hello, func(ev protocol.Event) {
		if _, ok := ev.(*protocol.ApprovalWaiting); ok && onWaiting != nil {
			onWaiting()
		}
	}))
}

// ListModels lists the local models; role nil lists all.
func (c *Client) ListModels(ctx context.Context, role *protocol.Role) ([]protocol.ModelInfo, error) {
	r, err := parse[protocol.ListModelsReply](c.Request(ctx, &protocol.ListModels{Role: role}, nil))
	return r.Models, err
}

// DownloadModel downloads a model, after the user agrees. onEvent gets its
// model_state and download_progress events.
func (c *Client) DownloadModel(ctx context.Context, model, reason string, onEvent func(protocol.Event)) error {
	_, err := c.Request(ctx, &protocol.DownloadModel{Model: model, Reason: reason}, onEvent)
	return err
}

// TranscribeFile starts a job. Its events come on Events.
func (c *Client) TranscribeFile(ctx context.Context, req protocol.TranscribeFile) (uint64, error) {
	r, err := parse[protocol.JobReply](c.Request(ctx, &req, nil))
	return r.Job, err
}

// CancelJob cancels a job of this connection.
func (c *Client) CancelJob(ctx context.Context, job uint64) error {
	_, err := c.Request(ctx, &protocol.CancelJob{Job: job}, nil)
	return err
}

// SessionStart starts a live session. Its events come on Events.
func (c *Client) SessionStart(ctx context.Context, req protocol.SessionStart) (protocol.SessionReply, error) {
	return parse[protocol.SessionReply](c.Request(ctx, &req, nil))
}

// SessionAudio sends 16 kHz mono audio of the source client. It has no
// reply; a wrong one gets an error event without id.
func (c *Client) SessionAudio(session uint64, samples []int16) error {
	return c.write(nil, &protocol.SessionAudio{Session: session, PCM: protocol.EncodeI16(samples)})
}

// SessionStop stops a live session and gives its text.
func (c *Client) SessionStop(ctx context.Context, session uint64) (string, error) {
	r, err := parse[protocol.TextReply](c.Request(ctx, &protocol.SessionStop{Session: session}, nil))
	if err == nil {
		c.mu.Lock()
		delete(c.sessions, session)
		c.mu.Unlock()
	}
	return r.Text, err
}

// Enhance runs a style of the user on text.
func (c *Client) Enhance(ctx context.Context, req protocol.Enhance) (protocol.EnhanceReply, error) {
	return parse[protocol.EnhanceReply](c.Request(ctx, &req, nil))
}

// Subscribe replaces the events of the connection and gives the ones that it
// gets now.
func (c *Client) Subscribe(ctx context.Context, events []string) ([]string, error) {
	if events == nil {
		events = []string{}
	}
	r, err := parse[protocol.SubscribeReply](c.Request(ctx, &protocol.Subscribe{Events: events}, nil))
	return r.Events, err
}

// Bye asks the host to close the connection, and waits for the end.
func (c *Client) Bye(ctx context.Context) error {
	c.mu.Lock()
	c.byeSent = true
	c.mu.Unlock()
	_, err := c.Request(ctx, &protocol.Bye{}, nil)
	if err != nil && c.Err() == nil {
		return err
	}
	select {
	case <-c.done:
	case <-ctx.Done():
		c.Close()
	}
	return nil
}

// Close ends the connection. The open requests fail with connection_lost.
func (c *Client) Close() error {
	c.mu.Lock()
	if c.end == nil {
		c.byeSent = true
	}
	c.mu.Unlock()
	err := c.conn.Close()
	<-c.done
	return err
}

// queue passes events on in order without blocking the reader. Its channel
// closes after the last event.
type queue struct {
	mu     sync.Mutex
	cond   *sync.Cond
	items  []protocol.Event
	closed bool
	out    chan protocol.Event
}

func newQueue() *queue {
	q := &queue{out: make(chan protocol.Event)}
	q.cond = sync.NewCond(&q.mu)
	go func() {
		defer close(q.out)
		for {
			q.mu.Lock()
			for len(q.items) == 0 && !q.closed {
				q.cond.Wait()
			}
			if len(q.items) == 0 {
				q.mu.Unlock()
				return
			}
			ev := q.items[0]
			q.items = q.items[1:]
			q.mu.Unlock()
			q.out <- ev
		}
	}()
	return q
}

func (q *queue) push(ev protocol.Event) {
	q.mu.Lock()
	q.items = append(q.items, ev)
	q.mu.Unlock()
	q.cond.Signal()
}

func (q *queue) close() {
	q.mu.Lock()
	q.closed = true
	q.mu.Unlock()
	q.cond.Signal()
}
