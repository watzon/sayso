package ui

import (
	"context"
	"errors"
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/store"
)

// slowCapture is a capture like macAudio: Start returns once the device
// opened (after delay), Stop is a no-op while nothing runs yet, and a
// second Start while running fails.
type slowCapture struct {
	mu      sync.Mutex
	delay   time.Duration
	running bool
	starts  atomic.Int32
	lists   atomic.Int32
	devices []platform.AudioDevice
	rms     float32
}

func (s *slowCapture) ListDevices() ([]platform.AudioDevice, error) {
	s.lists.Add(1)
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.devices == nil {
		return []platform.AudioDevice{{ID: "mbp", Name: "MacBook Pro Mic", IsDefault: true}}, nil
	}
	return s.devices, nil
}

func (s *slowCapture) Start(_ context.Context, _ string, on func(platform.AudioChunk)) error {
	s.starts.Add(1)
	time.Sleep(s.delay)
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.running {
		return errors.New("audio: capture already running")
	}
	s.running = true
	if s.rms > 0 {
		for i := range 48 {
			on(platform.AudioChunk{RMS: s.rms * (0.4 + 0.6*float32((i*7)%11)/10)})
		}
	}
	return nil
}

func (s *slowCapture) Stop() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.running = false
	return nil
}

func (s *slowCapture) isRunning() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.running
}

// waitMeter waits until every start and stop asked so far has run.
func (m *micMeter) waitMeter() {
	done := make(chan struct{})
	m.queue(func() { close(done) })
	<-done
}

func meterEnv(t *testing.T, delay time.Duration) (*testEnv, *slowCapture) {
	e := newEnv(t)
	capt := &slowCapture{delay: delay}
	perms := &fakePermissions{st: platform.PermissionStatus{Microphone: true, Accessibility: true, NeedsInputPerms: true}}
	e.a.d.Platform = &platform.Services{Permissions: perms, Meter: capt}
	return e, capt
}

// Leaving the permissions step while the meter's capture still opens does
// not leave the microphone on, and coming back runs the test again.
func TestMeterStopWhileTheMicrophoneOpensLeavesItOff(t *testing.T) {
	e, capt := meterEnv(t, 60*time.Millisecond)
	e.a.goToStep(stepPermissions)
	e.a.syncMeter(true) // the frame of the permissions step starts the test
	e.a.goToStep(stepHotkeys)
	e.a.onb.access.meter.waitMeter()
	if capt.isRunning() {
		t.Fatal("the microphone test still records after leaving the step")
	}
	e.a.goToStep(stepPermissions)
	e.a.syncMeter(true)
	e.a.onb.access.meter.waitMeter()
	if _, on, err := e.a.onb.access.meter.snapshot(); err != "" || !on || !capt.isRunning() {
		t.Errorf("coming back: on %v, err %q, running %v", on, err, capt.isRunning())
	}
	// Closing the window stops it too.
	e.a.stopMeter()
	e.a.onb.access.meter.waitMeter()
	if capt.isRunning() {
		t.Error("the window closed with the microphone on")
	}
}

// Back and forth quickly: an old stop never stops the new test, and no
// "already running" error sticks.
func TestMeterQuickReturnsKeepTheTestRunning(t *testing.T) {
	e, capt := meterEnv(t, 5*time.Millisecond)
	m := &e.a.onb.access.meter
	e.a.goToStep(stepPermissions)
	e.a.syncMeter(true)
	m.waitMeter()
	if !capt.isRunning() {
		t.Fatal("the meter did not start")
	}
	for i := range 50 {
		e.a.goToStep(stepHotkeys)
		e.a.goToStep(stepPermissions)
		e.a.syncMeter(true)
		m.waitMeter()
		if _, on, err := m.snapshot(); err != "" || !on || !capt.isRunning() {
			t.Fatalf("return %d: on %v, err %q, running %v", i, on, err, capt.isRunning())
		}
		e.a.goToStep(stepHotkeys)
		m.waitMeter()
		if capt.isRunning() {
			t.Fatalf("return %d: the microphone stayed on", i)
		}
	}
}

