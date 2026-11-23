package dictation

import (
	"context"
	"errors"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/store"
)

type fakeAudio struct {
	mu      sync.Mutex
	onChunk func(platform.AudioChunk)
	fail    bool
	stops   int
}

func (a *fakeAudio) ListDevices() ([]platform.AudioDevice, error) { return nil, nil }
func (a *fakeAudio) Start(_ context.Context, _ string, f func(platform.AudioChunk)) error {
	if a.fail {
		return errors.New("no microphone")
	}
	a.mu.Lock()
	a.onChunk = f
	a.mu.Unlock()
	return nil
}
func (a *fakeAudio) Stop() error { a.mu.Lock(); a.stops++; a.onChunk = nil; a.mu.Unlock(); return nil }
func (a *fakeAudio) speak(n int) {
	a.mu.Lock()
	f := a.onChunk
	a.mu.Unlock()
	if f != nil {
		f(platform.AudioChunk{Samples: make([]float32, n), RMS: 0.2})
	}
}

type fakeInserter struct {
	mu    sync.Mutex
	texts []string
	fail  bool
}

func (i *fakeInserter) Insert(text string, _ platform.InsertOptions) platform.InsertResult {
	i.mu.Lock()
	defer i.mu.Unlock()
	if i.fail {
		return platform.InsertResult{Method: "failed", Err: errors.New("No text field is focused")}
	}
	i.texts = append(i.texts, text)
	return platform.InsertResult{Method: "pasted", ClipboardRestored: true, Backend: "fake"}
}
func (i *fakeInserter) ReadClipboard() (string, error) { return "", nil }
func (i *fakeInserter) WriteClipboard(string) error    { return nil }

type fakeSounds struct {
	mu   sync.Mutex
	cues []platform.SoundCue
}

func (s *fakeSounds) Play(c platform.SoundCue, _ []byte, _ float32) error {
	s.mu.Lock()
	s.cues = append(s.cues, c)
	s.mu.Unlock()
	return nil
}

type fakeMedia struct {
	mu              sync.Mutex
	pauses, resumes int
	// hold, when set, keeps Pause and Resume until it closes: a slow player.
	hold chan struct{}
}

func (m *fakeMedia) Pause() (bool, error) {
	m.wait()
	m.mu.Lock()
	m.pauses++
	m.mu.Unlock()
	return true, nil
}

func (m *fakeMedia) Resume() error {
	m.wait()
	m.mu.Lock()
	m.resumes++
	m.mu.Unlock()
	return nil
}

func (m *fakeMedia) wait() {
	if m.hold != nil {
		<-m.hold
	}
}

// waitCounts waits until the player was paused and resumed so many times.
func (m *fakeMedia) waitCounts(t *testing.T, pauses, resumes int) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for {
		m.mu.Lock()
		p, r := m.pauses, m.resumes
		m.mu.Unlock()
		if p == pauses && r == resumes {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("media paused %d and resumed %d times, want %d and %d", p, r, pauses, resumes)
		}
		time.Sleep(5 * time.Millisecond)
	}
}

type fakeHotkeys struct{ recording []bool }

func (h *fakeHotkeys) Configure(platform.HotkeyConfig) error  { return nil }
func (h *fakeHotkeys) SetRecording(r bool)                    { h.recording = append(h.recording, r) }
func (h *fakeHotkeys) Start(func(platform.HotkeyEvent)) error { return nil }
func (h *fakeHotkeys) Stop() error                            { return nil }

type fakeContext struct{}

func (fakeContext) ActiveApp() (platform.ActiveApp, error) {
	return platform.ActiveApp{BundleID: "com.example.notes", Name: "Notes", Site: "example.com"}, nil
}

type fakeEngine struct {
	mu   sync.Mutex
	text string
	err  error
}

