package bridge

import (
	"fmt"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/engine"
)

// Live sessions with a fake engine, a fake microphone, and a fake system
// audio (ported from sayso-speech/src/live/tests.rs). The engine events go
// through the real bus of the host.

// liveFake is an engine. The preview stream sends a partial for each push.
// The VAD session finds speech with Basic VAD. The final pass gives one
// segment for the loud samples of a part, with the text part<n>.
// Diarization gives two speakers: 1 until 3.1 s, then 0.
type liveFake struct {
	mu          sync.Mutex
	released    *sync.Cond
	hold        bool
	events      chan engine.Event
	loads       []string
	streams     map[uint64]bool
	vads        map[uint64]*BasicVad
	opened      []uint64
	pushed      map[uint64]int
	transcribed int
	inFinal     bool
	diarized    []int
	// maxStreams is the most streams and VAD sessions open at once, or 0.
	maxStreams int
	// vadKeepsOpenSpeech: the VAD session does not end its open speech at EndVad.
	vadKeepsOpenSpeech bool
	// texts: a loud part gives the first, another part the second.
	texts *[2]string
	// failFinal: the final pass fails with this message and model_not_downloaded.
	failFinal string
	// notOnDisk are models that are not downloaded.
	notOnDisk map[string]bool
	// loadStarted and loadRelease, when set, make the next load say that it
	// runs and wait.
	loadStarted, loadRelease chan struct{}
}

func newLiveFake() *liveFake {
	f := &liveFake{events: make(chan engine.Event, 1<<16), streams: map[uint64]bool{}, vads: map[uint64]*BasicVad{}, pushed: map[uint64]int{}}
	f.released = sync.NewCond(&f.mu)
	return f
}

func (f *liveFake) emit(e engine.Event) { f.events <- e }

func (f *liveFake) openNow() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.streams) + len(f.vads)
}

func (f *liveFake) holdFinalPass(hold bool) {
	f.mu.Lock()
	f.hold = hold
	f.mu.Unlock()
	f.released.Broadcast()
}

func (f *liveFake) emitParts(session uint64, parts []SpeechPart) {
	for _, p := range parts {
		f.emit(engine.Event{Kind: engine.EventSpeechStart, Session: session, StartMs: p.StartMs})
		f.emit(engine.Event{Kind: engine.EventSpeechEnd, Session: session, StartMs: p.StartMs, EndMs: p.EndMs})
	}
}

func (f *liveFake) StartState() EngineStart { return EngineStarted }
func (f *liveFake) Features() []string {
	return []string{"segments", "vad", "diarization", "concurrent_streams"}
}
func (f *liveFake) Status(m string) engine.ModelStatus {
	if f.notOnDisk[m] {
		return engine.ModelStatus{State: engine.StateNotDownloaded}
	}
	return engine.ModelStatus{State: engine.StateDownloaded}
}
func (f *liveFake) Download(string) error { return nil }
func (f *liveFake) Load(m string) error {
	f.mu.Lock()
	started, release := f.loadStarted, f.loadRelease
	f.loadStarted, f.loadRelease = nil, nil
	f.mu.Unlock()
	if started != nil {
		close(started)
		<-release
	}
	f.mu.Lock()
	f.loads = append(f.loads, m)
	f.mu.Unlock()
	return nil
}
func (f *liveFake) Transcribe([]float32, string, string) (string, error) {
	return "", fmt.Errorf("a live session transcribes with segments")
}
func (f *liveFake) DetectSpeech(string, []float32) ([]SpeechPart, error) {
	return nil, fmt.Errorf("a live session finds speech with a VAD session")
}
func (f *liveFake) full() error {
	if f.maxStreams > 0 && len(f.streams)+len(f.vads) >= f.maxStreams {
		return &EngineError{Code: CodeTooManyStreams, Message: "8 streams are open; end one first"}
	}
	return nil
}
func (f *liveFake) StartStream(session uint64, _, _ string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if err := f.full(); err != nil {
		return err
	}
	f.streams[session] = true
	f.opened = append(f.opened, session)
	return nil
}
func (f *liveFake) StartVad(session uint64, _ string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	if err := f.full(); err != nil {
		return err
	}
	f.vads[session] = NewBasicVad()
	f.opened = append(f.opened, session)
	return nil
}
func (f *liveFake) PushAudio(session uint64, samples []float32) error {
	f.mu.Lock()
	f.pushed[session] += len(samples)
	total := f.pushed[session]
	preview := f.streams[session]
	var parts []SpeechPart
	vad, isVad := f.vads[session]
	if isVad {
		parts = vad.Accept(samples)
	}
	f.mu.Unlock()
	if preview {
		f.emit(engine.Event{Kind: engine.EventPartial, Session: session, Tentative: fmt.Sprintf("%d ms", total*1000/SampleRate)})
	}
	f.emitParts(session, parts)
	if !preview && !isVad {
		return fmt.Errorf("session %d is not open", session)
	}
	return nil
}
func (f *liveFake) EndStream(session uint64) {
	f.mu.Lock()
	delete(f.streams, session)
	f.mu.Unlock()
}
func (f *liveFake) EndVad(session uint64) error {
	f.mu.Lock()
	vad, ok := f.vads[session]
	delete(f.vads, session)
	f.mu.Unlock()
	if !ok {
		return fmt.Errorf("not open")
	}
	if !f.vadKeepsOpenSpeech {
		f.emitParts(session, vad.Finish())
	}
	return nil
}
func (f *liveFake) TranscribeSegments(samples []float32, _, _ string) ([]Segment, error) {
	if f.failFinal != "" {
		return nil, &EngineError{Code: CodeModelNotDownloaded, Message: f.failFinal}
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	f.inFinal = true
	for f.hold {
		f.released.Wait()
	}
	f.inFinal = false
	f.transcribed++
	first, last := -1, -1
	loud := false
	for i, s := range samples {
		if math.Abs(float64(s)) > 0.02 {
			if first < 0 {
				first = i
			}
			last = i
		}
		if math.Abs(float64(s)) > 0.4 {
			loud = true
		}
	}
	if first < 0 {
		return nil, nil
	}
	text := fmt.Sprintf("part%d", f.transcribed)
	if f.texts != nil {
		text = f.texts[1]
		if loud {
			text = f.texts[0]
		}
	}
	return []Segment{{StartMs: uint64(first * 1000 / SampleRate), EndMs: uint64(last * 1000 / SampleRate), Text: text}}, nil
}
func (f *liveFake) Diarize(_ string, samples []float32, _ *uint32) ([]RawTurn, error) {
	f.mu.Lock()
	f.diarized = append(f.diarized, len(samples))
	f.mu.Unlock()
	return []RawTurn{{900, 3100, 1}, {3900, 5600, 0}}, nil
}
func (f *liveFake) Subscribe() (<-chan engine.Event, func()) { return f.events, func() {} }

// eventLog is a sink that keeps every event.
type eventLog struct {
	mu  sync.Mutex
	all []Event
}

func (l *eventLog) Event(e Event) {
	l.mu.Lock()
	l.all = append(l.all, e)
	l.mu.Unlock()
}

func (l *eventLog) events() []Event {
	l.mu.Lock()
	defer l.mu.Unlock()
	return append([]Event(nil), l.all...)
}

func (l *eventLog) segments() []SegmentEvent {
	var out []SegmentEvent
	for _, e := range l.events() {
		if s, ok := e.(SegmentEvent); ok {
			out = append(out, s)
		}
	}
	return out
}

func (l *eventLog) partials() []PartialEvent {
	var out []PartialEvent
	for _, e := range l.events() {
		if p, ok := e.(PartialEvent); ok {
			out = append(out, p)
		}
	}
	return out
}

func (l *eventLog) count(kind string) int {
	n := 0
	for _, e := range l.events() {
		if e.Kind() == kind {
			n++
		}
	}
	return n
}

func (l *eventLog) has(want Event) bool {
	for _, e := range l.events() {
		if reflect.DeepEqual(e, want) {
			return true
		}
	}
	return false
}

// fakeMic is a microphone whose frames the test sends.
type fakeMic struct {
	mu     sync.Mutex
	feed   Feed
	opened int
}

func (m *fakeMic) Open(feed Feed) (func(), error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.feed = feed
	m.opened++
	return func() { m.mu.Lock(); m.feed = nil; m.mu.Unlock() }, nil
}

func (m *fakeMic) current() Feed {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.feed
}

func (m *fakeMic) send(samples []float32) {
	feed := m.current()
	if feed == nil {
		panic("the microphone is not open")
	}
	feed.Audio(Captured{Samples: samples})
}

func (m *fakeMic) frames(samples []float32) {
	for i := 0; i < len(samples); i += 320 {
		m.send(samples[i:min(i+320, len(samples))])
	}
}

func (m *fakeMic) isOpen() bool { return m.current() != nil }

func (m *fakeMic) opens() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.opened
}

