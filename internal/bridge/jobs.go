package bridge

import (
	"log/slog"
	"slices"
	"sort"
	"sync"
	"sync/atomic"
	"time"
)

// Jobs: the transcription of a file in the background for an app
// (sayso-speech/src/jobs and sayso-host/src/jobs).

const (
	// JobCancelled is the message of a job that the app cancelled.
	JobCancelled = "The job was cancelled."
	// JobGrantGone is the message of a job whose grant the user took back.
	JobGrantGone = "The user took back the grant for this job."
	// StaleAfter is when a dictation that did not end stops holding jobs.
	StaleAfter = 20 * time.Minute
	lookAgain  = 250 * time.Millisecond
)

// ---- DictationGate ----

// DictationGate holds jobs between two engine calls while a dictation runs,
// and holds jobs (not live sessions) while a live session transcribes a part.
type DictationGate struct {
	mu         sync.Mutex
	changed    *sync.Cond
	began      *time.Time
	liveParts  int
	staleAfter time.Duration
}

// NewDictationGate is a gate with StaleAfter.
func NewDictationGate() *DictationGate { return NewDictationGateWithStale(StaleAfter) }

// NewDictationGateWithStale is a gate whose dictations go stale after d.
func NewDictationGateWithStale(d time.Duration) *DictationGate {
	g := &DictationGate{staleAfter: d}
	g.changed = sync.NewCond(&g.mu)
	return g
}

func (g *DictationGate) holds() bool {
	if g.began == nil {
		return false
	}
	if time.Since(*g.began) < g.staleAfter {
		return true
	}
	slog.Warn("host: a dictation began long ago and did not end; jobs go on", "seconds", int(g.staleAfter.Seconds()))
	g.began = nil
	return false
}

// Begin marks the start of a dictation.
func (g *DictationGate) Begin() {
	g.mu.Lock()
	defer g.mu.Unlock()
	if !g.holds() {
		now := time.Now()
		g.began = &now
	}
}

// End marks the end of a dictation.
func (g *DictationGate) End() {
	g.mu.Lock()
	g.began = nil
	g.mu.Unlock()
	g.changed.Broadcast()
}

// Dictating is true while a dictation runs.
func (g *DictationGate) Dictating() bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.holds()
}

// LivePart marks a part of a live session in the engine until the
// function that it gives runs.
func (g *DictationGate) LivePart() func() {
	g.mu.Lock()
	g.liveParts++
	g.mu.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			g.mu.Lock()
			if g.liveParts > 0 {
				g.liveParts--
			}
			g.mu.Unlock()
			g.changed.Broadcast()
		})
	}
}

// WaitIdle waits until no dictation runs. It is false when stop turned true.
func (g *DictationGate) WaitIdle(stop func() bool) bool { return g.wait(stop, false) }

// WaitForJob waits until no dictation runs and no live part is in the engine.
func (g *DictationGate) WaitForJob(stop func() bool) bool { return g.wait(stop, true) }

func (g *DictationGate) wait(stop func() bool, forJob bool) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	for {
		if stop() {
			return false
		}
		if !(g.holds() || (forJob && g.liveParts > 0)) {
			return true
		}
		t := time.AfterFunc(lookAgain, func() { g.mu.Lock(); g.changed.Broadcast(); g.mu.Unlock() })
		g.changed.Wait()
		t.Stop()
	}
}

// Wake makes waiters look again.
func (g *DictationGate) Wake() {
	g.mu.Lock()
	g.changed.Broadcast()
	g.mu.Unlock()
}

// ---- ModelUse ----

// ModelUse says which models jobs and live sessions hold.
type ModelUse struct {
	mu     sync.Mutex
	models map[string]*modelUse
}

type modelUse struct {
	holders  int
	released *time.Time
}

// NewModelUse is an empty ModelUse.
func NewModelUse() *ModelUse { return &ModelUse{models: map[string]*modelUse{}} }

// Hold holds models until the function that it gives runs.
func (m *ModelUse) Hold(models []string) func() {
	m.mu.Lock()
	for _, id := range models {
		u, ok := m.models[id]
		if !ok {
			u = &modelUse{}
			m.models[id] = u
		}
		u.holders++
	}
	m.mu.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			m.mu.Lock()
			defer m.mu.Unlock()
			for _, id := range models {
				if u, ok := m.models[id]; ok {
					if u.holders > 0 {
						u.holders--
					}
					if u.holders == 0 {
						now := time.Now()
						u.released = &now
					}
				}
			}
		})
	}
}