// Finish saves completed to disk; a new App from the saved config resumes
// at the saved step.
func TestOnboardingFinishIsSavedAndTheStepResumes(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepHotkeys)
	loaded, err := config.Load(e.paths.ConfigFile())
	if err != nil {
		t.Fatal(err)
	}
	if a2 := New(Deps{Config: loaded.Config}); a2.OnboardingStep() != stepHotkeys || !a2.NeedsOnboarding() {
		t.Errorf("resume: step %d, needs %v", a2.OnboardingStep(), a2.NeedsOnboarding())
	}
	e.a.goToStep(stepDone)
	press(t, onboardingTester(e), i18n.T("onboarding.footer.open_sayso"))
	if loaded, err = config.Load(e.paths.ConfigFile()); err != nil {
		t.Fatal(err)
	}
	if !loaded.Config.Onboarding.Completed || New(Deps{Config: loaded.Config}).NeedsOnboarding() {
		t.Error("finish was not saved")
	}
}

func TestArrowTargetStopsAtTheEnds(t *testing.T) {
	for _, c := range []struct {
		key          mu.Key
		sel, n, want int
		ok           bool
	}{
		{mu.KeyDown, 0, 3, 1, true}, {mu.KeyRight, 1, 3, 2, true}, {mu.KeyDown, 2, 3, 0, false},
		{mu.KeyUp, 2, 3, 1, true}, {mu.KeyLeft, 0, 3, 0, false}, {mu.KeyA, 0, 3, 0, false},
		// With nothing chosen the first key picks an end.
		{mu.KeyDown, -1, 3, 0, true}, {mu.KeyUp, -1, 3, 2, true}, {mu.KeyUp, -1, 0, 0, false},
	} {
		got, ok := arrowTarget(c.key, c.sel, c.n)
		if ok != c.ok || (ok && got != c.want) {
			t.Errorf("%v from %d of %d: %d %v, want %d %v", c.key, c.sel, c.n, got, ok, c.want, c.ok)
		}
	}
}

func arrow(tt *mu.Tester, k mu.Key) {
	tt.Key(0, k)
	tt.Frame()
	tt.Frame() // the focus request lands
}

// The model cards are one radio group: the arrow keys choose the next or
// previous card and take the focus there, and stop at the ends.
func TestModelCardsFollowTheArrowKeys(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepModel)
	choices := modelChoices(e.a.cfg.Dictation.Language)
	e.a.chooseModel(choices[0])
	e.a.onb.focus = "model:" + string(choices[0])
	tt := onboardingTester(e)
	tt.Frame()
	tt.Frame()
	for _, s := range []struct {
		key  mu.Key
		want int
	}{{mu.KeyRight, 1}, {mu.KeyDown, 2}, {mu.KeyDown, 2}, {mu.KeyLeft, 1}, {mu.KeyUp, 0}} {
		arrow(tt, s.key)
		if e.a.FinalModel() != choices[s.want] || e.a.onb.focus != "model:"+string(choices[s.want]) {
			t.Fatalf("%v: model %s, focus %q; want %s", s.key, e.a.FinalModel(), e.a.onb.focus, choices[s.want])
		}
	}
	// Tab reaches the cards from the keyboard alone.
	e.a.onb.focus = ""
	for range 12 {
		tt.Key(0, mu.KeyTab)
		tt.Frame()
	}
	before := e.a.FinalModel()
	arrow(tt, mu.KeyRight)
	arrow(tt, mu.KeyDown)
	if e.a.FinalModel() == before {
		t.Error("after Tab, the arrow keys did not change the model")
	}
}