// fakeSystem is the system audio, which the test steers.
type fakeSystem struct {
	mu      sync.Mutex
	feed    Feed
	refuse  error
	appPID  *uint32
	hold    bool
	waiting bool
	panics  bool
	opened  int
}

func (s *fakeSystem) Open(appPID *uint32, feed Feed) (func(), []string, error) {
	s.mu.Lock()
	s.opened++
	for s.hold {
		s.waiting = true
		s.mu.Unlock()
		time.Sleep(time.Millisecond)
		s.mu.Lock()
	}
	defer s.mu.Unlock()
	if s.panics {
		panic("a panic in the capture")
	}
	if s.refuse != nil {
		return nil, nil, s.refuse
	}
	s.appPID = appPID
	s.feed = feed
	return func() { s.mu.Lock(); s.feed = nil; s.mu.Unlock() }, []string{"sayso", "app"}, nil
}

func (s *fakeSystem) set(f func(*fakeSystem)) {
	s.mu.Lock()
	f(s)
	s.mu.Unlock()
}

func (s *fakeSystem) current() Feed {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.feed
}

func (s *fakeSystem) audio(samples []float32) {
	for i := 0; i < len(samples); i += 320 {
		s.current().Audio(Captured{Samples: samples[i:min(i+320, len(samples))]})
	}
}

type liveFixture struct {
	eng      *liveFake
	mic      *fakeMic
	system   *fakeSystem
	gate     *DictationGate
	sessions *liveSessions
	temp     string
}

func newLiveFixture(t *testing.T, eng *liveFake) *liveFixture {
	t.Helper()
	b := startBus(eng)
	t.Cleanup(b.stop)
	f := &liveFixture{eng: eng, mic: &fakeMic{}, system: &fakeSystem{}, gate: NewDictationGate(), temp: t.TempDir()}
	f.sessions = newLiveSessions(LiveDeps{
		Engine: eng, Bus: b, Microphone: f.mic, SystemAudio: f.system, Gate: f.gate,
		Models: NewModelUse(), RecordingDir: filepath.Join(f.temp, "live"),
	}).(*liveSessions)
	return f
}

func testOwner(connection uint64) Owner {
	return Owner{Connection: connection, AppID: "dev.example.notetaker"}
}

func testSpec(sources ...Source) SessionSpec {
	return SessionSpec{Sources: sources, FinalModel: "final", PreviewModel: "streaming", Vad: VadRun{Basic: true}, Language: "en"}
}

func noPreview(sources ...Source) SessionSpec {
	s := testSpec(sources...)
	s.PreviewModel = ""
	return s
}

func silenceMs(ms int) []float32 { return make([]float32, ms*SampleRate/1000) }

// speechMs is something like speech, about -20 dB: a voice of 140 Hz with
// harmonics in syllables of 200 ms.
func speechMs(ms int) []float32 {
	out := make([]float32, ms*SampleRate/1000)
	for n := range out {
		t := float64(n) / SampleRate
		var voice float64
		for h := 1; h <= 5; h++ {
			voice += math.Sin(2*math.Pi*140*float64(h)*t) / float64(h)
		}
		syllable := t*5 - math.Floor(t*5)
		envelope := 0.05
		if syllable < 0.85 {
			envelope = math.Max(math.Sin(math.Pi*syllable/0.85), 0.3)
		}
		out[n] = float32(0.1 * voice * envelope)
	}
	return out
}

func loudMs(ms int) []float32 {
	out := speechMs(ms)
	for i := range out {
		out[i] *= 3
	}
	return out
}

func concat(parts ...[]float32) []float32 {
	var out []float32
	for _, p := range parts {
		out = append(out, p...)
	}
	return out
}

func toPCM(samples []float32) []int16 {
	out := make([]int16, len(samples))
	for i, s := range samples {
		out[i] = liveToI16(s)
	}
	return out
}

