package ui

import (
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/paths"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
)

// fakeDictation records what the views asked of the controller.
type fakeDictation struct {
	calls []string
	last  string
}

func (f *fakeDictation) Toggle()             { f.calls = append(f.calls, "toggle") }
func (f *fakeDictation) Cancel()             { f.calls = append(f.calls, "cancel") }
func (f *fakeDictation) PushToTalkDown()     { f.calls = append(f.calls, "ptt-down") }
func (f *fakeDictation) PushToTalkUp()       { f.calls = append(f.calls, "ptt-up") }
func (f *fakeDictation) PickStyle(id string) { f.calls = append(f.calls, "style:"+id) }
func (f *fakeDictation) Dismiss()            { f.calls = append(f.calls, "dismiss") }
func (f *fakeDictation) Undo()               { f.calls = append(f.calls, "undo") }
func (f *fakeDictation) LastText() string    { return f.last }

func (f *fakeDictation) count(call string) int {
	n := 0
	for _, c := range f.calls {
		if c == call {
			n++
		}
	}
	return n
}

// fakeEngine is a running engine with fixed model states.
type fakeEngine struct {
	mu         sync.Mutex
	states     map[string]engine.ModelStatus
	downloaded []string
}

func (f *fakeEngine) Status(m string) engine.ModelStatus {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.states[m]
}
func (f *fakeEngine) WaitReady(m string, _ time.Time) engine.ModelStatus { return f.Status(m) }
func (f *fakeEngine) setState(m string, s engine.ModelStatus) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.states[m] = s
}
func (f *fakeEngine) downloads() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.downloaded)
}
func (f *fakeEngine) Load(string) error { return nil }
func (f *fakeEngine) Transcribe(_ []float32, o dictation.SessionOptions) (dictation.Transcript, error) {
	return dictation.Transcript{Text: "again", Model: o.FinalModel}, nil
}
func (f *fakeEngine) PreviewReady(string) bool                           { return true }
func (f *fakeEngine) StartStream(uint64, dictation.SessionOptions) error { return nil }
func (f *fakeEngine) PushAudio(uint64, []float32) error                  { return nil }
func (f *fakeEngine) EndStream(uint64)                                   {}
func (f *fakeEngine) Running() bool                                      { return true }
func (f *fakeEngine) Download(m string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.downloaded = append(f.downloaded, m)
	return nil
}
func (f *fakeEngine) Delete(string) error { return nil }

// fakeShell records what App asked of the desktop.
type fakeShell struct {
	shown   map[Window]bool
	overlay bool
	opened  []string
	hub     int
	quit    bool
	theme   config.ThemeMode
	// redraws is atomic: background loaders (Keychain, storage) redraw too.
	redraws atomic.Int64
}

func newFakeShell() *fakeShell                         { return &fakeShell{shown: map[Window]bool{}} }
func (s *fakeShell) Invalidate()                       { s.redraws.Add(1) }
func (s *fakeShell) ShowHub()                          { s.hub++ }
func (s *fakeShell) ShowOverlay(show bool)             { s.overlay = show }
func (s *fakeShell) ShowWindow(w Window, show bool)    { s.shown[w] = show }
func (s *fakeShell) SetThemeMode(m config.ThemeMode)   { s.theme = m }
func (s *fakeShell) SetOverlaySize(config.OverlaySize) {}
func (s *fakeShell) OpenPath(p string)                 { s.opened = append(s.opened, p) }
func (s *fakeShell) OpenURL(u string)                  { s.opened = append(s.opened, u) }
func (s *fakeShell) Quit()                             { s.quit = true }

type testEnv struct {
	a     *App
	dict  *fakeDictation
	eng   *fakeEngine
	shell *fakeShell
	db    *store.Store
	paths paths.Paths
}

// fixedNow is a Wednesday afternoon.
var fixedNow = time.Date(2026, 11, 18, 15, 4, 0, 0, time.Local)