// The AI options, the Ollama models and the cloud presets take the arrow
// keys as the model cards do.
func TestAIGroupsFollowTheArrowKeys(t *testing.T) {
	e := newEnv(t)
	e.a.detectAI = func(enhance.LanguageModel) detected {
		return detected{claude: "/opt/bin/claude", ollama: []string{"qwen3:4b", "llama3.2:3b"}, ollamaOK: true}
	}
	e.a.goToStep(stepAI)
	tt := onboardingTester(e)
	tt.Frame()
	waitFor(t, func() bool { tt.Frame(); return e.a.onb.ai.found.done })
	e.a.selectAI(aiChoice{kind: aiClaude})
	e.a.onb.focus = "ai:claude"
	tt.Frame()
	tt.Frame()
	arrow(tt, mu.KeyDown)
	if ch, ok := e.a.aiChoice(); !ok || ch.kind != aiCloud || e.a.onb.focus != "ai:cloud" {
		t.Fatalf("Down from Claude: %+v %v, focus %q", ch, ok, e.a.onb.focus)
	}
	// The presets of the cloud form.
	e.a.onb.focus = "preset:openrouter"
	tt.Frame()
	tt.Frame()
	arrow(tt, mu.KeyRight)
	if f := e.a.onb.ai; f.preset != 1 || f.baseURL != aiPresets[1][2] {
		t.Fatalf("Right on OpenRouter: preset %d, url %q", f.preset, f.baseURL)
	}
	// The Ollama list.
	e.a.selectAI(aiChoice{kind: aiLocal})
	e.a.onb.ai.localOpen = true
	e.a.onb.focus = "ollama:qwen3:4b"
	tt.Frame()
	tt.Frame()
	arrow(tt, mu.KeyDown)
	if e.a.onb.ai.local != "llama3.2:3b" {
		t.Errorf("Down in the Ollama list: %q", e.a.onb.ai.local)
	}
}

// fakeHotkeys is a hotkey manager that finds the conflicts a test sets.
type fakeHotkeys struct {
	mu        sync.Mutex
	conflicts map[string][]platform.HotkeyConflict
	lookups   int
}

func (f *fakeHotkeys) Configure(platform.HotkeyConfig) error  { return nil }
func (f *fakeHotkeys) SetRecording(bool)                      {}
func (f *fakeHotkeys) Start(func(platform.HotkeyEvent)) error { return nil }
func (f *fakeHotkeys) Stop() error                            { return nil }
func (f *fakeHotkeys) Conflicts(h string) []platform.HotkeyConflict {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.lookups++
	return f.conflicts[h]
}

// Where the system lets Sayso look, the step names the other programs
// that use the keys; a system shortcut that is off does not count.
func TestHotkeysStepNamesOtherPrograms(t *testing.T) {
	e := newEnv(t)
	toggle := config.MustHotkey("opt+space")
	e.a.cfg.Hotkeys.Toggle = &toggle
	e.a.cfg.Hotkeys.PushToTalk = nil
	hk := &fakeHotkeys{conflicts: map[string][]platform.HotkeyConflict{
		toggle.String(): {
			{Hotkey: toggle.String(), App: true, Name: "ChatGPT"},
			{Hotkey: toggle.String(), Name: "Spotlight search", Enabled: false},
		},
	}}
	e.a.d.Platform = &platform.Services{Hotkeys: hk}
	e.a.goToStep(stepHotkeys)
	tt := onboardingTester(e)
	tt.Frame()
	chatgpt := conflictText(platform.HotkeyConflict{Hotkey: toggle.String(), App: true, Name: "ChatGPT"})
	waitFor(t, func() bool { tt.Frame(); return tt.HasText(chatgpt) })
	if !tt.HasText(i18n.T("onboarding.hotkeys.lead")) || !tt.HasText(i18n.T("onboarding.hotkeys.conflict")) {
		t.Errorf("lead or conflict status missing: %q", tt.Texts())
	}
	spotlight := conflictText(platform.HotkeyConflict{Hotkey: toggle.String(), Name: "Spotlight search"})
	if tt.HasText(spotlight) {
		t.Error("a shortcut that is off counts")
	}
	// A key no other program uses.
	free := config.MustHotkey("ctrl+opt+k")
	e.a.setRecorded(slotToggle, &free)
	waitFor(t, func() bool { tt.Frame(); return tt.HasText(i18n.T("onboarding.hotkeys.no_conflict")) })
	if tt.HasText(chatgpt) {
		t.Error("the old conflict still shows")
	}
}

