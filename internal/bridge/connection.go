package bridge

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"
)

// One connection of an app (connection.rs).

const (
	maxLine            = 16 * 1024 * 1024
	maxFirstLine       = 64 * 1024
	firstLineWait      = 5 * time.Second
	maxConnections     = 64
	dictationDoneEvent = "dictation_done"
)

var knownEvents = []struct {
	name string
	cap  Capability
}{{dictationDoneEvent, CapDictationObserve}}

type entry struct {
	appID      string
	out        *outgoing
	writerDone chan struct{}
	events     []string
}

// closeWithHostClosing says host_closing and ends the writer. It gives the
// channel that closes when the writer ended, once.
func (e *entry) closeWithHostClosing() chan struct{} {
	e.out.line(Encode(nil, HostClosing{}))
	e.out.close()
	w := e.writerDone
	e.writerDone = nil
	return w
}

type connection struct {
	s        *shared
	number   uint64
	caller   *Caller
	writer   *Peer
	notifier notifier
	jobsMu   sync.Mutex
	jobs     map[uint64]bool
	open     *atomic.Bool
	asking   *atomic.Bool
}

func serve(s *shared, number uint64, c *conn) {
	if c.release != nil {
		defer c.release()
	}
	out := newOutgoing()
	open := &atomic.Bool{}
	open.Store(true)
	n := notifier{out: out, open: open}
	writerDone := make(chan struct{})
	go func() {
		defer close(writerDone)
		writeLoop(c.writer, out, c.closer)
	}()
	s.connMu.Lock()
	if s.isClosing() {
		s.connMu.Unlock()
		e := &entry{out: out, writerDone: writerDone}
		if w := e.closeWithHostClosing(); w != nil {
			<-w
		}
		return
	}
	s.connections[number] = &entry{out: out, writerDone: writerDone}
	s.connMu.Unlock()

	cn := &connection{s: s, number: number, notifier: n, jobs: map[uint64]bool{}, open: open, asking: &atomic.Bool{}}
	firstLine := make(chan struct{})
	var firstOnce sync.Once
	gotFirst := func() { firstOnce.Do(func() { close(firstLine) }) }
	go func() {
		select {
		case <-firstLine:
		case <-time.After(firstLineWait):
			slog.Debug("host: a program said nothing, and its connection closes")
			c.closer()
		}
	}()
	reader := bufio.NewReaderSize(c.reader, 64*1024)
	for {
		limit := maxFirstLine
		if cn.caller != nil {
			limit = maxLine
		}
		line, err := readLine(reader, limit+1)
		if len(line) == 0 && err != nil {
			if !errors.Is(err, io.EOF) {
				slog.Debug("host: a connection failed", "err", err)
			}
			break
		}
		gotFirst()
		if len(line) > limit {
			out.line(Encode(nil, ReplyError(CodeBadRequest, "the line is longer than 16 MiB")))
			break
		}
		if !utf8.Valid(line) {
			out.line(Encode(nil, ReplyError(CodeBadRequest, "the line is not UTF-8")))
			break
		}
		text := strings.TrimRight(string(line), "\r\n")
		if strings.TrimSpace(text) == "" {
			if err != nil {
				break
			}
			continue
		}
		if !cn.checkWriter(c.peer()) {
			break
		}
		if cn.handle(text) {
			break
		}
		if err != nil {
			break
		}
	}
	gotFirst()
	open.Store(false)
	if s.jobs != nil {
		s.jobs.CancelConnection(number)
	}
	if s.live != nil {
		s.live.ConnectionClosed(number)
	}
	out.close()
	s.connMu.Lock()
	e := s.connections[number]
	delete(s.connections, number)
	s.connMu.Unlock()
	if e != nil && e.writerDone != nil {
		<-e.writerDone
	}
	if cn.caller != nil {
		slog.Info("host: an app disconnected", "app", cn.caller.Name)
	}
}