func (e *fakeEngine) Status(string) engine.ModelStatus {
	return engine.ModelStatus{State: engine.StateReady}
}
func (e *fakeEngine) WaitReady(string, time.Time) engine.ModelStatus {
	return engine.ModelStatus{State: engine.StateReady}
}
func (e *fakeEngine) Load(string) error { return nil }
func (e *fakeEngine) Transcribe(s []float32, o SessionOptions) (Transcript, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	return Transcript{Text: e.text, Model: o.FinalModel, ElapsedMs: 12}, e.err
}
func (e *fakeEngine) PreviewReady(string) bool                 { return false }
func (e *fakeEngine) StartStream(uint64, SessionOptions) error { return nil }
func (e *fakeEngine) PushAudio(uint64, []float32) error        { return nil }
func (e *fakeEngine) EndStream(uint64)                         {}

type fakeHistory struct {
	mu      sync.Mutex
	entries []store.HistoryEntry
	audio   int
	saved   chan struct{}
	// gate, when set, holds InsertEntry until it is closed: a slow disk.
	gate chan struct{}
}

func (h *fakeHistory) count() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return len(h.entries)
}

func (h *fakeHistory) InsertEntry(e store.HistoryEntry) (int64, error) {
	if h.gate != nil {
		<-h.gate
	}
	h.mu.Lock()
	h.entries = append(h.entries, e)
	h.mu.Unlock()
	return 1, nil
}
func (h *fakeHistory) SaveAudio([]float32) (string, error) {
	h.mu.Lock()
	h.audio++
	h.mu.Unlock()
	return "a.flac", nil
}
func (h *fakeHistory) RecordReplacementUses([]store.AppliedReplacement) error {
	if h.saved != nil {
		h.saved <- struct{}{}
	}
	return nil
}

type fakeHost struct {
	mu    sync.Mutex
	done  []bridge.DictationDone
	gate  *bridge.DictationGate
	gates atomic.Int64 // calls of DictationGate
}

func (h *fakeHost) DictationDone(d bridge.DictationDone, a, b bool) {
	if a || b {
		return
	}
	h.mu.Lock()
	h.done = append(h.done, d)
	h.mu.Unlock()
}
func (h *fakeHost) DictationGate() *bridge.DictationGate {
	h.gates.Add(1)
	return h.gate
}

type states struct {
	mu      sync.Mutex
	seen    []State
	notices []Message
	// history gets a value for each HistoryChanged, with the number of
	// entries the history had then.
	history chan int
	h       *fakeHistory
}

func (s *states) StateChanged(st State)  { s.mu.Lock(); s.seen = append(s.seen, st); s.mu.Unlock() }
func (s *states) Notice(m Message)       { s.mu.Lock(); s.notices = append(s.notices, m); s.mu.Unlock() }
func (s *states) Level(float32)          {}
func (s *states) Preview(string, string) {}
func (s *states) HistoryChanged() {
	if s.history != nil {
		select {
		case s.history <- s.h.count():
		default: // a test that does not read them
		}
	}
}

func (s *states) waitFor(t *testing.T, phase string) State {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		s.mu.Lock()
		for _, st := range s.seen {
			if st.Phase() == phase {
				s.mu.Unlock()
				return st
			}
		}
		s.mu.Unlock()
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("no %s state: %+v %+v", phase, s.seen, s.notices)
	return State{}
}

type rig struct {
	c       *Controller
	audio   *fakeAudio
	ins     *fakeInserter
	sounds  *fakeSounds
	media   *fakeMedia
	hot     *fakeHotkeys
	eng     *fakeEngine
	history *fakeHistory
	host    *fakeHost
	obs     *states
	cfg     *config.Config
	incog   bool
}

func localModel(t *testing.T) models.ModelID {
	for _, m := range models.LocalModels() {
		if m.FinalPass && m.Task() == models.TaskSpeech {
			return m.ID
		}
	}
	t.Skip("no local model")
	return ""
}

