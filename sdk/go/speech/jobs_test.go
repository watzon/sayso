package speech

import (
	"path/filepath"
	"reflect"
	"sync"
	"testing"
	"time"
)

func TestChunksJoinPadAndCut(t *testing.T) {
	parts := []SpeechPart{{StartMs: 1000, EndMs: 2000}, {StartMs: 2500, EndMs: 4000}, {StartMs: 9000, EndMs: 9500}, {StartMs: 50_000, EndMs: 70_000}}
	got := planChunks(parts, 60_000, maxChunkMs)
	want := []Chunk{{800, 4200}, {8800, 9700}, {49_800, 60_000}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("%v", got)
	}
	if len(planChunks(nil, 1000, maxChunkMs)) != 0 {
		t.Fatal("no speech, no chunk")
	}
	long := planChunks([]SpeechPart{{StartMs: 0, EndMs: 9000}, {StartMs: 9500, EndMs: 19000}}, 20_000, maxChunkMsWithoutTimes)
	if len(long) != 2 {
		t.Fatalf("a chunk stays under the limit: %v", long)
	}
}

func TestSpeakers(t *testing.T) {
	turns := []RawTurn{{0, 60_000, 0}, {60_000, 120_000, 1}, {30_000, 30_500, 2}}
	folded := foldSmallSpeakers(turns)
	for _, tr := range folded {
		if tr.Speaker == 2 {
			t.Fatal("a small speaker stays", folded)
		}
	}
	segs := [][2]uint64{{0, 10_000}, {10_000, 20_000}, {70_000, 80_000}, {200_000, 210_000}}
	sp := assignSpeakers(segs, turns[:2])
	if *sp[0] != 0 || *sp[2] != 1 || *sp[3] != 1 {
		t.Fatal("nearest speaker", *sp[0], *sp[2], *sp[3])
	}
	got := segmentTurns(segs, sp)
	want := []Turn{{StartMs: 0, EndMs: 20_000, Speaker: "1"}, {StartMs: 70_000, EndMs: 210_000, Speaker: "2"}}
	if !reflect.DeepEqual(got, want) {
		t.Fatal(got)
	}
}

type sink struct {
	mu     sync.Mutex
	events []Event
}

func (s *sink) Event(e Event) { s.mu.Lock(); s.events = append(s.events, e); s.mu.Unlock() }
func (s *sink) all() []Event {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Event(nil), s.events...)
}

type blockingRunner struct {
	started chan uint64
}

func (b blockingRunner) Run(job JobSpec, run *JobRun) {
	b.started <- run.ID
	for run.BeforeEngineCall() {
		time.Sleep(5 * time.Millisecond)
	}
	run.Stopped(true, "")
}

func TestQueueRunsInOrderAndCancels(t *testing.T) {
	gate := NewDictationGate()
	runner := blockingRunner{started: make(chan uint64, 4)}
	q := StartJobQueue(runner, gate)
	defer q.Close()
	a, b := &sink{}, &sink{}
	idA, _ := q.Submit(1, JobSpec{}, a, func(uint64) {})
	idB, _ := q.Submit(2, JobSpec{}, b, func(uint64) {})
	if <-runner.started != idA {
		t.Fatal("order")
	}
	if !reflect.DeepEqual(q.Connections(), []uint64{1, 2}) {
		t.Fatal(q.Connections())
	}
	q.GrantGone(2)
	if ev := b.all(); len(ev) < 2 || ev[len(ev)-1].(*JobFailed).Message != JobGrantGone || *ev[len(ev)-1].(*JobFailed).Code != CodeCapability {
		t.Fatal(ev)
	}
	q.CancelConnection(1)
	deadline := time.Now().Add(2 * time.Second)
	for q.HasJobs(1) && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	ev := a.all()
	failed := 0
	for _, e := range ev {
		if f, ok := e.(*JobFailed); ok {
			failed++
			if f.Message != JobCancelled {
				t.Fatal(f)
			}
		}
	}
	if failed != 1 {
		t.Fatal("one end only", ev)
	}
	_ = idB
}

func TestGateHoldsJobsDuringADictation(t *testing.T) {
	g := NewDictationGateWithStale(time.Hour)
	g.Begin()
	done := make(chan bool)
	go func() { done <- g.WaitForJob(func() bool { return false }) }()
	select {
	case <-done:
		t.Fatal("ran during a dictation")
	case <-time.After(50 * time.Millisecond):
	}
	g.End()
	if !<-done {
		t.Fatal("not released")
	}
	release := g.LivePart()
	if !g.WaitIdle(func() bool { return false }) {
		t.Fatal("a live part holds jobs only")
	}
	stopped := g.WaitForJob(func() bool { return true })
	release()
	if stopped {
		t.Fatal("stop")
	}
	stale := NewDictationGateWithStale(time.Millisecond)
	stale.Begin()
	time.Sleep(5 * time.Millisecond)
	if stale.Dictating() {
		t.Fatal("stale")
	}
}

func TestModelUse(t *testing.T) {
	m := NewModelUse()
	release := m.Hold([]string{"b", "a"})
	if !m.Held("a") || !reflect.DeepEqual(m.Wanted(0), []string{"a", "b"}) {
		t.Fatal("held")
	}
	release()
	release()
	if m.Held("a") || !reflect.DeepEqual(m.Wanted(time.Hour), []string{"a", "b"}) || len(m.Wanted(0)) != 0 {
		t.Fatal("released")
	}
}

func boolp(b bool) *bool { return &b }

func absPath(name string) string {
	p, _ := filepath.Abs(filepath.Join(string(filepath.Separator), "tmp", name))
	return p
}