// readLine reads up to and with a newline, at most limit bytes. A longer
// line comes back with limit bytes.
func readLine(r *bufio.Reader, limit int) ([]byte, error) {
	var buf []byte
	for {
		part, err := r.ReadSlice('\n')
		buf = append(buf, part...)
		if len(buf) >= limit {
			return buf[:limit], nil
		}
		if err == bufio.ErrBufferFull {
			continue
		}
		return buf, err
	}
}

func writeLoop(w io.Writer, out *outgoing, closer func()) {
	defer closer()
	for {
		line, ok := out.next()
		if !ok {
			return
		}
		if _, err := io.WriteString(w, line+"\n"); err != nil {
			slog.Debug("host: cannot write to a connection", "err", err)
			out.close()
			return
		}
	}
}

func (cn *connection) responder(id *string) *responder { return newResponder(id, cn.notifier) }

// checkWriter identifies the first writer, and then checks that the same
// process wrote each line.
func (cn *connection) checkWriter(peer Peer, err error) bool {
	if cn.writer == nil {
		cn.identify(peer, err)
		return true
	}
	if err != nil {
		slog.Warn("host: a connection closes", "err", err)
		return false
	}
	if !cn.writer.SameProcess(peer) {
		name := "?"
		if cn.caller != nil {
			name = cn.caller.Name
		}
		slog.Warn("host: another process wrote to the connection of an app, so it closes", "app", name)
		return false
	}
	return true
}

func (cn *connection) identify(peer Peer, err error) {
	if err != nil {
		slog.Warn("host: a program has no identity", "err", err)
		return
	}
	caller, err := cn.s.identify.Identify(peer)
	if err != nil {
		slog.Warn("host: a program has no identity", "err", err)
		return
	}
	p := peer
	cn.writer = &p
	slog.Info("host: an app connected", "app", caller.Name)
	cn.s.connMu.Lock()
	if e, ok := cn.s.connections[cn.number]; ok {
		e.appID = caller.Identity.ID()
	}
	cn.s.connMu.Unlock()
	cn.caller = &caller
}

// handle runs one line. It is true when the connection must close.
func (cn *connection) handle(line string) bool {
	decoded, derr := DecodeRequest(line)
	if cn.caller == nil {
		var id *string
		switch d := decoded.(type) {
		case *Message[Request]:
			id = d.ID
		case *Unknown:
			id = d.ID
		}
		var bad *BadFields
		if errors.As(derr, &bad) {
			id = bad.ID
		}
		cn.responder(id).reply(notApproved())
		return true
	}
	caller := *cn.caller
	var notMsg *NotAMessage
	var bad *BadFields
	switch {
	case errors.As(derr, &notMsg):
		cn.responder(nil).reply(ReplyError(CodeBadRequest, notMsg.Message))
		return false
	case errors.As(derr, &bad):
		cn.responder(bad.ID).reply(ReplyError(CodeBadRequest, fmt.Sprintf("bad %s request: %s", bad.Kind, bad.Message)))
		return false
	}
	if u, ok := decoded.(*Unknown); ok {
		cn.responder(u.ID).reply(ReplyError("", "unknown request type "+u.Kind))
		return false
	}
	msg := decoded.(*Message[Request])
	r := cn.responder(msg.ID)
	switch req := msg.Body.(type) {
	case *Bye:
		r.reply(ReplyOK(nil))
		return true
	case *Hello:
		return cn.hello(caller, req, r)
	}
	apps := cn.s.apps.Current()
	rec := apps.Find(caller.Identity)
	if rec == nil || !rec.Allowed {
		if cn.asking.Load() {
			r.reply(ReplyError(CodeBadRequest, "wait for the reply to hello: the user has not answered yet"))
			return false
		}
		slog.Info("host: an app is not approved", "app", caller.Name)
		r.reply(notApproved())
		return true
	}
	granted := rec.Capabilities()
	for _, c := range RequestCapabilities(msg.Body) {
		if !slices.Contains(granted, c) {
			r.reply(capabilityError(c))
			return false
		}
	}
	appID := caller.Identity.ID()
	switch req := msg.Body.(type) {
	case *ListModels:
		cn.spawn(r, func() { listModels(cn.s, req, r) })
	case *DownloadModel:
		cn.spawn(r, func() { downloadModel(cn.s, appID, req, r) })
	case *TranscribeFile:
		defer r.finish()
		transcribeFile(cn.s, cn.number, &cn.jobsMu, cn.jobs, req, r)
	case *CancelJob:
		defer r.finish()
		cancelJob(cn.s, cn.number, &cn.jobsMu, cn.jobs, req, r)
	case *SessionStart:
		var pid *uint32
		if cn.writer != nil {
			p := cn.writer.PID
			pid = &p
		}
		number := cn.number
		cn.spawn(r, func() { sessionStart(cn.s, number, appID, pid, req, r) })
	case *SessionAudio:
		defer r.finish()
		sessionAudio(cn.s, cn.number, req, r)
	case *SessionStop:
		number := cn.number
		cn.spawn(r, func() { sessionStop(cn.s, number, req, r) })
	case *Enhance:
		cn.spawn(r, func() { enhanceRequest(cn.s, appID, req, r) })
	case *Subscribe:
		cn.subscribe(req, r)
	default:
		r.reply(ReplyError(CodeUnsupported, fmt.Sprintf("this version of Sayso cannot run %s yet", msg.Body.Kind())))
	}
	return false
}