func newEnv(t *testing.T) *testEnv {
	t.Helper()
	i18n.SetLocale("en")
	t.Cleanup(func() { i18n.SetLocale("en") })
	dir := t.TempDir()
	p := paths.Paths{ConfigDir: filepath.Join(dir, "config"), DataDir: filepath.Join(dir, "data"), CacheDir: filepath.Join(dir, "cache")}
	if err := p.CreateAll(); err != nil {
		t.Fatal(err)
	}
	db, err := store.Open(p.DatabaseFile(), p.AudioDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { db.Close() })
	cfg := config.Default()
	cfg.General.Language = "en"
	e := &testEnv{dict: &fakeDictation{}, eng: &fakeEngine{states: map[string]engine.ModelStatus{}}, shell: newFakeShell(), db: db, paths: p}
	e.a = New(Deps{
		Paths: p, ConfigPath: p.ConfigFile(), Config: cfg, Store: db, Engine: e.eng, Dictation: e.dict,
		Secrets: providers.NewMemoryStore(), Version: "0.16.0", Now: func() time.Time { return fixedNow }, Shell: e.shell,
		ReleaseNotes: testNotes,
	})
	e.a.TrackKeys = true
	// Onboarding never looks for the AI tools of the machine that runs the tests.
	e.a.detectAI = func(enhance.LanguageModel) detected { return detected{} }
	RegisterFonts()
	return e
}

const testNotes = `
[[release]]
version = "0.16.0"
title = "Sayso on Go"

[[release.item]]
icon = "sparkle"
title = "A new engine room"
text = "The app now runs on MyGo."
`

func (e *testEnv) addEntry(t *testing.T, text, app string, audio bool) int64 {
	t.Helper()
	en := store.HistoryEntry{CreatedAt: fixedNow.Add(-time.Hour), DurationMs: 4000, Transcript: text, FinalText: text, StyleID: "raw", Model: "parakeet-tdt-v3",
		App: &store.TargetApp{BundleID: "com." + strings.ToLower(app), Name: app}}
	en.Insert.Result = store.InsertPasted
	if audio {
		name, err := e.db.SaveAudio(make([]float32, 16000))
		if err != nil {
			t.Fatal(err)
		}
		en.AudioFile = name
	}
	id, err := e.db.InsertEntry(en)
	if err != nil {
		t.Fatal(err)
	}
	e.a.dataChanged()
	return id
}

func (e *testEnv) hub(t *testing.T) *mu.Tester {
	t.Helper()
	return mu.NewTester(e.a.HubView, HubWidth, HubHeight)
}

// press clicks the first control with an accessible name.
func press(t *testing.T, tt *mu.Tester, name string) {
	t.Helper()
	for _, a := range tt.AccessAll() {
		if a.Name != name || a.Role == mu.RoleText || a.Role == mu.RoleNone {
			continue
		}
		b := a.Bounds
		tt.ClickAt(b.X+b.W/2, b.Y+b.H/2)
		return
	}
	t.Fatalf("no control named %q; texts %q", name, tt.Texts())
}

// pressLast clicks the last control with an accessible name.
func pressLast(t *testing.T, tt *mu.Tester, name string) {
	t.Helper()
	var hit *mu.AccessInfo
	for _, a := range tt.AccessAll() {
		if a.Name == name && a.Role != mu.RoleText && a.Role != mu.RoleNone {
			a := a
			hit = &a
		}
	}
	if hit == nil {
		t.Fatalf("no control named %q", name)
	}
	tt.ClickAt(hit.Bounds.X+hit.Bounds.W/2, hit.Bounds.Y+hit.Bounds.H/2)
}

// controlBounds is the box of the first control with an accessible name.
func controlBounds(tt *mu.Tester, name string) (mu.Rect, bool) {
	for _, a := range tt.AccessAll() {
		if a.Name == name && a.Role != mu.RoleText && a.Role != mu.RoleNone {
			return a.Bounds, true
		}
	}
	return mu.Rect{}, false
}

func hasControl(tt *mu.Tester, name string) bool {
	for _, a := range tt.AccessAll() {
		if a.Name == name && a.Role != mu.RoleText {
			return true
		}
	}
	return false
}

func missingKeys(seen map[string]bool, want []string) []string {
	var miss []string
	for _, k := range want {
		if !seen[k] {
			miss = append(miss, k)
		}
	}
	sort.Strings(miss)
	return miss
}