func newRig(t *testing.T, edit func(*config.Config)) *rig {
	cfg := config.Default()
	cfg.Dictation.Model = localModel(t)
	cfg.Dictation.SpokenPunctuation = true
	cfg.Audio.PauseMedia = true
	cfg.Sounds.Enabled = true
	if edit != nil {
		edit(&cfg)
	}
	r := &rig{audio: &fakeAudio{}, ins: &fakeInserter{}, sounds: &fakeSounds{}, media: &fakeMedia{}, hot: &fakeHotkeys{}, eng: &fakeEngine{text: " hello world period "}, history: &fakeHistory{saved: make(chan struct{}, 4)}, host: &fakeHost{gate: bridge.NewDictationGate()}, obs: &states{}, cfg: &cfg}
	r.obs.h, r.obs.history = r.history, make(chan int, 4)
	timing := DefaultTiming()
	timing.MinDurationMs = 0
	r.c = New(Deps{
		Platform: platform.Services{Audio: r.audio, Inserter: r.ins, Sounds: r.sounds, Media: r.media, Hotkeys: r.hot, Context: fakeContext{}},
		Engine:   r.eng,
		History:  r.history,
		Host:     r.host,
		Config:   func() *config.Config { return r.cfg },
		Replacer: func() *enhance.Replacer {
			return enhance.NewReplacer([]store.Replacement{{ID: 1, From: "world", To: "World"}})
		},
		Incognito:  func() bool { return r.incog },
		Observer:   r.obs,
		Timing:     &timing,
		AppVersion: "test",
		TickEvery:  10 * time.Millisecond,
	})
	t.Cleanup(r.c.Close)
	return r
}