// Where Sayso cannot look at other programs, the step promises only the
// check of its own actions.
func TestHotkeysStepWithoutAFinderPromisesOnlySaysosCheck(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepHotkeys)
	tt := onboardingTester(e)
	tt.Frame()
	if !tt.HasText(i18n.T("onboarding.hotkeys.lead_local")) || tt.HasText(i18n.T("onboarding.hotkeys.lead")) {
		t.Errorf("lead: %q", tt.Texts())
	}
	if !tt.HasText(i18n.T("onboarding.hotkeys.no_clash")) || tt.HasText(i18n.T("onboarding.hotkeys.no_conflict")) {
		t.Errorf("status: %q", tt.Texts())
	}
}

func TestConflictTextNamesTheShortcutInTheInterfaceLanguage(t *testing.T) {
	if got := shortcutName("Emoji & Symbols"); got != i18n.T("settings.recorder.shortcut.emoji_symbols") {
		t.Errorf("Emoji & Symbols: %q", got)
	}
	if got := shortcutName("Dock: turn hiding on or off"); got != i18n.T("settings.recorder.shortcut.dock_turn_hiding_on_or_off") {
		t.Errorf("Dock: %q", got)
	}
	if got := shortcutName("Mission Control"); got != "Mission Control" {
		t.Errorf("a name without a text: %q", got)
	}
	got := conflictText(platform.HotkeyConflict{Hotkey: "opt+space", App: true, Name: "Raycast", Confirmed: true})
	if want := i18n.T("settings.recorder.conflict_app", "name", "Raycast", "keys", hotkeyWords(config.MustHotkey("opt+space"))); got != want {
		t.Errorf("got %q, want %q", got, want)
	}
}

// The model-in-memory dot: loading (accent) while Sayso keeps the model
// loaded and it is not in memory yet; idle when it loads on first use.
func TestPracticeModelDotLoadsUntilIdle(t *testing.T) {
	e := newEnv(t)
	e.eng.setState(string(e.a.FinalModel()), engine.ModelStatus{State: engine.StateDownloaded})
	e.a.goToStep(stepPractice)
	tt := onboardingTester(e)
	e.a.cfg.Dictation.PreloadModel = true
	tt.Frame()
	if !tt.HasText(i18n.T("onboarding.practice.opt_loading")) {
		t.Errorf("preload on: %q", tt.Texts())
	}
	e.a.cfg.Dictation.PreloadModel = false
	tt.Frame()
	if !tt.HasText(i18n.T("onboarding.practice.opt_idle")) {
		t.Errorf("preload off: %q", tt.Texts())
	}
}

// The permissions step lists the microphones again while it shows: one
// plugged in appears in the microphone test.
func TestPermissionsStepFollowsTheMicrophones(t *testing.T) {
	e, capt := meterEnv(t, 0)
	e.a.d.Platform.Audio = capt
	e.a.goToStep(stepPermissions)
	tt := onboardingTester(e)
	tt.Frame()
	if !tt.HasText("MacBook Pro Mic") {
		t.Fatalf("no device: %q", tt.Texts())
	}
	capt.mu.Lock()
	capt.devices = []platform.AudioDevice{{ID: "usb", Name: "USB Microphone", IsDefault: true}}
	capt.mu.Unlock()
	tt.Advance(permPoll + 100*time.Millisecond)
	tt.Frame()
	tt.Frame()
	if !tt.HasText("USB Microphone") {
		t.Errorf("the new microphone does not show: %q", tt.Texts())
	}
	e.a.stopMeter()
	e.a.onb.access.meter.waitMeter()
}