func (cn *connection) hello(caller Caller, req *Hello, r *responder) bool {
	if cn.asking.Load() {
		r.reply(ReplyError(CodeBadRequest, "an earlier hello waits for the user"))
		return false
	}
	step := helloStepOf(cn.s, caller, req)
	switch step.kind {
	case stepReply:
		r.reply(ReplyOK(step.reply))
	case stepRefuse:
		slog.Info("host: an app is not approved", "app", caller.Name)
		r.reply(notApproved())
		return true
	case stepAsk:
		r.event(ApprovalWaiting{})
		cn.asking.Store(true)
		go func() {
			defer r.finish()
			askAndReply(cn.s, caller, req, step.question, cn.open, cn.asking, r)
		}()
	}
	return false
}

func (cn *connection) subscribe(req *Subscribe, r *responder) {
	events := []string{}
	for _, e := range knownEvents {
		if slices.Contains(req.Events, e.name) {
			events = append(events, e.name)
		}
	}
	cn.s.connMu.Lock()
	if e, ok := cn.s.connections[cn.number]; ok {
		e.events = slices.Clone(events)
	}
	cn.s.connMu.Unlock()
	r.reply(ReplyOK(SubscribeReply{Events: events}))
}

func (cn *connection) spawn(r *responder, work func()) {
	go func() {
		defer r.finish()
		work()
	}()
}

// sendToSubscribers sends an event to each connection that subscribed to
// it and whose app has its capability.
func sendToSubscribers(s *shared, name string, event Event) {
	if s.isClosing() {
		return
	}
	var capability Capability
	found := false
	for _, e := range knownEvents {
		if e.name == name {
			capability, found = e.cap, true
		}
	}
	if !found {
		return
	}
	s.connMu.Lock()
	any := false
	for _, e := range s.connections {
		if slices.Contains(e.events, name) {
			any = true
		}
	}
	s.connMu.Unlock()
	if !any {
		return
	}
	apps := s.apps.Current()
	line := Encode(nil, event)
	s.connMu.Lock()
	defer s.connMu.Unlock()
	for _, e := range s.connections {
		if !slices.Contains(e.events, name) || e.appID == "" {
			continue
		}
		if rec := apps.Record(e.appID); rec != nil && rec.Allowed && slices.Contains(rec.Capabilities(), capability) {
			e.out.line(line)
		}
	}
}

func capabilityError(missing Capability) Reply {
	code := CodeCapability
	c := missing
	return &ErrorReply{Code: &code, Message: fmt.Sprintf("the user did not grant the capability %s", missing), Capability: &c}
}