// The history entry is written on its own goroutine, which can end after
// the dictation's last state: the views that read the history for that
// state saw no entry. The observer hears HistoryChanged once the entry is
// written, never before (Home and History stayed stale, issue 3).
func TestHistoryChangedComesAfterTheWrite(t *testing.T) {
	r := newRig(t, nil)
	r.history.gate = make(chan struct{})
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.audio.speak(16000)
	r.c.Toggle()
	r.obs.waitFor(t, "done")
	select {
	case n := <-r.obs.history:
		t.Fatalf("HistoryChanged before the write, with %d entries", n)
	default:
	}
	if r.history.count() != 0 {
		t.Fatal("the entry was written before the gate opened")
	}
	close(r.history.gate)
	select {
	case n := <-r.obs.history:
		if n != 1 {
			t.Fatalf("HistoryChanged with %d entries, want the new one", n)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("no HistoryChanged after the write")
	}
}

func TestADictationGoesInAndToHistory(t *testing.T) {
	r := newRig(t, nil)
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	if !r.host.gate.Dictating() {
		t.Fatal("jobs wait during a dictation")
	}
	r.audio.speak(16000)
	r.c.Toggle()
	r.obs.waitFor(t, "done")
	<-r.history.saved
	r.ins.mu.Lock()
	inserted := strings.Join(r.ins.texts, "|")
	r.ins.mu.Unlock()
	if inserted != "hello World. " {
		t.Fatalf("%q", inserted)
	}
	r.history.mu.Lock()
	e := r.history.entries[0]
	r.history.mu.Unlock()
	if e.Transcript != " hello world period " || e.FinalText != "hello World." || e.App == nil || e.App.BundleID != "com.example.notes" || e.App.Site != "example.com" || e.Insert.Result != store.InsertPasted || e.AudioFile != "a.flac" || len(e.Waveform) != 96 || e.Diagnostics == nil || e.Diagnostics.AppVersion != "test" || len(e.Replacements) != 1 || e.Enhance.Result != store.EnhanceNotUsed {
		t.Fatalf("%+v", e)
	}
	r.host.mu.Lock()
	if len(r.host.done) != 1 || r.host.done[0].Text != "hello World." || *r.host.done[0].App != "Notes" {
		t.Fatalf("%+v", r.host.done)
	}
	r.host.mu.Unlock()
	r.media.waitCounts(t, 1, 1)
	if len(r.hot.recording) != 2 || !r.hot.recording[0] || r.hot.recording[1] {
		t.Fatal(r.hot.recording)
	}
	if r.host.gate.Dictating() {
		t.Fatal("the gate is still held")
	}
	time.Sleep(20 * time.Millisecond)
	r.sounds.mu.Lock()
	cues := r.sounds.cues
	r.sounds.mu.Unlock()
	if len(cues) != 2 || cues[0] != platform.CueStart || cues[1] != platform.CueStop {
		t.Fatal(cues)
	}
	if r.c.LastText() != "hello World." {
		t.Fatal(r.c.LastText())
	}
}

func TestIncognitoAndHistoryOffLeaveNoTrace(t *testing.T) {
	r := newRig(t, nil)
	r.incog = true
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.audio.speak(1600)
	r.c.Toggle()
	r.obs.waitFor(t, "done")
	time.Sleep(50 * time.Millisecond)
	r.history.mu.Lock()
	n := len(r.history.entries)
	r.history.mu.Unlock()
	if n != 0 || len(r.host.done) != 0 {
		t.Fatal("a trace", n, r.host.done)
	}
	if SavesHistory(false, false, false) || SavesHistory(true, true, false) || SavesHistory(true, false, true) || !SavesHistory(true, false, false) {
		t.Fatal("rule")
	}
}

func TestAFailedInsertStaysAndIsSaved(t *testing.T) {
	r := newRig(t, nil)
	r.ins.fail = true
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.c.Toggle()
	st := r.obs.waitFor(t, "failed")
	if st.Notice.Kind != NoticeInsertFailed || st.Notice.Text != "hello World." || !st.Stays {
		t.Fatalf("%+v", st)
	}
	<-r.history.saved
	r.history.mu.Lock()
	defer r.history.mu.Unlock()
	if r.history.entries[0].Insert.Result != store.InsertFailed {
		t.Fatal(r.history.entries[0].Insert)
	}
}

func TestAMicrophoneThatFailsShowsAnError(t *testing.T) {
	r := newRig(t, nil)
	r.audio.fail = true
	r.c.Toggle()
	st := r.obs.waitFor(t, "failed")
	if st.Notice.Kind != NoticeError || st.Notice.Message != "overlay.notice.microphone_failed" {
		t.Fatalf("%+v", st)
	}
}

func TestATranscribeFailureIsSavedForARetry(t *testing.T) {
	r := newRig(t, nil)
	r.eng.err = errors.New("engine stopped")
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.c.Toggle()
	st := r.obs.waitFor(t, "failed")
	if st.Notice.Message != "engine stopped" {
		t.Fatal(st)
	}
	<-r.history.saved
}

func TestSilenceShowsNoSpeech(t *testing.T) {
	r := newRig(t, nil)
	r.eng.text = "   "
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.c.Toggle()
	if st := r.obs.waitFor(t, "done"); st.Notice.Kind != NoticeNoSpeech {
		t.Fatal(st)
	}
}

type upper struct{}

func (upper) Run(j StyleJob) enhance.PipelineOutput {
	return enhance.PipelineOutput{Text: strings.ToUpper(j.Text), Enhance: store.EnhanceOutcome{Result: store.EnhanceApplied, ProviderID: j.Provider.ID, Model: "m"}}
}

func TestAStyleRunsWithItsProvider(t *testing.T) {
	r := newRig(t, func(c *config.Config) {
		c.AI.Enabled = true
		model := "gpt-x"
		id := "p"
		c.AI.Providers = []config.Provider{{ID: "p", Name: "P", Kind: config.KindOpenAICompatible, Model: &model}}
		c.AI.DefaultProvider = &id
	})
	lib := enhance.LoadStyles(filepath.Join(t.TempDir(), "styles"))
	r.c.d.Styles = func() *enhance.StyleLibrary { return lib }
	r.c.d.Styler = upper{}
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.c.Toggle()
	st := r.obs.waitFor(t, "done")
	if st.Notice.Kind != NoticeInserted || st.Notice.StyleName == "" {
		t.Fatalf("%+v", st)
	}
	r.ins.mu.Lock()
	defer r.ins.mu.Unlock()
	if r.ins.texts[0] != "HELLO WORLD. " {
		t.Fatal(r.ins.texts)
	}
}

// waiting is a Styler that runs until its context ends.
type waiting struct{ started, ended chan struct{} }

func (w waiting) Run(j StyleJob) enhance.PipelineOutput {
	close(w.started)
	<-j.Context.Done()
	close(w.ended)
	return enhance.PipelineOutput{Text: j.Text}
}

// Cancel keeps the style step for Undo; when the undo window ends (here: a
// new dictation starts), its context ends, so a local cleanup model stops.
func TestADiscardedDictationEndsItsStyleStep(t *testing.T) {
	r := newRig(t, func(c *config.Config) {
		c.AI.Enabled = true
		model := "gpt-x"
		id := "p"
		c.AI.Providers = []config.Provider{{ID: "p", Name: "P", Kind: config.KindOpenAICompatible, Model: &model}}
		c.AI.DefaultProvider = &id
	})
	lib := enhance.LoadStyles(filepath.Join(t.TempDir(), "styles"))
	r.c.d.Styles = func() *enhance.StyleLibrary { return lib }
	w := waiting{started: make(chan struct{}), ended: make(chan struct{})}
	r.c.d.Styler = w
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.c.Toggle()
	select {
	case <-w.started:
	case <-time.After(5 * time.Second):
		t.Fatal("the style step did not start")
	}
	r.c.Cancel()
	select {
	case <-w.ended:
		t.Fatal("Cancel ended the style step; Undo needs it")
	case <-time.After(100 * time.Millisecond):
	}
	r.c.Toggle()
	select {
	case <-w.ended:
	case <-time.After(2 * time.Second):
		t.Fatal("the style step did not end when the dictation was discarded")
	}
}

// preparing is a Styler that hears Prepare.
type preparing struct {
	upper
	got chan config.Provider
}

func (p preparing) Prepare(_ *enhance.Style, provider config.Provider) { p.got <- provider }

// A provider that runs a program starts it while the user speaks, and gets
// the CLI timeout.
func TestAClaudeProviderIsPreparedWhenTheMicrophoneOpens(t *testing.T) {
	r := newRig(t, func(c *config.Config) {
		c.AI.Enabled = true
		id, model, path := "claude-code", "sonnet", "/nowhere/claude"
		c.AI.Providers = []config.Provider{{ID: id, Name: "Claude Code", Kind: config.KindClaudeCLI, Model: &model, Path: &path}}
		c.AI.DefaultProvider = &id
		c.AI.ActiveStyle = "rambling"
	})
	lib := enhance.LoadStyles(filepath.Join(t.TempDir(), "styles"))
	r.c.d.Styles = func() *enhance.StyleLibrary { return lib }
	styler := preparing{got: make(chan config.Provider, 1)}
	r.c.d.Styler = styler
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	select {
	case p := <-styler.got:
		if p.ID != "claude-code" {
			t.Fatal(p.ID)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("the provider was not prepared")
	}
	if got := StyleTimeout(r.cfg.AI, r.cfg.AI.Providers[0]); got != 8*time.Second {
		t.Fatal(got)
	}
	var _ Preparer = &DefaultStyler{}
}

type preloadCleaner struct{ got chan string }

func (preloadCleaner) Clean(enhance.CleanupRequest) (enhance.CleanupResponse, error) {
	return enhance.CleanupResponse{}, nil
}
func (p preloadCleaner) Preload(model string) { p.got <- model }

// A cleanup model loads while the microphone records, unless the style runs nothing on it.
func TestDefaultStylerPreloadsTheCleanupModel(t *testing.T) {
	lib := enhance.LoadStyles(t.TempDir())
	clean, _ := lib.Get("clean")
	raw, _ := lib.Get("raw")
	model := "s1-mini"
	p := config.Provider{ID: "c", Kind: config.KindCleanupModel, Model: &model}
	c := preloadCleaner{got: make(chan string, 2)}
	d := &DefaultStyler{Cleaner: c}
	d.Prepare(raw, p)
	d.Prepare(clean, p)
	d.Prepare(clean, config.Provider{ID: "o", Kind: config.KindOpenAICompatible})
	if len(c.got) != 1 || <-c.got != "s1-mini" {
		t.Fatal("preload")
	}
	(&DefaultStyler{}).Prepare(clean, p) // no cleaner: nothing
}

func TestWhatADictationNeeds(t *testing.T) {
	r := newRig(t, nil)
	r.c.d.Engine = nil
	r.c.Toggle()
	time.Sleep(30 * time.Millisecond)
	r.obs.mu.Lock()
	defer r.obs.mu.Unlock()
	if len(r.obs.notices) == 0 || r.obs.notices[0].Key != "overlay.notice.no_engine" || len(r.obs.seen) != 0 {
		t.Fatal(r.obs.notices, r.obs.seen)
	}
	if p, m, ok := SplitModelID("openai:gpt-4o-transcribe"); !ok || p != "openai" || m != "gpt-4o-transcribe" {
		t.Fatal(p, m)
	}
	if msg("k", "a", 1).String() != "k{a=1}" {
		t.Fatal(msg("k", "a", 1).String())
	}
}

// slowContext is a focused app that takes until release to answer, as an
// app busy with its own work answers Accessibility.
type slowContext struct{ release chan struct{} }

func (c slowContext) ActiveApp() (platform.ActiveApp, error) {
	<-c.release
	return fakeContext{}.ActiveApp()
}

// The overlay shows a recording at once: a media player that takes its
// time to pause, or to resume, holds neither the recording state nor the
// processing state.
func TestSlowMediaDoesNotHoldTheStates(t *testing.T) {
	r := newRig(t, nil)
	hold := make(chan struct{})
	free := sync.OnceFunc(func() { close(hold) })
	t.Cleanup(free) // before the rig closes, so a held controller can stop
	r.media.hold = hold
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.audio.speak(16000)
	r.c.Toggle()
	r.obs.waitFor(t, "processing")
	free()
	r.obs.waitFor(t, "done")
	r.media.waitCounts(t, 1, 1)
}

// Quitting while the player still pauses leaves the user's music playing:
// the resume queued behind the pause plays before Close returns.
func TestCloseResumesTheMediaItPaused(t *testing.T) {
	for i := range 20 {
		r := newRig(t, nil)
		hold := make(chan struct{})
		r.media.hold = hold
		r.c.Toggle()
		r.obs.waitFor(t, "recording")
		r.c.Cancel()
		r.obs.waitFor(t, "cancelled")
		time.AfterFunc(20*time.Millisecond, func() { close(hold) })
		r.c.Close()
		r.media.mu.Lock()
		p, res := r.media.pauses, r.media.resumes
		r.media.mu.Unlock()
		if p != 1 || res != 1 {
			t.Fatalf("run %d: paused %d and resumed %d times by Close, want 1 and 1", i, p, res)
		}
	}
}

// The recording state comes before the focused app is known: the overlay
// does not wait for Accessibility. The dictation still learns the app.
func TestRecordingShowsBeforeTheFocusedApp(t *testing.T) {
	r := newRig(t, nil)
	release := make(chan struct{})
	free := sync.OnceFunc(func() { close(release) })
	t.Cleanup(free) // before the rig closes, so a held controller can stop
	r.c.d.Platform.Context = slowContext{release: release}
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	free()
	r.audio.speak(16000)
	r.c.Toggle()
	r.obs.waitFor(t, "done")
	<-r.history.saved
	r.history.mu.Lock()
	e := r.history.entries[0]
	r.history.mu.Unlock()
	if e.App == nil || e.App.BundleID != "com.example.notes" {
		t.Fatalf("app %+v", e.App)
	}
}

// At rest the controller does not wake up: the clock of the machine runs
// only while a recording or a notice can time out. It still ends a notice
// and a recording that is too long.
func TestTheControllerSleepsAtRest(t *testing.T) {
	r := newRig(t, nil)
	time.Sleep(20 * 10 * time.Millisecond) // 20 ticks of the rig
	if n := r.host.gates.Load(); n != 0 {
		t.Fatalf("the controller ran %d times at rest", n)
	}
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.audio.speak(16000)
	r.c.Toggle()
	r.obs.waitFor(t, "done")
	r.obs.waitFor(t, "idle")
	n := r.host.gates.Load()
	time.Sleep(20 * 10 * time.Millisecond)
	if m := r.host.gates.Load(); m != n {
		t.Fatalf("the controller ran %d times at rest after a dictation", m-n)
	}
}

func TestATooLongRecordingStops(t *testing.T) {
	r := newRig(t, nil)
	r.c.do(func() { r.c.machine.Timing.MaxDurationMs = 50 })
	r.c.Toggle()
	r.obs.waitFor(t, "recording")
	r.audio.speak(16000)
	r.obs.waitFor(t, "done")
}