// A long label keeps the cloud form's labels on one line ("API-Schlüssel").
func TestCloudLabelsFitTheLongestLabel(t *testing.T) {
	prev := i18n.CurrentLocale()
	t.Cleanup(func() { i18n.SetLocale(prev) })
	i18n.SetLocale("en")
	en := cloudLabelWidth()
	i18n.SetLocale("de")
	de := cloudLabelWidth()
	lw, _ := mu.ShapeText(i18n.T("onboarding.ai.api_key"), mu.Font{Family: "Instrument Sans", Size: 13, Weight: 500}, 0).Size()
	if de < lw || de < 70 || en < 70 {
		t.Errorf("en %v, de %v, API-Schlüssel %v", en, de, lw)
	}
}

// ---- Screenshots ----

// The steps in the states the Paper artboards draw, light and dark, plus
// de and ru, written to $SAYSO_ONB_SHOTS (skipped without it).
func TestOnboardingShots(t *testing.T) {
	out := os.Getenv("SAYSO_ONB_SHOTS")
	if out == "" {
		t.Skip("SAYSO_ONB_SHOTS not set")
	}
	capt := &slowCapture{rms: 0.7}
	for _, dark := range []bool{false, true} {
		mode := map[bool]string{false: "light", true: "dark"}[dark]
		for i, s := range paperStates(capt) {
			_ = capt.Stop()
			renderStep(t, s.step, s.set, dark, "", fmt.Sprintf("%s/%02d-%s-%s.png", out, i+1, s.name, mode), nil)
		}
	}
	for _, lang := range []string{"de", "ru"} {
		for i, s := range paperStates(capt) {
			_ = capt.Stop()
			renderStep(t, s.step, s.set, false, lang, fmt.Sprintf("%s/locale/%02d-%s-%s.png", out, i+1, s.name, lang), nil)
		}
	}
	x := out + "/extra"
	openAll := func(e *testEnv, tt *mu.Tester) { e.a.openAllModels(); tt.Frame() }
	cloud := func(e *testEnv, tt *mu.Tester) {
		e.a.selectAI(aiChoice{kind: aiCloud})
		e.a.onboardingNext()
		tt.Frame()
	}
	renderStep(t, stepModel, nil, false, "", x+"/model-all-models-light.png", openAll)
	renderStep(t, stepModel, nil, true, "", x+"/model-all-models-dark.png", openAll)
	renderStep(t, stepAI, nil, false, "", x+"/ai-cloud-form-light.png", cloud)
	renderStep(t, stepAI, nil, false, "de", x+"/ai-cloud-form-de.png", cloud)
	renderStep(t, stepPermissions, func(t *testing.T, e *testEnv) {
		e.a.d.Platform = &platform.Services{Permissions: &fakePermissions{st: platform.PermissionStatus{MicrophoneDenied: true, NeedsInputPerms: true}}}
	}, false, "", x+"/permissions-blocked-light.png", nil)
	renderStep(t, stepHotkeys, nil, false, "", x+"/hotkeys-recording-ptt-light.png", func(e *testEnv, tt *mu.Tester) {
		e.a.cfg.Hotkeys.PushToTalk = nil
		e.a.startRecording(slotPTT)
		tt.Frame()
	})
	renderStep(t, stepWelcome, nil, false, "ru", x+"/welcome-ru.png", nil)
	renderStep(t, stepWelcome, nil, false, "ja", x+"/welcome-ja.png", nil)
	renderStep(t, stepWelcome, nil, false, "zh-CN", x+"/welcome-zh-CN.png", nil)
	renderStep(t, stepWelcome, nil, false, "ko", x+"/welcome-ko.png", nil)
}

type stepState func(t *testing.T, e *testEnv)

