package bridge

import (
	"sync"
	"sync/atomic"
)

// outgoing is the unbounded queue of lines for the writer of one
// connection. A close ends the writer.
type outgoing struct {
	mu     sync.Mutex
	cond   *sync.Cond
	lines  []string
	closed bool
}

func newOutgoing() *outgoing {
	o := &outgoing{}
	o.cond = sync.NewCond(&o.mu)
	return o
}

// line queues a line. After close it is dropped.
func (o *outgoing) line(s string) {
	o.mu.Lock()
	if !o.closed {
		o.lines = append(o.lines, s)
		o.cond.Signal()
	}
	o.mu.Unlock()
}

// close ends the writer after the lines that are queued.
func (o *outgoing) close() {
	o.mu.Lock()
	o.closed = true
	o.cond.Broadcast()
	o.mu.Unlock()
}

// next is the next line, or false after close when no line is left.
func (o *outgoing) next() (string, bool) {
	o.mu.Lock()
	defer o.mu.Unlock()
	for len(o.lines) == 0 && !o.closed {
		o.cond.Wait()
	}
	if len(o.lines) == 0 {
		return "", false
	}
	s := o.lines[0]
	o.lines[0] = ""
	o.lines = o.lines[1:]
	return s, true
}

// notifier sends events without an id to one connection.
type notifier struct {
	out  *outgoing
	open *atomic.Bool
}

func (n notifier) send(e Event)  { n.out.line(Encode(nil, e)) }
func (n notifier) close()        { n.out.close() }
func (n notifier) isOpen() bool  { return n.open.Load() }
func (n notifier) Event(e Event) { n.send(e) }

// EventSink takes the events of a job or a live session.
type EventSink interface{ Event(Event) }

// responder answers one request: events with its id, then one reply.
type responder struct {
	id       *string
	notifier notifier
	replied  atomic.Bool
}

func newResponder(id *string, n notifier) *responder { return &responder{id: id, notifier: n} }

func (r *responder) event(e Event) { r.notifier.out.line(Encode(r.id, e)) }

// reply sends the one reply. A second reply is dropped.
func (r *responder) reply(reply Reply) {
	if r.replied.Swap(true) {
		return
	}
	r.notifier.out.line(Encode(r.id, reply))
}

func (r *responder) hasID() bool { return r.id != nil }

// noReply marks a request without an id that gets no reply.
func (r *responder) noReply() { r.replied.Store(true) }

func (r *responder) isOpen() bool { return r.notifier.isOpen() }

// finish sends an error when the request ended without a reply (the Drop
// of Responder in Rust).
func (r *responder) finish() {
	if !r.replied.Swap(true) {
		r.notifier.out.line(Encode(r.id, ReplyError(CodeUnsupported, "Sayso stopped this request before it had an answer")))
	}
}