// sendClient sends the audio in parts of 160 ms, as an extension does.
func sendClient(t *testing.T, l *liveSessions, connection, session uint64, samples []int16) {
	t.Helper()
	step := SampleRate * 160 / 1000
	for i := 0; i < len(samples); i += step {
		if err := l.Audio(connection, session, EncodeI16(samples[i:min(i+step, len(samples))])); err != nil {
			t.Fatal(err)
		}
	}
}

func eventually(t *testing.T, what string, check func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !check() {
		if time.Now().After(deadline) {
			t.Fatal(what)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func near(ms, want, slack uint64) bool {
	if ms > want {
		return ms-want <= slack
	}
	return want-ms <= slack
}

func mustStart(t *testing.T, f *liveFixture, owner Owner, spec SessionSpec, sink EventSink) Started {
	t.Helper()
	s, err := f.sessions.Start(owner, spec, sink)
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func mustStop(t *testing.T, f *liveFixture, connection, session uint64) string {
	t.Helper()
	text, err := f.sessions.Stop(connection, session)
	if err != nil {
		t.Fatal(err)
	}
	return text
}

func code(err *LiveError) ErrorCode {
	if err == nil {
		return "<nil>"
	}
	return err.Code
}

func dirEmpty(t *testing.T, dir string) bool {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	return len(entries) == 0
}

func TestLiveClientSessionGivesPartialsSegmentsARecordingAndSpeakers(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	recordTo := filepath.Join(f.temp, "meeting")
	sink := &eventLog{}
	var obsMu sync.Mutex
	var observed []*LiveInfo
	f.sessions.SetObserver(func(i *LiveInfo) { obsMu.Lock(); observed = append(observed, i); obsMu.Unlock() })
	spec := testSpec(SourceClient)
	spec.Diarization, spec.RecordTo = "speaker-diarization", recordTo
	session := mustStart(t, f, testOwner(1), spec, sink).Session
	if info, ok := f.sessions.Current(); !ok || info.Session != session || info.AppID != "dev.example.notetaker" || info.CapturesMic {
		t.Fatal(info, ok)
	}

	audio := toPCM(concat(silenceMs(1000), speechMs(2000), silenceMs(1000), speechMs(1500), silenceMs(1000)))
	sendClient(t, f.sessions, 1, session, audio)
	eventually(t, "the first segment comes before the stop", func() bool { return len(sink.segments()) > 0 })
	// Partials pass through the event bus on their own goroutine, and a stop
	// does not wait for the last one; wait for it before stopping.
	eventually(t, "the preview stream got all the audio", func() bool {
		partials := sink.partials()
		return len(partials) > 0 && partials[len(partials)-1].Tentative == "6500 ms"
	})
	if text := mustStop(t, f, 1, session); text != "part1 part2" {
		t.Fatal(text)
	}

	partials := sink.partials()
	if len(partials) == 0 || partials[len(partials)-1].Tentative != "6500 ms" {
		t.Fatal("the preview stream got all the audio", len(partials))
	}
	for _, p := range partials {
		if p.Session != session || p.Source == nil || *p.Source != SourceClient {
			t.Fatal(p)
		}
	}
	segments := sink.segments()
	if len(segments) != 2 {
		t.Fatal(segments)
	}
	for _, s := range segments {
		if *s.Session != session || *s.Source != SourceClient || s.Job != nil {
			t.Fatal(s)
		}
	}
	// The times are from the start of the session, not from the start of the part.
	if !near(segments[0].StartMs, 1000, 60) || !near(segments[0].EndMs, 3000, 60) || !near(segments[1].StartMs, 4000, 60) || !near(segments[1].EndMs, 5500, 60) {
		t.Fatalf("%+v", segments)
	}
	// The recording has every sample that the extension sent.
	b, err := os.ReadFile(filepath.Join(recordTo, "client.wav"))
	if err != nil || len(b) != 44+len(audio)*2 {
		t.Fatal(len(b), err)
	}
	got, _ := readLiveWav(filepath.Join(recordTo, "client.wav"))
	if !reflect.DeepEqual(toPCM(got), audio) {
		t.Fatal("the recording differs")
	}
	if !reflect.DeepEqual(f.eng.diarized, []int{len(audio)}) {
		t.Fatal("diarization ran on the whole recording", f.eng.diarized)
	}
	// Speakers come after the last segment, with labels in the order of first speech.
	events := sink.events()
	at := -1
	for i, e := range events {
		if _, ok := e.(SpeakersEvent); ok {
			at = i
		}
	}
	if at < 0 {
		t.Fatal("no speakers", events)
	}
	for _, e := range events[at+1:] {
		if k := e.Kind(); k == "segment" || k == "partial" {
			t.Fatal("an event after the speakers", e)
		}
	}
	sp := events[at].(SpeakersEvent)
	if *sp.Session != session || *sp.Source != SourceClient || sp.Job != nil || len(sp.Turns) != 2 ||
		sp.Turns[0].StartMs != segments[0].StartMs || sp.Turns[0].Speaker != "1" || sp.Turns[1].StartMs != segments[1].StartMs || sp.Turns[1].Speaker != "2" {
		t.Fatalf("%+v", sp)
	}
	if f.eng.openNow() != 0 {
		t.Fatal("the engine sessions are closed")
	}
	if _, ok := f.sessions.Current(); ok {
		t.Fatal("the slot is free")
	}
	obsMu.Lock()
	if len(observed) < 2 || observed[0] == nil || observed[0].Session != session || observed[len(observed)-1] != nil {
		t.Fatal(observed)
	}
	obsMu.Unlock()
	if !reflect.DeepEqual(f.eng.loads, []string{"final", "streaming", "speaker-diarization"}) {
		t.Fatal(f.eng.loads)
	}
	if f.sessions.deps.Models.Held("final") {
		t.Fatal("the models are let go after the session")
	}
}

func TestLiveASecondSessionIsBusyUntilTheFirstEnds(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	first := mustStart(t, f, testOwner(1), testSpec(SourceClient), &eventLog{}).Session
	if !f.sessions.deps.Models.Held("final") {
		t.Fatal("a session holds its models")
	}
	_, err := f.sessions.Start(testOwner(2), testSpec(SourceClient), &eventLog{})
	if code(err) != CodeBusy || strings.Contains(err.Message, "Sayso") {
		t.Fatal(err)
	}
	if _, err := f.sessions.Stop(2, first); code(err) != CodeBadRequest {
		t.Fatal("another connection cannot stop it", err)
	}
	if err := f.sessions.Audio(2, first, ""); code(err) != CodeBadRequest {
		t.Fatal("another connection cannot send it audio", err)
	}
	if text := mustStop(t, f, 1, first); text != "" {
		t.Fatal(text)
	}
	second := mustStart(t, f, testOwner(2), testSpec(SourceClient), &eventLog{}).Session
	if second == first {
		t.Fatal("the same id")
	}
	if _, err := f.sessions.Stop(1, second); code(err) != CodeBadRequest {
		t.Fatal("the first connection does not own it")
	}
	mustStop(t, f, 2, second)
}

func TestLiveEngineSessionIDsAreAboveDictationAndUnique(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	spec := testSpec(SourceMic, SourceClient)
	spec.Vad = VadRun{Model: "silero-vad"}
	for i := 0; i < 2; i++ {
		session := mustStart(t, f, testOwner(1), spec, &eventLog{}).Session
		if f.eng.openNow() != 4 {
			t.Fatal("a preview stream and a VAD session for each source", f.eng.openNow())
		}
		mustStop(t, f, 1, session)
	}
	seen := map[uint64]bool{}
	for _, id := range f.eng.opened {
		if id < EngineSessionBase || seen[id] {
			t.Fatal(f.eng.opened)
		}
		seen[id] = true
	}
	if len(f.eng.opened) != 8 || f.eng.openNow() != 0 {
		t.Fatal(f.eng.opened)
	}
}

func TestLiveAFullEngineGivesTooManyStreamsAndClosesWhatOpened(t *testing.T) {
	eng := newLiveFake()
	eng.maxStreams = 3
	f := newLiveFixture(t, eng)
	spec := testSpec(SourceMic, SourceClient)
	spec.Vad = VadRun{Model: "silero-vad"}
	_, err := f.sessions.Start(testOwner(1), spec, &eventLog{})
	if code(err) != CodeTooManyStreams {
		t.Fatal(err)
	}
	if f.eng.openNow() != 0 || f.mic.isOpen() || !dirEmpty(t, filepath.Join(f.temp, "live")) {
		t.Fatal("the failed start leaves something open")
	}
	if _, ok := f.sessions.Current(); ok || f.sessions.deps.Models.Held("final") {
		t.Fatal("the slot or the models are held")
	}
}

func TestLiveSourcesAreChecked(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	for _, sources := range [][]Source{{}, {SourceClient, SourceClient}, {SourceSystem, SourceSystem}, {"camera"}} {
		if _, err := f.sessions.Start(testOwner(1), testSpec(sources...), &eventLog{}); code(err) != CodeBadRequest {
			t.Fatal(sources, err)
		}
	}
	relative := testSpec(SourceClient)
	relative.RecordTo = "notes"
	if _, err := f.sessions.Start(testOwner(1), relative, &eventLog{}); code(err) != CodeBadRequest {
		t.Fatal(err)
	}
	if _, ok := f.sessions.Current(); ok {
		t.Fatal("a refused start leaves the slot free")
	}
}

func TestLiveAMicSessionRecordsToATemporaryFileThatTheStopDeletes(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), testSpec(SourceMic), sink).Session
	if info, _ := f.sessions.Current(); !info.CapturesMic {
		t.Fatal(info)
	}
	f.mic.frames(concat(silenceMs(500), speechMs(1500), silenceMs(800)))
	entries, _ := os.ReadDir(filepath.Join(f.temp, "live"))
	if len(entries) != 1 || !strings.HasSuffix(entries[0].Name(), "mic.wav") {
		t.Fatal(entries)
	}
	recording := filepath.Join(f.temp, "live", entries[0].Name())
	if text := mustStop(t, f, 1, session); text != "part1" {
		t.Fatal(text)
	}
	if f.mic.isOpen() {
		t.Fatal("the stop gives the microphone back")
	}
	if _, err := os.Stat(recording); !os.IsNotExist(err) {
		t.Fatal("the temporary recording is deleted", err)
	}
	segments := sink.segments()
	if len(segments) != 1 || *segments[0].Source != SourceMic || !near(segments[0].StartMs, 500, 60) {
		t.Fatalf("%+v", segments)
	}
}

func TestLiveBasicVadCutsPartsAtPausesAndTheStopEndsTheOpenPart(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), noPreview(SourceClient), sink).Session
	// Speech, a pause of 300 ms that does not end it, a pause of 1 s that
	// does, and speech that still runs at the stop.
	sendClient(t, f.sessions, 1, session, toPCM(concat(silenceMs(500), speechMs(1000), silenceMs(300), speechMs(700), silenceMs(1000), speechMs(1200))))
	if text := mustStop(t, f, 1, session); text != "part1 part2" {
		t.Fatal(text)
	}
	s := sink.segments()
	if len(s) != 2 || !near(s[0].StartMs, 500, 60) || !near(s[0].EndMs, 2500, 60) || !near(s[1].StartMs, 3500, 60) || !near(s[1].EndMs, 4700, 60) {
		t.Fatalf("%+v", s)
	}
	if len(sink.partials()) != 0 {
		t.Fatal("no preview model, no partials")
	}
}

