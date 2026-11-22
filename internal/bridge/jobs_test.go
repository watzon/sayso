package bridge

import (
	"math"
	"os"
	"path/filepath"
	"reflect"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/models"
)

func TestChunksJoinPadAndCut(t *testing.T) {
	parts := []SpeechPart{{1000, 2000}, {2500, 4000}, {9000, 9500}, {50_000, 70_000}}
	got := planChunks(parts, 60_000, maxChunkMs)
	want := []Chunk{{800, 4200}, {8800, 9700}, {49_800, 60_000}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("%v", got)
	}
	if len(planChunks(nil, 1000, maxChunkMs)) != 0 {
		t.Fatal("no speech, no chunk")
	}
	long := planChunks([]SpeechPart{{0, 9000}, {9500, 19000}}, 20_000, maxChunkMsWithoutTimes)
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
	want := []Turn{{0, 20_000, "1"}, {70_000, 210_000, "2"}}
	if !reflect.DeepEqual(got, want) {
		t.Fatal(got)
	}
}

func tone(seconds float64, on func(i int) bool) []float32 {
	n := int(seconds * SampleRate)
	out := make([]float32, n)
	for i := range out {
		if on(i) {
			out[i] = float32(0.3 * math.Sin(2*math.Pi*300*float64(i)/SampleRate))
		} else {
			out[i] = float32(0.0005 * math.Sin(float64(i)))
		}
	}
	return out
}

func TestBasicVadFindsSpeechAndCutsLongParts(t *testing.T) {
	parts := DetectSpeech(tone(6, func(i int) bool { return i >= 2*SampleRate && i < 3*SampleRate }))
	if len(parts) != 1 || parts[0].StartMs < 1900 || parts[0].StartMs > 2100 || parts[0].EndMs < 2900 || parts[0].EndMs > 3100 {
		t.Fatal(parts)
	}
	if got := DetectSpeech(tone(3, func(int) bool { return false })); len(got) != 0 {
		t.Fatal("silence", got)
	}
	if got := DetectSpeech(tone(1, func(i int) bool { return i < SampleRate/10 })); len(got) != 0 {
		t.Fatal("a click is no speech", got)
	}
	long := DetectSpeech(tone(65, func(i int) bool { return i%SampleRate >= 2*vadFrame }))
	for _, p := range long {
		if p.EndMs-p.StartMs > vadMaxPartMs {
			t.Fatal("too long", p)
		}
	}
	if len(long) < 3 {
		t.Fatal(long)
	}
}

func TestResamplerKeepsLengthAndTone(t *testing.T) {
	for _, rate := range []uint32{8000, 22050, 44100, 48000, 16000, 96000, 11025} {
		r := NewStreamResampler(rate, SampleRate)
		in := make([]float32, int(rate))
		for i := range in {
			in[i] = float32(math.Sin(2 * math.Pi * 440 * float64(i) / float64(rate)))
		}
		var out []float32
		for i := 0; i < len(in); i += 1000 {
			out = r.Process(in[i:min(i+1000, len(in))], out)
		}
		out = r.Process(make([]float32, rate/10), out)
		if len(out) < SampleRate-50 || len(out) > SampleRate+SampleRate/10+50 {
			t.Fatalf("%d: %d samples", rate, len(out))
		}
		var peak float32
		for _, s := range out[2000:14000] {
			peak = max(peak, float32(math.Abs(float64(s))))
		}
		if peak < 0.9 || peak > 1.1 {
			t.Fatalf("%d: peak %v", rate, peak)
		}
	}
}

