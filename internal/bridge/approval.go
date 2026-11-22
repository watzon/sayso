package bridge

import (
	"log/slog"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// The approval question (approval.rs): one question at a time, at most 4
// apps wait for one, at most 4 connections of one app wait, and an app
// whose question went away without an answer waits 30 s.

const (
	approvalMaxConnections = 4
	approvalMaxIdentities  = 4
	approvalCooldown       = 30 * time.Second
	approvalLook           = 50 * time.Millisecond
)

// hostFeatures are the optional parts of the Host API that this host serves.
var hostFeatures = []string{}

// ApprovalQuestion asks the user about an app.
type ApprovalQuestion struct {
	AppID        string
	Identity     Identity
	Name         string
	Signer       string
	ClaimedName  string
	Interpreter  bool
	Capabilities []Capability
	// Approved is true when the user allowed the app before and the question
	// is about new capabilities.
	Approved bool
}

type approvalDone int

const (
	doneNone approvalDone = iota
	doneAnswered
	doneNoAnswer
)

type approvalEntry struct {
	number  uint64
	appID   string
	waiters []*atomic.Bool
	done    approvalDone
}

type approvals struct {
	mu       sync.Mutex
	changed  *sync.Cond
	entries  []*approvalEntry
	showing  bool
	cooling  map[string]time.Time
	next     uint64
	cooldown time.Duration
}

type waitResult int

const (
	waitDone waitResult = iota
	waitBusy
	waitLeft
)

func newApprovals(cooldown time.Duration) *approvals {
	a := &approvals{cooling: map[string]time.Time{}, cooldown: cooldown}
	a.changed = sync.NewCond(&a.mu)
	return a
}

// waitFor waits on the cond for at most d.
func (a *approvals) waitFor(d time.Duration) {
	t := time.AfterFunc(d, func() {
		a.mu.Lock()
		a.changed.Broadcast()
		a.mu.Unlock()
	})
	a.changed.Wait()
	t.Stop()
}

func live(e *approvalEntry) int {
	n := 0
	for _, f := range e.waiters {
		if f.Load() {
			n++
		}
	}
	return n
}

func (a *approvals) find(number uint64) (int, *approvalEntry) {
	for i, e := range a.entries {
		if e.number == number {
			return i, e
		}
	}
	return -1, nil
}

func (a *approvals) leave(number uint64, open *atomic.Bool) {
	i, e := a.find(number)
	if e == nil {
		return
	}
	e.waiters = slices.DeleteFunc(e.waiters, func(f *atomic.Bool) bool { return f == open })
	if len(e.waiters) == 0 {
		a.entries = slices.Delete(a.entries, i, i+1)
	}
}

func (a *approvals) wait(s *shared, caller Caller, q ApprovalQuestion, open *atomic.Bool) waitResult {
	a.mu.Lock()
	defer a.mu.Unlock()
	now := time.Now()
	for id, until := range a.cooling {
		if !until.After(now) {
			delete(a.cooling, id)
		}
	}
	if _, ok := a.cooling[q.AppID]; ok {
		return waitBusy
	}
	identities := 0
	for _, e := range a.entries {
		if e.done == doneNone && live(e) > 0 {
			identities++
		}
	}
	var number uint64
	var same *approvalEntry
	for _, e := range a.entries {
		if e.done == doneNone && e.appID == q.AppID {
			same = e
			break
		}
	}
	switch {
	case same != nil && live(same) >= approvalMaxConnections:
		return waitBusy
	case same != nil:
		same.waiters = append(same.waiters, open)
		number = same.number
	case identities >= approvalMaxIdentities:
		return waitBusy
	default:
		number = a.next
		a.next++
		a.entries = append(a.entries, &approvalEntry{number: number, appID: q.AppID, waiters: []*atomic.Bool{open}})
	}
	for {
		_, e := a.find(number)
		if e == nil {
			return waitLeft
		}
		if e.done != doneNone {
			result := waitDone
			if e.done == doneNoAnswer {
				result = waitLeft
			}
			a.leave(number, open)
			a.changed.Broadcast()
			return result
		}
		if !open.Load() || s.isClosing() {
			a.leave(number, open)
			a.changed.Broadcast()
			return waitLeft
		}
		nextUp := false
		for _, x := range a.entries {
			if x.done == doneNone {
				nextUp = x.number == number
				break
			}
		}
		if !a.showing && nextUp {
			a.showing = true
			a.mu.Unlock()
			done := a.show(s, caller, q, number)
			a.mu.Lock()
			a.showing = false
			if _, e := a.find(number); e != nil {
				e.done = done
			}
			a.changed.Broadcast()
			continue
		}
		a.waitFor(approvalLook)
	}
}

func (a *approvals) show(s *shared, caller Caller, q ApprovalQuestion, number uint64) approvalDone {
	slog.Info("host: asking the user about an app", "app", q.Name, "capabilities", q.Capabilities)
	withdrawn := func() bool {
		a.mu.Lock()
		defer a.mu.Unlock()
		if s.isClosing() {
			return true
		}
		_, e := a.find(number)
		return e == nil || live(e) == 0
	}
	allow, answered := s.services.AskApproval(q, withdrawn)
	if !answered {
		if !s.isClosing() && withdrawn() {
			a.mu.Lock()
			a.cooling[q.AppID] = time.Now().Add(a.cooldown)
			a.mu.Unlock()
		}
		return doneNoAnswer
	}
	slog.Info("host: the user answered", "app", q.Name, "allow", allow)
	var err error
	if q.Approved {
		err = s.apps.answerNew(q.AppID, q.Capabilities, allow)
	} else {
		err = s.apps.answerFirst(caller, q.Capabilities, allow)
	}
	if err != nil {
		slog.Warn("host: cannot keep the answer", "app", q.Name, "err", err)
		return doneNoAnswer
	}
	return doneAnswered
}

type helloStepKind int

const (
	stepReply helloStepKind = iota
	stepRefuse
	stepAsk
)

type helloStep struct {
	kind     helloStepKind
	reply    HelloReply
	question ApprovalQuestion
}

func helloStepOf(s *shared, caller Caller, req *Hello) helloStep {
	if s.apps.IsBroken() {
		return helloStep{kind: stepRefuse}
	}
	apps := s.apps.Current()
	r := apps.Find(caller.Identity)
	switch {
	case r != nil && !r.Allowed:
		return helloStep{kind: stepRefuse}
	case r != nil:
		if fresh := unanswered(r, req.Capabilities); len(fresh) > 0 {
			return helloStep{kind: stepAsk, question: question(caller, req, fresh, true)}
		}
		return helloStep{kind: stepReply, reply: helloReply(s, r, req.Capabilities)}
	}
	return helloStep{kind: stepAsk, question: question(caller, req, askable(req.Capabilities), false)}
}

func question(caller Caller, req *Hello, caps []Capability, approved bool) ApprovalQuestion {
	var claimed string
	if req.App != nil {
		if n, ok := CleanName(req.App.Name); ok && !strings.EqualFold(n, caller.Name) {
			claimed = n
		}
	}
	return ApprovalQuestion{AppID: caller.Identity.ID(), Identity: caller.Identity, Name: caller.Name, Signer: caller.Signer, ClaimedName: claimed, Interpreter: caller.Interpreter, Capabilities: caps, Approved: approved}
}

func helloReply(s *shared, r *AppRecord, asked []Capability) HelloReply {
	denied := r.DeniedCapabilities()
	for _, c := range asked {
		if !c.Known() {
			denied = append(denied, c)
		}
	}
	return HelloReply{HostVersion: s.hostVersion, Protocol: Protocol, Features: slices.Clone(hostFeatures), Capabilities: r.Capabilities(), Denied: denied}
}

func notApproved() Reply {
	return ReplyError(CodeNotApproved, "the user did not approve this app in Sayso")
}

// askAndReply asks the user, then replies to hello.
func askAndReply(s *shared, caller Caller, req *Hello, q ApprovalQuestion, open, asking *atomic.Bool, r *responder) {
	for {
		var next helloStep
		switch s.approvals.wait(s, caller, q, open) {
		case waitBusy:
			approved := q.Approved
			asking.Store(false)
			r.reply(ReplyError(CodeBusy, "Sayso cannot ask about this app now; try again later"))
			if !approved {
				r.notifier.close()
			}
			return
		case waitDone:
			next = helloStepOf(s, caller, req)
		case waitLeft:
			next = helloStepOf(s, caller, req)
			if next.kind == stepAsk {
				if next.question.Approved {
					if rec := s.apps.Current().Find(caller.Identity); rec != nil {
						next = helloStep{kind: stepReply, reply: helloReply(s, rec, req.Capabilities)}
					} else {
						next = helloStep{kind: stepRefuse}
					}
				} else {
					next = helloStep{kind: stepRefuse}
				}
			}
		}
		switch next.kind {
		case stepReply:
			asking.Store(false)
			r.reply(ReplyOK(next.reply))
			return
		case stepRefuse:
			asking.Store(false)
			r.reply(notApproved())
			r.notifier.close()
			return
		case stepAsk:
			q = next.question
		}
	}
}