func TestLiveAFinalPassThatFailsGivesPartFailedAndTheSessionEndsNormally(t *testing.T) {
	eng := newLiveFake()
	eng.failFinal = "HTTP 401: the key is wrong"
	f := newLiveFixture(t, eng)
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), noPreview(SourceClient), sink).Session
	sendClient(t, f.sessions, 1, session, toPCM(concat(silenceMs(500), speechMs(1000), silenceMs(1000))))
	if text := mustStop(t, f, 1, session); text != "" {
		t.Fatal(text)
	}
	var failed []PartFailed
	for _, e := range sink.events() {
		if p, ok := e.(PartFailed); ok {
			failed = append(failed, p)
		}
	}
	if len(failed) != 1 {
		t.Fatal(failed)
	}
	p := failed[0]
	if p.Session != session || p.Source != SourceClient || p.Message != "HTTP 401: the key is wrong" || p.Code == nil || *p.Code != CodeModelNotDownloaded ||
		p.StartMs >= p.EndMs || !near(p.StartMs, 500, 300) || !near(p.EndMs, 1500, 600) {
		t.Fatalf("%+v", p)
	}
	if len(sink.segments()) != 0 {
		t.Fatal("no segment for a failed part")
	}
}

func TestLiveAnEngineVadGivesThePartsAndTheStopEndsSpeechThatIsOpen(t *testing.T) {
	eng := newLiveFake()
	eng.vadKeepsOpenSpeech = true
	f := newLiveFixture(t, eng)
	sink := &eventLog{}
	spec := testSpec(SourceClient)
	spec.Vad = VadRun{Model: "silero-vad"}
	session := mustStart(t, f, testOwner(1), spec, sink).Session
	sendClient(t, f.sessions, 1, session, toPCM(concat(silenceMs(500), speechMs(1000), silenceMs(1000), speechMs(800))))
	// The engine reports the start of the speech that is open, and never its end.
	var vad uint64
	eng.mu.Lock()
	for id := range eng.vads {
		vad = id
	}
	eng.mu.Unlock()
	eventually(t, "the first part", func() bool { return len(sink.segments()) > 0 })
	eng.emit(engine.Event{Kind: engine.EventSpeechStart, Session: vad, StartMs: 2500})
	time.Sleep(50 * time.Millisecond)
	if text := mustStop(t, f, 1, session); text != "part1 part2" {
		t.Fatal(text)
	}
	s := sink.segments()
	if len(s) != 2 || !near(s[0].StartMs, 500, 60) || !near(s[0].EndMs, 1500, 60) || !near(s[1].StartMs, 2500, 60) || !near(s[1].EndMs, 3300, 60) {
		t.Fatalf("the open speech goes to the end of the audio: %+v", s)
	}
	found := false
	for _, m := range eng.loads {
		found = found || m == "silero-vad"
	}
	if !found {
		t.Fatal(eng.loads)
	}
}