func TestReadAudioMixesAndResamples(t *testing.T) {
	dir := t.TempDir()
	for _, c := range []struct{ rate, channels int }{{16000, 1}, {44100, 2}, {48000, 1}} {
		path := filepath.Join(dir, "a.wav")
		writeTone(t, path, c.rate, c.channels, 2)
		a, err := ReadAudio(path)
		if err != nil {
			t.Fatal(err)
		}
		if a.LenMs() < 1990 || a.LenMs() > 2000 {
			t.Fatalf("%v: %d ms", c, a.LenMs())
		}
	}
	if _, err := ReadAudio(filepath.Join(dir, "missing.wav")); err == nil {
		t.Fatal("missing")
	}
	bad := filepath.Join(dir, "bad.mp4")
	writeFile(t, bad, "\x00\x00\x00\x20ftypM4A \x00\x00")
	if _, err := ReadAudio(bad); err == nil {
		t.Fatal("m4a")
	}
	text := filepath.Join(dir, "notes.txt")
	writeFile(t, text, "hello there")
	if _, err := ReadAudio(text); err == nil {
		t.Fatal("text")
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
	if ev := b.all(); len(ev) < 2 || ev[len(ev)-1].(JobFailed).Message != JobGrantGone || *ev[len(ev)-1].(JobFailed).Code != CodeCapability {
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
		if f, ok := e.(JobFailed); ok {
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

func TestResolveJob(t *testing.T) {
	final := finalSpeechModel(t)
	onDisk := map[models.ModelID]bool{final.ID: true}
	view := EngineView{OnDisk: func(id models.ModelID) bool { return onDisk[id] }}
	user := UserChoices{FinalModel: final.ID, Language: "en"}
	job, r := ResolveJob(&TranscribeFile{Path: absPath("a.wav")}, user, view)
	if r != nil || job.Model != string(final.ID) || !job.Vad.Basic || job.Language != "en" {
		t.Fatalf("%+v %v", job, r)
	}
	zero := uint32(0)
	for _, c := range []struct {
		req  TranscribeFile
		code ErrorCode
	}{
		{TranscribeFile{Path: "a.wav"}, CodeBadRequest},
		{TranscribeFile{Path: absPath("a.wav"), Speakers: &zero}, CodeBadRequest},
		{TranscribeFile{Path: absPath("a.wav"), Model: strp("nope")}, CodeUnknownModel},
		{TranscribeFile{Path: absPath("a.wav"), Diarize: boolp(true)}, CodeModelNotDownloaded},
	} {
		if _, r := ResolveJob(&c.req, user, view); r == nil || r.Code != c.code {
			t.Errorf("%+v: %v", c.req, r)
		}
	}
	if _, r := ResolveJob(&TranscribeFile{Path: absPath("a.wav")}, UserChoices{FinalModel: "openai:whisper-1"}, view); r == nil || r.Code != CodeUnsupported {
		t.Fatal(r)
	}
	if vads := models.ModelsFor(models.TaskVad); len(vads) > 0 {
		onDisk[vads[0].ID] = true
		job, _ := ResolveJob(&TranscribeFile{Path: absPath("a.wav")}, user, view)
		if job.Vad.Model != string(vads[0].ID) {
			t.Fatal("the VAD model on disk", job.Vad)
		}
		noVad := EngineView{OnDisk: view.OnDisk, Features: []string{"segments"}}
		if _, r := ResolveJob(&TranscribeFile{Path: absPath("a.wav"), VadModel: strp(string(vads[0].ID))}, user, noVad); r == nil || r.Code != CodeUnsupported {
			t.Fatal("an engine without vad", r)
		}
	}
	if _, r := ResolveJob(&TranscribeFile{Path: absPath("a.wav"), Model: strp(string(final.ID)), VadModel: strp("basic")}, user, view); r != nil {
		t.Fatal(r)
	}
}

func boolp(b bool) *bool { return &b }

func absPath(name string) string {
	p, _ := filepath.Abs(filepath.Join(string(filepath.Separator), "tmp", name))
	return p
}

func TestListModelsRolesAndState(t *testing.T) {
	cfg := testConfig("x")
	all := ListModelInfos(models.LocalModels(), func(models.ModelID) engine.ModelStatus { return engine.ModelStatus{} }, cfg, nil)
	if len(all) != len(models.LocalModels()) {
		t.Fatal(len(all))
	}
	for _, m := range all {
		if m.ID == models.BasicVAD || m.State != StateNotDownloaded || m.Name == "" {
			t.Fatalf("%+v", m)
		}
	}
	other := Role("translate")
	if len(ListModelInfos(models.LocalModels(), func(models.ModelID) engine.ModelStatus { return engine.ModelStatus{} }, cfg, &other)) != 0 {
		t.Fatal("unknown role")
	}
}

func writeFile(t *testing.T, path, text string) {
	if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
		t.Fatal(err)
	}
}