// Wanted are the models that are held, and the ones released less than
// keep ago (keep < 0: all that were ever held), sorted.
func (m *ModelUse) Wanted(keep time.Duration) []string {
	m.mu.Lock()
	defer m.mu.Unlock()
	ids := []string{}
	for id, u := range m.models {
		if u.holders > 0 || keep < 0 || (u.released != nil && time.Since(*u.released) < keep) {
			ids = append(ids, id)
		} else {
			delete(m.models, id)
		}
	}
	sort.Strings(ids)
	return ids
}

// Held is true while a job or a live session holds the model.
func (m *ModelUse) Held(model string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	u, ok := m.models[model]
	return ok && u.holders > 0
}

// ---- JobSpec ----

// VadRun is the VAD of a job: Basic (energy) or a model.
type VadRun struct {
	Basic bool
	Model string
}

// JobSpec is a job that resolve checked.
type JobSpec struct {
	Path        string
	Model       string
	Timestamps  TimestampLevel
	Vad         VadRun
	Diarization string
	Language    string
	Speakers    *uint32
}

// Models are the models of the job, in the order of loading.
func (j JobSpec) Models() []string {
	var out []string
	if !j.Vad.Basic && j.Vad.Model != "" {
		out = append(out, j.Vad.Model)
	}
	out = append(out, j.Model)
	if j.Diarization != "" {
		out = append(out, j.Diarization)
	}
	return out
}

// ---- JobQueue ----

// RunJob runs one job.
type RunJob interface {
	Run(job JobSpec, run *JobRun)
}

type ending int

const (
	endCancelled ending = iota
	endGrantGone
)

func (e ending) event(job uint64) Event {
	if e == endGrantGone {
		code := CodeCapability
		return JobFailed{Job: job, Message: JobGrantGone, Code: &code}
	}
	return JobFailed{Job: job, Message: JobCancelled}
}

type jobEvents struct {
	sink   EventSink
	cancel atomic.Bool
	mu     sync.Mutex
	ended  bool
}

func (e *jobEvents) send(ev Event) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.ended {
		return
	}
	switch ev.(type) {
	case JobDone, JobFailed:
		e.ended = true
	}
	e.sink.Event(ev)
}

func (e *jobEvents) doCancel(job uint64, end ending) bool {
	if e.cancel.Swap(true) {
		return false
	}
	e.send(end.event(job))
	return true
}

// JobRun is the running job as the runner sees it.
type JobRun struct {
	ID     uint64
	events *jobEvents
	gate   *DictationGate
}

// Event sends an event of the job. After job_done or job_failed the job
// sends nothing more.
func (r *JobRun) Event(e Event) { r.events.send(e) }

// Cancelled is true after a cancel.
func (r *JobRun) Cancelled() bool { return r.events.cancel.Load() }

// BeforeEngineCall waits for the gate. It is false after a cancel.
func (r *JobRun) BeforeEngineCall() bool {
	if r.gate == nil {
		return !r.Cancelled()
	}
	return r.gate.WaitForJob(r.Cancelled)
}

// Stopped sends job_failed: the cancel message, or message.
func (r *JobRun) Stopped(cancelled bool, message string) {
	if cancelled {
		message = JobCancelled
	}
	r.Event(JobFailed{Job: r.ID, Message: message})
}

type queuedJob struct {
	id, connection uint64
	job            JobSpec
	sink           EventSink
}

type runningJob struct {
	id, connection uint64
	events         *jobEvents
}

// JobQueue runs one job at a time, in the order they came.
type JobQueue struct {
	mu       sync.Mutex
	wake     *sync.Cond
	waiting  []queuedJob
	running  *runningJob
	nextID   uint64
	closed   bool
	gate     *DictationGate
	runner   RunJob
	onChange func()
	done     chan struct{}
}

// StartJobQueue starts the worker.
func StartJobQueue(runner RunJob, gate *DictationGate) *JobQueue {
	q := &JobQueue{gate: gate, runner: runner, done: make(chan struct{})}
	q.wake = sync.NewCond(&q.mu)
	go q.work()
	return q
}