func TestLiveAStopWhileAPartIsTranscribedWaitsForItAndForTheLastPart(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), testSpec(SourceClient), sink).Session
	f.eng.holdFinalPass(true)
	sendClient(t, f.sessions, 1, session, toPCM(concat(silenceMs(300), speechMs(1000), silenceMs(1000))))
	eventually(t, "the final pass of the first part runs", func() bool { f.eng.mu.Lock(); defer f.eng.mu.Unlock(); return f.eng.inFinal })
	sendClient(t, f.sessions, 1, session, toPCM(speechMs(800)))
	type result struct {
		text string
		err  *LiveError
	}
	stopped := make(chan result, 1)
	go func() { text, err := f.sessions.Stop(1, session); stopped <- result{text, err} }()
	time.Sleep(200 * time.Millisecond)
	select {
	case <-stopped:
		t.Fatal("the stop waits for the final pass")
	default:
	}
	if info, _ := f.sessions.Current(); !info.Stopping {
		t.Fatal(info)
	}
	if err := f.sessions.Audio(1, session, ""); code(err) != CodeBadRequest {
		t.Fatal("no audio after the stop", err)
	}
	if _, err := f.sessions.Start(testOwner(2), testSpec(SourceClient), &eventLog{}); code(err) != CodeBusy {
		t.Fatal(err)
	}
	f.eng.holdFinalPass(false)
	r := <-stopped
	if r.err != nil || r.text != "part1 part2" || len(sink.segments()) != 2 {
		t.Fatal(r)
	}
	if _, ok := f.sessions.Current(); ok {
		t.Fatal("the slot is free")
	}
}

func TestLiveFinalPassesWaitWhileADictationRunsButThePreviewDoesNot(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), testSpec(SourceClient), sink).Session
	f.gate.Begin()
	sendClient(t, f.sessions, 1, session, toPCM(concat(speechMs(1000), silenceMs(1000))))
	eventually(t, "the preview goes on", func() bool { return len(sink.partials()) >= 12 })
	time.Sleep(100 * time.Millisecond)
	f.eng.mu.Lock()
	transcribed := f.eng.transcribed
	f.eng.mu.Unlock()
	if transcribed != 0 {
		t.Fatal("the part waits for the dictation")
	}
	f.gate.End()
	eventually(t, "the part runs after the dictation", func() bool { return len(sink.segments()) == 1 })
	mustStop(t, f, 1, session)
}

func TestLiveAClosedConnectionStopsItsSessionWithoutDiarization(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	spec := testSpec(SourceMic)
	spec.Diarization = "speaker-diarization"
	session := mustStart(t, f, testOwner(7), spec, sink).Session
	f.mic.send(speechMs(500))
	f.sessions.ConnectionClosed(3)
	if _, ok := f.sessions.Current(); !ok {
		t.Fatal("another connection does not stop it")
	}
	f.sessions.ConnectionClosed(7)
	eventually(t, "the session ends", func() bool { _, ok := f.sessions.Current(); return !ok })
	if f.mic.isOpen() || f.eng.openNow() != 0 || len(f.eng.diarized) != 0 || sink.count("speakers") != 0 {
		t.Fatal("the cancelled session left something")
	}
	eventually(t, "the temporary recording is deleted", func() bool { return dirEmpty(t, filepath.Join(f.temp, "live")) })
	if _, err := f.sessions.Stop(7, session); code(err) != CodeBadRequest {
		t.Fatal(err)
	}
}

func TestLiveAHostWithoutCapturesGetsUnsupported(t *testing.T) {
	eng := newLiveFake()
	b := startBus(eng)
	defer b.stop()
	l := newLiveSessions(LiveDeps{Engine: eng, Bus: b, Gate: NewDictationGate(), RecordingDir: t.TempDir()})
	for _, s := range []Source{SourceSystem, SourceMic} {
		_, err := l.Start(testOwner(1), testSpec(s), &eventLog{})
		if code(err) != CodeUnsupported || strings.Contains(err.Message, "Sayso") {
			t.Fatal(err)
		}
	}
}

const (
	theCall = "we should ship the release on friday if the tests pass"
	theUser = "i can take that one after lunch"
)

func echoFixture(t *testing.T) *liveFixture {
	eng := newLiveFake()
	eng.texts = &[2]string{theCall, theUser}
	return newLiveFixture(t, eng)
}