// paperStates are the states of the Paper artboards (Thorn's QA setup).
func paperStates(capt *slowCapture) []struct {
	name string
	step int
	set  stepState
} {
	optSpace := func(e *testEnv) { e.a.cfg.Hotkeys.Toggle = ptr(config.MustHotkey("opt+space")) }
	return []struct {
		name string
		step int
		set  stepState
	}{
		{"welcome", stepWelcome, nil},
		{"model", stepModel, func(t *testing.T, e *testEnv) {
			e.eng.setState(string(e.a.FinalModel()), engine.ModelStatus{State: engine.StateDownloading, Fraction: 0.32, BytesDone: 412e6, BytesTotal: 1300e6})
		}},
		{"ai", stepAI, func(t *testing.T, e *testEnv) {
			e.a.detectAI = func(enhance.LanguageModel) detected {
				return detected{claude: "/Users/x/.local/bin/claude", ollama: []string{"qwen3:4b", "llama3.2:3b"}, ollamaOK: true}
			}
			e.a.onb.ai.found = detected{}
			e.a.detectProviders(true)
			e.a.selectAI(aiChoice{kind: aiLocal})
		}},
		{"permissions", stepPermissions, func(t *testing.T, e *testEnv) {
			perms := &fakePermissions{st: platform.PermissionStatus{Microphone: true, NeedsInputPerms: true}}
			e.a.d.Platform = &platform.Services{Permissions: perms, Meter: capt, Audio: capt}
			e.eng.setState(string(e.a.FinalModel()), engine.ModelStatus{State: engine.StateDownloading, Fraction: 0.68, BytesDone: 884e6, BytesTotal: 1300e6})
		}},
		{"hotkeys", stepHotkeys, func(t *testing.T, e *testEnv) {
			optSpace(e)
			e.a.cfg.Hotkeys.PushToTalk = nil
			// Paper's hotkeys artboard: ChatGPT is open and uses ⌥ Space.
			k := e.a.cfg.Hotkeys.Toggle.String()
			e.a.d.Platform = &platform.Services{Hotkeys: &fakeHotkeys{conflicts: map[string][]platform.HotkeyConflict{
				k: {{Hotkey: k, App: true, Name: "ChatGPT"}},
			}}}
		}},
		{"practice", stepPractice, func(t *testing.T, e *testEnv) {
			e.eng.setState(string(e.a.FinalModel()), engine.ModelStatus{State: engine.StateReady})
			optSpace(e)
			e.a.d.Platform = &platform.Services{Permissions: &fakePermissions{st: platform.PermissionStatus{Microphone: true, Accessibility: true, NeedsInputPerms: true}}}
			en := store.HistoryEntry{CreatedAt: fixedNow.Add(time.Second), Transcript: practiceSentence(), FinalText: practiceSentence(), StyleID: "raw", TranscribeMs: 300}
			en.Insert.Result = store.InsertPasted
			if _, err := e.db.InsertEntry(en); err != nil {
				t.Fatal(err)
			}
			e.a.dataChanged()
		}},
		{"done", stepDone, func(t *testing.T, e *testEnv) {
			optSpace(e)
			e.a.cfg.Hotkeys.PasteLast = ptr(config.MustHotkey("ctrl+cmd+v"))
		}},
	}
}

func renderStep(t *testing.T, step int, set stepState, dark bool, lang, path string, extra func(e *testEnv, tt *mu.Tester)) {
	t.Helper()
	e := newEnv(t)
	if lang != "" {
		e.a.cfg.General.Language = lang
		e.a.applyLocale()
		t.Cleanup(func() { e.a.cfg.General.Language = "en"; e.a.applyLocale() })
	}
	if dark {
		e.a.cfg.Appearance.Theme = config.ThemeDark
	}
	e.a.goToStep(step)
	if set != nil {
		set(t, e)
	}
	tt := mu.NewTesterWith(e.a.OnboardingView, OnboardingW, OnboardingH, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
	tt.SetTitleBar(mu.TitleBar{Height: 28, Left: 78, Bottom: 28})
	tt.SetDark(dark)
	if lang != "" {
		tt.SetLocale(lang)
	}
	tt.Frame()
	e.a.onb.access.meter.waitMeter()
	time.Sleep(60 * time.Millisecond) // a conflict lookup
	tt.Frame()
	if extra != nil {
		extra(e, tt)
	}
	tt.Frame()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, tt.Image()); err != nil {
		t.Fatal(err)
	}
	e.a.stopMeter()
	e.a.onb.access.meter.waitMeter()
}