func (q *JobQueue) changed() {
	q.mu.Lock()
	f := q.onChange
	q.mu.Unlock()
	if f != nil {
		f()
	}
}

// SetOnChange runs f after each change of the jobs.
func (q *JobQueue) SetOnChange(f func()) { q.mu.Lock(); q.onChange = f; q.mu.Unlock() }

// Submit queues a job. accepted runs with its id before its first event.
// It is false when the queue is closed.
func (q *JobQueue) Submit(connection uint64, job JobSpec, sink EventSink, accepted func(uint64)) (uint64, bool) {
	q.mu.Lock()
	if q.closed {
		q.mu.Unlock()
		return 0, false
	}
	q.nextID++
	id := q.nextID
	accepted(id)
	sink.Event(JobProgress{Job: id, Fraction: 0})
	q.waiting = append(q.waiting, queuedJob{id, connection, job, sink})
	q.wake.Broadcast()
	q.mu.Unlock()
	q.changed()
	return id, true
}

// Connections are the connections with a job, the running one first.
func (q *JobQueue) Connections() []uint64 {
	q.mu.Lock()
	defer q.mu.Unlock()
	var out []uint64
	if q.running != nil {
		out = append(out, q.running.connection)
	}
	for _, w := range q.waiting {
		out = append(out, w.connection)
	}
	return out
}

// Cancel cancels a job of a connection.
func (q *JobQueue) Cancel(connection, job uint64) bool {
	return q.cancelWhere(endCancelled, func(id, c uint64) bool { return c == connection && id == job }) > 0
}

// CancelConnection cancels each job of a connection.
func (q *JobQueue) CancelConnection(connection uint64) {
	q.cancelWhere(endCancelled, func(_, c uint64) bool { return c == connection })
}

// GrantGone ends the jobs of a connection whose app lost transcribe.file.
func (q *JobQueue) GrantGone(connection uint64) {
	q.cancelWhere(endGrantGone, func(_, c uint64) bool { return c == connection })
}

// HasJobs is true when the connection has a job.
func (q *JobQueue) HasJobs(connection uint64) bool {
	q.mu.Lock()
	defer q.mu.Unlock()
	if q.running != nil && q.running.connection == connection {
		return true
	}
	return slices.ContainsFunc(q.waiting, func(w queuedJob) bool { return w.connection == connection })
}

// Close cancels each job and stops the worker.
func (q *JobQueue) Close() {
	q.mu.Lock()
	q.closed = true
	q.mu.Unlock()
	q.cancelWhere(endCancelled, func(uint64, uint64) bool { return true })
	q.mu.Lock()
	q.wake.Broadcast()
	q.mu.Unlock()
}

func (q *JobQueue) cancelWhere(end ending, match func(id, connection uint64) bool) int {
	q.mu.Lock()
	var removed, kept []queuedJob
	for _, w := range q.waiting {
		if match(w.id, w.connection) {
			removed = append(removed, w)
		} else {
			kept = append(kept, w)
		}
	}
	q.waiting = kept
	var running *runningJob
	if q.running != nil && match(q.running.id, q.running.connection) {
		running = q.running
	}
	q.mu.Unlock()
	n := len(removed)
	if running != nil && running.events.doCancel(running.id, end) {
		n++
		if q.gate != nil {
			q.gate.Wake()
		}
	}
	for _, w := range removed {
		w.sink.Event(end.event(w.id))
	}
	if len(removed) > 0 {
		q.changed()
	}
	return n
}

func (q *JobQueue) work() {
	defer close(q.done)
	for {
		q.mu.Lock()
		for !q.closed && len(q.waiting) == 0 {
			q.wake.Wait()
		}
		if q.closed {
			q.mu.Unlock()
			return
		}
		next := q.waiting[0]
		q.waiting = q.waiting[1:]
		events := &jobEvents{sink: next.sink}
		q.running = &runningJob{id: next.id, connection: next.connection, events: events}
		moved := slices.Clone(q.waiting)
		q.mu.Unlock()
		for _, w := range moved {
			w.sink.Event(JobProgress{Job: w.id, Fraction: 0})
		}
		q.changed()
		q.runner.Run(next.job, &JobRun{ID: next.id, events: events, gate: q.gate})
		q.mu.Lock()
		q.running = nil
		q.mu.Unlock()
		q.changed()
	}
}