func TestLiveAMicrophoneSegmentThatTheSystemCoversIsDropped(t *testing.T) {
	f := echoFixture(t)
	sink := &eventLog{}
	started := mustStart(t, f, testOwner(1), noPreview(SourceMic, SourceSystem), sink)
	if !reflect.DeepEqual(started.Excludes, []string{"sayso", "app"}) {
		t.Fatal(started.Excludes)
	}
	// The call plays, and the microphone hears it.
	audio := concat(silenceMs(500), loudMs(2000), silenceMs(1000))
	f.system.audio(audio)
	f.mic.frames(audio)
	if text := mustStop(t, f, 1, started.Session); text != theCall {
		t.Fatal("one copy of the sentence:", text)
	}
	s := sink.segments()
	if len(s) != 1 || *s[0].Source != SourceSystem {
		t.Fatalf("%+v", s)
	}
}

func TestLiveTheOwnWordsOfTheUserStayWhileTheCallSpeaks(t *testing.T) {
	f := echoFixture(t)
	sink := &eventLog{}
	started := mustStart(t, f, testOwner(1), noPreview(SourceMic, SourceSystem), sink)
	f.system.audio(concat(silenceMs(500), loudMs(2000), silenceMs(1000)))
	f.mic.frames(concat(silenceMs(500), speechMs(2000), silenceMs(1000)))
	text := mustStop(t, f, 1, started.Session)
	if !strings.Contains(text, theCall) || !strings.Contains(text, theUser) || len(sink.segments()) != 2 {
		t.Fatal(text)
	}
}

func TestLiveAHeldSegmentComesWhileTheSessionRunsWhenTheSystemCatchesUp(t *testing.T) {
	f := echoFixture(t)
	sink := &eventLog{}
	started := mustStart(t, f, testOwner(1), noPreview(SourceMic, SourceSystem), sink)
	f.mic.frames(concat(silenceMs(200), speechMs(1000), silenceMs(1000)))
	eventually(t, "the final pass of the microphone ran", func() bool { f.eng.mu.Lock(); defer f.eng.mu.Unlock(); return f.eng.transcribed == 1 })
	// The system source hears silence, and its time goes past the segment.
	f.system.audio(silenceMs(4000))
	eventually(t, "the segment comes", func() bool {
		f.system.audio(silenceMs(20))
		return len(sink.segments()) > 0
	})
	if *sink.segments()[0].Source != SourceMic {
		t.Fatal(sink.segments())
	}
	if text := mustStop(t, f, 1, started.Session); text != theUser {
		t.Fatal(text)
	}
}

func TestLiveASessionWithOneCapturedSourceHasNoEchoRule(t *testing.T) {
	f := echoFixture(t)
	sink := &eventLog{}
	started := mustStart(t, f, testOwner(1), noPreview(SourceMic), sink)
	f.mic.frames(concat(silenceMs(200), speechMs(1000), silenceMs(1000)))
	eventually(t, "the segment comes at once", func() bool { return len(sink.segments()) > 0 })
	mustStop(t, f, 1, started.Session)
}

func TestLiveAGapGivesAnEventAndSilenceSoTheTimesGoOn(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	recordTo := filepath.Join(f.temp, "meeting")
	spec := noPreview(SourceSystem)
	spec.RecordTo = recordTo
	session := mustStart(t, f, testOwner(1), spec, sink).Session
	f.system.audio(silenceMs(500))
	f.system.current().Gap(1000)
	f.system.audio(concat(speechMs(1000), silenceMs(1000)))
	mustStop(t, f, 1, session)
	if !sink.has(SourceGap{Session: session, Source: SourceSystem, Ms: 1000}) {
		t.Fatal(sink.events())
	}
	if s := sink.segments(); len(s) == 0 || !near(s[0].StartMs, 1500, 60) {
		t.Fatalf("the speech comes after the gap: %+v", s)
	}
	if got, _ := readLiveWav(filepath.Join(recordTo, "system.wav")); len(got) != 3500*SampleRate/1000 {
		t.Fatal("the recording has the silence", len(got))
	}
}

func TestLiveAVeryLongGapDoesNotWriteAllOfItsSilence(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	recordTo := filepath.Join(f.temp, "meeting")
	spec := noPreview(SourceSystem)
	spec.RecordTo = recordTo
	session := mustStart(t, f, testOwner(1), spec, &eventLog{}).Session
	f.system.current().Gap(8 * 3_600_000)
	mustStop(t, f, 1, session)
	if got, _ := readLiveWav(filepath.Join(recordTo, "system.wav")); len(got) != 60*SampleRate {
		t.Fatal("at most a minute", len(got))
	}
}

func TestLiveASourceThatEndsTellsTheAppAfterItsLastSegmentAndTheOthersGoOn(t *testing.T) {
	f := echoFixture(t)
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), noPreview(SourceMic, SourceSystem), sink).Session
	f.system.audio(concat(silenceMs(200), loudMs(1000)))
	f.system.current().Ended(SourceEnd{Reason: EndPermission})
	ended := SourceEnded{Session: session, Source: SourceSystem, Reason: EndPermission}
	eventually(t, "the source ends", func() bool { return sink.has(ended) })
	sawSystem := false
	for _, e := range sink.events() {
		if reflect.DeepEqual(e, ended) {
			break
		}
		if s, ok := e.(SegmentEvent); ok && *s.Source == SourceSystem {
			sawSystem = true
		}
	}
	if !sawSystem {
		t.Fatal("the last segment comes first", sink.events())
	}
	// The microphone goes on, and its segments no longer wait.
	if _, ok := f.sessions.Current(); !ok {
		t.Fatal("the session goes on")
	}
	f.mic.frames(concat(silenceMs(10_000), speechMs(1000), silenceMs(1000)))
	eventually(t, "the microphone segment comes", func() bool {
		for _, s := range sink.segments() {
			if *s.Source == SourceMic {
				return true
			}
		}
		return false
	})
	text := mustStop(t, f, 1, session)
	if !strings.Contains(text, theCall) || !strings.Contains(text, theUser) {
		t.Fatal(text)
	}
}

func TestLiveACaptureThatFailsForGoodEndsWithTheReasonFailed(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), noPreview(SourceSystem), sink).Session
	f.system.current().Ended(SourceEnd{Reason: EndFailed, Message: "the sound server is gone"})
	eventually(t, "the source ends", func() bool {
		return sink.has(SourceEnded{Session: session, Source: SourceSystem, Reason: EndFailed})
	})
	mustStop(t, f, 1, session)
}

func TestLiveARefusedOrUnsupportedCaptureStopsTheStartAndClosesWhatOpened(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	for _, c := range []struct {
		err  error
		code ErrorCode
	}{
		{&OpenError{Kind: OpenPermission, Message: "refused"}, CodePermission},
		{&OpenError{Kind: OpenUnsupported, Message: "macOS 14.1"}, CodeUnsupported},
		{&OpenError{Kind: OpenFailed, Message: "no device"}, ""},
		{fmt.Errorf("no device"), ""},
	} {
		f.system.set(func(s *fakeSystem) { s.refuse = c.err })
		_, err := f.sessions.Start(testOwner(1), testSpec(SourceMic, SourceSystem), &eventLog{})
		if err == nil || err.Code != c.code || !strings.Contains(err.Message, "the source system did not start") {
			t.Fatal(c, err)
		}
		if f.eng.openNow() != 0 || f.mic.isOpen() || f.mic.opens() != 0 {
			t.Fatal("the microphone is given back")
		}
		if _, ok := f.sessions.Current(); ok {
			t.Fatal("the slot is free")
		}
	}
}

func TestLiveThePidOfTheAppGoesToTheCapture(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	pid := uint32(4242)
	session := mustStart(t, f, Owner{Connection: 1, AppID: "app", PID: &pid}, noPreview(SourceSystem), &eventLog{}).Session
	if f.system.appPID == nil || *f.system.appPID != 4242 {
		t.Fatal(f.system.appPID)
	}
	mustStop(t, f, 1, session)
}

func TestLiveASourceThatStartsLateGetsSilenceSoTheSourcesShareTheirTimes(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	recordTo := filepath.Join(f.temp, "meeting")
	spec := noPreview(SourceMic, SourceSystem)
	spec.RecordTo = recordTo
	session := mustStart(t, f, testOwner(1), spec, sink).Session
	at := func(ms int64, samples []float32) Captured {
		return Captured{Samples: samples, Time: time.Duration(5_000+ms) * time.Millisecond, Timed: true}
	}
	f.mic.current().Audio(at(0, silenceMs(1000)))
	// The source handles its items in order, so this event says that it has the first audio.
	f.mic.current().Gap(0)
	eventually(t, "the first audio of the microphone arrives", func() bool { return sink.count("source_gap") > 0 })
	// The first audio of the system comes 300 ms after the first of the microphone.
	f.system.current().Audio(at(300, silenceMs(700)))
	mustStop(t, f, 1, session)
	if got, _ := readLiveWav(filepath.Join(recordTo, "mic.wav")); len(got) != SampleRate {
		t.Fatal(len(got))
	}
	if got, _ := readLiveWav(filepath.Join(recordTo, "system.wav")); len(got) != SampleRate {
		t.Fatal("300 ms of silence and 700 ms of audio", len(got))
	}
}

// startThatWaits starts a session that waits in the capture of the system audio.
func startThatWaits(t *testing.T, f *liveFixture, connection uint64, spec SessionSpec) chan *LiveError {
	t.Helper()
	f.system.set(func(s *fakeSystem) { s.hold, s.waiting = true, false })
	done := make(chan *LiveError, 1)
	started := make(chan Started, 1)
	go func() {
		s, err := f.sessions.Start(testOwner(connection), spec, &eventLog{})
		if err == nil {
			started <- s
		}
		done <- err
	}()
	eventually(t, "the capture waits", func() bool { f.system.mu.Lock(); defer f.system.mu.Unlock(); return f.system.waiting })
	t.Cleanup(func() {
		select {
		case s := <-started:
			f.sessions.Stop(connection, s.Session)
		default:
		}
	})
	return done
}

func TestLiveAStartWhoseConnectionClosesEndsWhenTheQuestionEndsAndFreesTheSlot(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	var obsMu sync.Mutex
	observed := 0
	f.sessions.SetObserver(func(*LiveInfo) { obsMu.Lock(); observed++; obsMu.Unlock() })
	done := startThatWaits(t, f, 1, testSpec(SourceMic, SourceSystem))
	f.sessions.ConnectionClosed(2)
	f.sessions.ConnectionClosed(1)
	if _, err := f.sessions.Start(testOwner(3), testSpec(SourceClient), &eventLog{}); code(err) != CodeBusy {
		t.Fatal("the slot stays taken while the question is open", err)
	}
	f.system.set(func(s *fakeSystem) { s.hold = false })
	err := <-done
	if err == nil || !strings.Contains(err.Message, "closed") {
		t.Fatal(err)
	}
	if f.system.current() != nil || f.mic.opens() != 0 || f.eng.openNow() != 0 {
		t.Fatal("the start left something open")
	}
	if _, ok := f.sessions.Current(); ok {
		t.Fatal("the slot is free")
	}
	obsMu.Lock()
	if observed != 0 {
		t.Fatal("no recording state for a session that did not start")
	}
	obsMu.Unlock()
	next := mustStart(t, f, testOwner(3), testSpec(SourceClient), &eventLog{})
	mustStop(t, f, 3, next.Session)
}

func TestLiveACancelOfAnotherConnectionDoesNotEndAStart(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	done := startThatWaits(t, f, 1, testSpec(SourceSystem))
	f.sessions.ConnectionClosed(2)
	f.system.set(func(s *fakeSystem) { s.hold = false })
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestLiveACloseDuringAStartEndsItAndNoSessionStartsAfterIt(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	done := startThatWaits(t, f, 1, testSpec(SourceSystem))
	f.sessions.Close()
	f.system.set(func(s *fakeSystem) { s.hold = false })
	if err := <-done; err == nil {
		t.Fatal("the start goes on")
	}
	if f.system.current() != nil {
		t.Fatal("the capture runs")
	}
	if _, ok := f.sessions.Current(); ok {
		t.Fatal("a session runs")
	}
	_, err := f.sessions.Start(testOwner(1), testSpec(SourceClient), &eventLog{})
	if err == nil || strings.Contains(err.Message, "Sayso") {
		t.Fatal("the host closes", err)
	}
}

func TestLiveTheMicrophoneWaitsForTheQuestionOfTheSystem(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	done := startThatWaits(t, f, 1, testSpec(SourceMic, SourceSystem))
	if f.mic.opens() != 0 {
		t.Fatal("no microphone while the user answers")
	}
	f.system.set(func(s *fakeSystem) { s.hold = false })
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if info, ok := f.sessions.Current(); !ok || !info.CapturesMic || f.mic.opens() != 1 {
		t.Fatal(info)
	}
	f.mic.send(silenceMs(100))
}

func TestLiveARefusedSystemAudioOpensNoMicrophoneAndLeavesNoRecording(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	folder := filepath.Join(f.temp, "recordings")
	f.system.set(func(s *fakeSystem) { s.refuse = &OpenError{Kind: OpenPermission, Message: "refused"} })
	spec := testSpec(SourceMic, SourceSystem)
	spec.RecordTo = folder
	if _, err := f.sessions.Start(testOwner(1), spec, &eventLog{}); code(err) != CodePermission {
		t.Fatal(err)
	}
	if f.mic.opens() != 0 || f.eng.openNow() != 0 || !dirEmpty(t, folder) {
		t.Fatal("no WAV stays")
	}
}

func TestLiveAStartThatFailsLateDeletesTheRecordingsThatItMade(t *testing.T) {
	eng := newLiveFake()
	eng.maxStreams = 1
	f := newLiveFixture(t, eng)
	folder := filepath.Join(f.temp, "recordings")
	spec := testSpec(SourceClient, SourceMic)
	spec.RecordTo = folder
	if _, err := f.sessions.Start(testOwner(1), spec, &eventLog{}); code(err) != CodeTooManyStreams {
		t.Fatal(err)
	}
	if !dirEmpty(t, folder) || f.eng.openNow() != 0 {
		t.Fatal("no WAV stays")
	}
}

func TestLiveAPanicInAStartFreesTheSlot(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	f.system.set(func(s *fakeSystem) { s.panics = true })
	func() {
		defer func() {
			if recover() == nil {
				t.Fatal("no panic")
			}
		}()
		f.sessions.Start(testOwner(1), testSpec(SourceSystem), &eventLog{})
	}()
	f.system.set(func(s *fakeSystem) { s.panics = false })
	started := mustStart(t, f, testOwner(1), testSpec(SourceClient), &eventLog{})
	mustStop(t, f, 1, started.Session)
}

func TestLiveTheSilenceLimitStopsTheAudioWithOneEventAndTheStopGivesTheText(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	spec := noPreview(SourceMic)
	spec.SilenceLimit = 2 * time.Second
	session := mustStart(t, f, testOwner(1), spec, sink).Session
	// Speech of 1 s, then a pause under the limit, then more speech: the clock starts again.
	for _, s := range [][]float32{silenceMs(500), speechMs(1000), silenceMs(1500), speechMs(800), silenceMs(1500)} {
		f.mic.send(s)
	}
	time.Sleep(50 * time.Millisecond)
	if !f.mic.isOpen() || sink.count("silence_limit") != 0 {
		t.Fatal("under the limit")
	}
	f.mic.send(silenceMs(1500))
	eventually(t, "the limit passes", func() bool { return sink.count("silence_limit") == 1 })
	if f.mic.isOpen() {
		t.Fatal("the limit gives the microphone back")
	}
	if !sink.has(SilenceLimit{Session: session, SilenceMs: 2000}) {
		t.Fatal(sink.events())
	}
	// The session ends as usual, and the last part still has its text.
	if text := mustStop(t, f, 1, session); text != "part1 part2" {
		t.Fatal(text)
	}
	if sink.count("silence_limit") != 1 {
		t.Fatal("no second event at the stop")
	}
}

func TestLiveTheSilenceLimitCountsFromTheStartWhenNobodySpeaks(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	spec := testSpec(SourceClient)
	spec.SilenceLimit = time.Second
	session := mustStart(t, f, testOwner(1), spec, sink).Session
	sendClient(t, f.sessions, 1, session, toPCM(silenceMs(1200)))
	eventually(t, "the limit passes", func() bool { return sink.count("silence_limit") == 1 })
	if text := mustStop(t, f, 1, session); text != "" {
		t.Fatal(text)
	}
	// A session with no limit has no event.
	sink = &eventLog{}
	session = mustStart(t, f, testOwner(1), testSpec(SourceClient), sink).Session
	sendClient(t, f.sessions, 1, session, toPCM(silenceMs(3000)))
	mustStop(t, f, 1, session)
	if sink.count("silence_limit") != 0 {
		t.Fatal(sink.events())
	}
}

func TestLiveAnEngineVadStartsTheSilenceClockAtTheEndOfTheSpeech(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	spec := testSpec(SourceClient)
	spec.Vad = VadRun{Model: "silero-vad"}
	spec.SilenceLimit = 4 * time.Second
	session := mustStart(t, f, testOwner(1), spec, sink).Session
	sendClient(t, f.sessions, 1, session, toPCM(speechMs(2000)))
	sendClient(t, f.sessions, 1, session, toPCM(silenceMs(1000)))
	eventually(t, "the part ends", func() bool { return len(sink.segments()) > 0 })
	// 3 s of audio so far. The speech ended at about 2 s, so 3.5 s more pass
	// the limit, and 1.5 s more do not.
	sendClient(t, f.sessions, 1, session, toPCM(silenceMs(1500)))
	time.Sleep(50 * time.Millisecond)
	if sink.count("silence_limit") != 0 {
		t.Fatal("the clock starts at the end of the speech")
	}
	// The limit can end the source of the client during this audio, and then a send gets an error.
	pcm := toPCM(silenceMs(1800))
	for i := 0; i < len(pcm); i += SampleRate * 160 / 1000 {
		f.sessions.Audio(1, session, EncodeI16(pcm[i:min(i+SampleRate*160/1000, len(pcm))]))
	}
	eventually(t, "the limit passes", func() bool { return sink.count("silence_limit") == 1 })
	mustStop(t, f, 1, session)
}

func TestLiveStopForUserEndsWithSessionEnded(t *testing.T) {
	f := newLiveFixture(t, newLiveFake())
	sink := &eventLog{}
	session := mustStart(t, f, testOwner(1), noPreview(SourceClient), sink).Session
	sendClient(t, f.sessions, 1, session, toPCM(concat(silenceMs(300), speechMs(1000), silenceMs(1000))))
	if !f.sessions.StopForUser(session) {
		t.Fatal("it stops")
	}
	if f.sessions.StopForUser(session) {
		t.Fatal("it stops once")
	}
	eventually(t, "session_ended", func() bool { return sink.has(SessionEnded{Session: session, Text: "part1"}) })
	events := sink.events()
	if events[len(events)-1].Kind() != "session_ended" {
		t.Fatal("session_ended comes last", events)
	}
	eventually(t, "the slot is free", func() bool { _, ok := f.sessions.Current(); return !ok })
}
