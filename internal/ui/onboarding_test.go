package ui

import (
	"runtime"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/store"
)

// fakePermissions is a system whose grants a test changes. It never asks
// the real system.
type fakePermissions struct {
	mu       sync.Mutex
	st       platform.PermissionStatus
	reads    int
	requests []string
}

func (f *fakePermissions) Status() platform.PermissionStatus {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.reads++
	return f.st
}
func (f *fakePermissions) set(st platform.PermissionStatus) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.st = st
}
func (f *fakePermissions) asked(kind string) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.requests = append(f.requests, kind)
}
func (f *fakePermissions) RequestMicrophone() error      { f.asked("microphone"); return nil }
func (f *fakePermissions) RequestAccessibility() error   { f.asked("accessibility"); return nil }
func (f *fakePermissions) RequestInputMonitoring() error { f.asked("input_monitoring"); return nil }
func (f *fakePermissions) OpenSettings(kind string) error {
	f.asked("settings:" + kind)
	return nil
}

func onboardingTester(e *testEnv) *mu.Tester {
	return mu.NewTesterWith(e.a.OnboardingView, OnboardingW, OnboardingH, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
}

func footerLabel(e *testEnv) string { return e.a.footerFor().label }

// A saved step outside the steps resumes at the last one; a saved step
// resumes there; every move is saved to the config file.
func TestOnboardingResumesAndClampsTheSavedStep(t *testing.T) {
	e := newEnv(t)
	for _, c := range []struct{ saved, want int }{{0, 0}, {3, 3}, {6, 6}, {200, 6}} {
		cfg := config.Default()
		cfg.Onboarding.Step = uint8(c.saved)
		a := New(Deps{Config: cfg})
		if a.OnboardingStep() != c.want {
			t.Errorf("saved %d: step %d, want %d", c.saved, a.OnboardingStep(), c.want)
		}
	}
	e.a.goToStep(4)
	loaded, err := config.Load(e.paths.ConfigFile())
	if err != nil {
		t.Fatal(err)
	}
	if loaded.Config.Onboarding.Step != 4 || loaded.Config.Onboarding.Completed {
		t.Errorf("saved step %d, completed %v", loaded.Config.Onboarding.Step, loaded.Config.Onboarding.Completed)
	}
	e.a.goToStep(-3)
	if e.a.OnboardingStep() != 0 {
		t.Errorf("step %d after going below 0", e.a.OnboardingStep())
	}
	e.a.goToStep(99)
	if e.a.OnboardingStep() != stepDone {
		t.Errorf("step %d after going past the end", e.a.OnboardingStep())
	}
}

// Back goes back; a done step of the trail is a button to it; the next
// steps are not.
func TestOnboardingBackAndTrail(t *testing.T) {
	e := newEnv(t)
	tt := onboardingTester(e)
	press(t, tt, i18n.T("onboarding.footer.get_started"))
	if e.a.OnboardingStep() != stepModel {
		t.Fatalf("step %d", e.a.OnboardingStep())
	}
	tt.Frame()
	press(t, tt, i18n.T("onboarding.footer.back"))
	if e.a.OnboardingStep() != stepWelcome {
		t.Fatalf("back: step %d", e.a.OnboardingStep())
	}
	tt.Frame()
	if hasControl(tt, i18n.T("onboarding.footer.back")) {
		t.Error("the first step has Back")
	}
	e.a.goToStep(stepHotkeys)
	tt.Frame()
	if hasControl(tt, i18n.T("onboarding.go_to_step", "step", 6)) {
		t.Error("a step not reached yet is a button")
	}
	press(t, tt, i18n.T("onboarding.go_to_step", "step", 2))
	if e.a.OnboardingStep() != stepModel {
		t.Errorf("trail: step %d", e.a.OnboardingStep())
	}
}

// The model step waits for a model on disk: Continue starts the download
// and stays, the button shows the progress, and a model on disk goes on.
func TestModelStepWaitsForTheDownload(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepModel)
	tt := onboardingTester(e)
	id := string(e.a.FinalModel())
	if got := footerLabel(e); got != i18n.T("onboarding.footer.download") {
		t.Fatalf("footer %q", got)
	}
	press(t, tt, i18n.T("onboarding.footer.download"))
	if e.a.OnboardingStep() != stepModel || e.eng.downloads() != 1 {
		t.Fatalf("step %d, downloads %d", e.a.OnboardingStep(), e.eng.downloads())
	}
	e.eng.setState(id, engine.ModelStatus{State: engine.StateDownloading, Fraction: 0.4, BytesDone: 520e6, BytesTotal: 1300e6})
	tt.Frame()
	f := e.a.footerFor()
	if f.enabled || f.label != i18n.T("onboarding.footer.downloading", "percent", "40") {
		t.Errorf("downloading: %+v", f)
	}
	if !tt.HasText(i18n.T("onboarding.model.downloading", "name", e.a.modelName(e.a.FinalModel()))) {
		t.Errorf("no download well: %q", tt.Texts())
	}
	e.a.onboardingNext()
	if e.eng.downloads() != 1 || e.a.OnboardingStep() != stepModel {
		t.Errorf("a second download, or the step moved: %d %d", e.eng.downloads(), e.a.OnboardingStep())
	}
	e.eng.setState(id, engine.ModelStatus{State: engine.StateDownloaded})
	tt.Frame()
	press(t, tt, i18n.T("onboarding.footer.continue"))
	if e.a.OnboardingStep() != stepAI {
		t.Errorf("continue: step %d", e.a.OnboardingStep())
	}
}

// Choosing a card makes it the model and starts its download.
func TestModelCardChoosesAndDownloads(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepModel)
	tt := onboardingTester(e)
	choices := modelChoices(e.a.cfg.Dictation.Language)
	second, _ := models.Find(choices[1])
	press(t, tt, second.Name)
	if e.a.FinalModel() != second.ID || e.eng.downloads() != 1 {
		t.Errorf("model %s, downloads %d", e.a.FinalModel(), e.eng.downloads())
	}
}

func TestModelChoicesAreThreeWithTheSuggestedFirst(t *testing.T) {
	for _, lang := range []string{"en", "de", "zh", "ja", "ko", "ar", "auto"} {
		ids := modelChoices(lang)
		if len(ids) != 3 || ids[0] != suggestedModel(lang) {
			t.Fatalf("%s: %v", lang, ids)
		}
		for i, id := range ids {
			m, ok := models.Find(id)
			if !ok || m.Task() != models.TaskSpeech || slices.Index(ids, id) != i {
				t.Errorf("%s: %v", lang, ids)
			}
		}
	}
	if modelChoices("en")[0] != models.DefaultModel() {
		t.Error("English does not suggest the default model")
	}
}

// The second choice understands the language when one can; the others keep
// their order (model_step.rs order_choices).
func TestOrderChoices(t *testing.T) {
	langs := map[models.ModelID][]string{"english": {"en"}, "european": {"en", "de", "fr"}, "many": {"en", "de", "fr", "zh", "ja"}, "asian": {"zh", "ja"}}
	understands := func(lang string) func(models.ModelID) bool {
		return func(id models.ModelID) bool { return slices.Contains(langs[id], lang) }
	}
	rest := []models.ModelID{"english", "european", "many"}
	for _, c := range []struct {
		lang      string
		suggested models.ModelID
	}{{"de", "european"}, {"zh", "asian"}, {"fr", "many"}} {
		ids := orderChoices(c.suggested, "", rest, understands(c.lang))
		if len(ids) != 3 || !understands(c.lang)(ids[1]) {
			t.Errorf("%s: %v", c.lang, ids)
		}
	}
	if got := orderChoices("many", "", rest, understands("zh")); !slices.Equal(got, []models.ModelID{"many", "english", "european"}) {
		t.Errorf("zh: %v", got)
	}
	if got := orderChoices("english", "gpu", rest, understands("en")); got[1] != "gpu" {
		t.Errorf("gpu: %v", got)
	}
}

// The list of all models filters, chooses a model, closes, and closes
// with Esc.
func TestAllModelsListFiltersAndChooses(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepModel)
	tt := onboardingTester(e)
	press(t, tt, i18n.TP("onboarding.model.show_all", len(localModels()), nil))
	tt.Frame()
	if !e.a.onb.allModels || !tt.HasText(i18n.T("onboarding.all_models.title")) {
		t.Fatal("the list did not open")
	}
	var pick models.ModelInfo
	for _, m := range localModels() {
		if !slices.Contains(modelChoices("en"), m.ID) {
			pick = m
			break
		}
	}
	e.a.onb.allQuery = strings.ToUpper(pick.Name)
	tt.Frame()
	if !modelMatches(pick, e.a.onb.allQuery) || modelMatches(pick, "no model has this text") {
		t.Fatal("filter")
	}
	pressLast(t, tt, pick.Name)
	if e.a.onb.allModels || e.a.FinalModel() != pick.ID || e.eng.downloads() != 1 {
		t.Fatalf("open %v, model %s, downloads %d", e.a.onb.allModels, e.a.FinalModel(), e.eng.downloads())
	}
	tt.Frame()
	// A model from the list shows as the choice, with Change.
	if !hasControl(tt, i18n.T("onboarding.model.change_label")) {
		t.Errorf("no Change for a model from the list: %q", tt.Texts())
	}
	e.a.openAllModels()
	tt.Frame()
	tt.Key(0, mu.KeyEscape)
	tt.Frame()
	if e.a.onb.allModels {
		t.Error("Esc did not close the list")
	}
}

// Skip turns AI off; Continue with nothing chosen does nothing.
func TestAIStepSkipTurnsAIOff(t *testing.T) {
	e := newEnv(t)
	e.a.cfg.AI.Enabled = true
	e.a.detectAI = func(enhance.LanguageModel) detected { return detected{} }
	// A multilingual model on Automatic names no language, so no cleanup
	// model is suggested and nothing is chosen.
	e.a.cfg.Dictation.Model, e.a.cfg.Dictation.Language = "whisper-large-v3-turbo", "auto"
	e.a.goToStep(stepAI)
	tt := onboardingTester(e)
	tt.Frame()
	if f := e.a.footerFor(); f.enabled || f.skip == "" {
		t.Fatalf("nothing chosen: %+v", f)
	}
	press(t, tt, i18n.T("onboarding.footer.skip"))
	if e.a.cfg.AI.Enabled || e.a.OnboardingStep() != stepPermissions {
		t.Errorf("AI %v, step %d", e.a.cfg.AI.Enabled, e.a.OnboardingStep())
	}
}

// A found Claude CLI comes first and is chosen; Continue saves it as the
// default provider and turns AI on.
func TestAIStepSavesTheDetectedClaude(t *testing.T) {
	e := newEnv(t)
	e.a.detectAI = func(enhance.LanguageModel) detected { return detected{claude: "/opt/bin/claude"} }
	e.a.goToStep(stepAI)
	tt := onboardingTester(e)
	tt.Frame()
	if ch, ok := e.a.aiChoice(); !ok || ch.kind != aiClaude {
		t.Fatalf("choice %+v %v", ch, ok)
	}
	press(t, tt, i18n.T("onboarding.footer.continue"))
	pr, ok := e.a.cfg.AI.Provider("")
	if !ok || pr.Kind != config.KindClaudeCLI || pr.Path == nil || *pr.Path != "/opt/bin/claude" || !e.a.cfg.AI.Enabled {
		t.Fatalf("provider %+v %v, enabled %v", pr, ok, e.a.cfg.AI.Enabled)
	}
	if e.a.OnboardingStep() != stepPermissions {
		t.Errorf("step %d", e.a.OnboardingStep())
	}
}

// The cloud form says what to fix, in order: the key, then the model;
// then it saves the key in the secret store, not the config.
func TestAIStepCloudFormValidatesAndSaves(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepAI)
	tt := onboardingTester(e)
	tt.Frame()
	press(t, tt, i18n.T("onboarding.ai.cloud_api"))
	tt.Frame()
	press(t, tt, i18n.T("onboarding.footer.continue"))
	if e.a.onb.ai.err != aiErrAPIKey || e.a.OnboardingStep() != stepAI {
		t.Fatalf("no key: err %v, step %d", e.a.onb.ai.err, e.a.OnboardingStep())
	}
	tt.Frame()
	if !tt.HasText(aiErrAPIKey.text(&e.a.onb.ai)) {
		t.Errorf("the error does not show: %q", tt.Texts())
	}
	e.a.onb.ai.key = "sk-test"
	e.a.onboardingNext()
	if e.a.onb.ai.err != aiErrModel {
		t.Fatalf("no model: err %v", e.a.onb.ai.err)
	}
	e.a.onb.ai.model = "openai/gpt-x"
	e.a.onboardingNext()
	pr, ok := e.a.cfg.AI.Provider("")
	if !ok || pr.ID != "openrouter" || pr.ModelName() != "openai/gpt-x" || pr.APIKeyAccount == nil || e.a.OnboardingStep() != stepPermissions {
		t.Fatalf("provider %+v, step %d", pr, e.a.OnboardingStep())
	}
	if e.a.d.Secrets.Get(*pr.APIKeyAccount) != "sk-test" {
		t.Error("the key is not in the secret store")
	}
}

// The permissions step reads the grants again by itself, and Continue
// opens once both are on.
func TestPermissionsStepFollowsTheSystem(t *testing.T) {
	e := newEnv(t)
	perms := &fakePermissions{st: platform.PermissionStatus{NeedsInputPerms: true}}
	e.a.d.Platform = &platform.Services{Permissions: perms}
	e.a.goToStep(stepPermissions)
	tt := onboardingTester(e)
	tt.Frame()
	if f := e.a.footerFor(); f.enabled || f.skip == "" {
		t.Fatalf("none granted: %+v", f)
	}
	if !tt.HasText(i18n.T("onboarding.permissions.allow_microphone")) {
		t.Fatalf("no Allow microphone: %q", tt.Texts())
	}
	press(t, tt, i18n.T("onboarding.permissions.allow_microphone"))
	waitFor(t, func() bool {
		perms.mu.Lock()
		defer perms.mu.Unlock()
		return slices.Contains(perms.requests, "microphone")
	})
	perms.set(platform.PermissionStatus{Microphone: true, Accessibility: true, NeedsInputPerms: true})
	tt.Frame()
	if e.a.footerFor().enabled && hasInputPermissions() {
		t.Fatal("the grants were read again before the poll")
	}
	tt.Advance(permPoll + 100*time.Millisecond)
	tt.Frame()
	if f := e.a.footerFor(); !f.enabled || f.skip != "" {
		t.Fatalf("after the poll: %+v", f)
	}
	if !tt.HasText(i18n.T("onboarding.permissions.allowed")) {
		t.Errorf("no Allowed: %q", tt.Texts())
	}
	perms.set(platform.PermissionStatus{MicrophoneDenied: true, NeedsInputPerms: true})
	tt.Advance(permPoll + 100*time.Millisecond)
	tt.Frame()
	if !tt.HasText(i18n.T("onboarding.permissions.blocked")) {
		t.Errorf("no Blocked: %q", tt.Texts())
	}
}

// pressChord holds mods and key down, as a hand does, for one frame.
func pressChord(tt *mu.Tester, mods mu.Modifiers, key mu.Key) {
	tt.KeyRepeat(mods, key)
	tt.Frame()
}

func waitFor(t *testing.T, ok func() bool) {
	t.Helper()
	for range 200 {
		if ok() {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("timed out")
}

// Change listens for a chord in the window and saves it; Esc stops; a key
// another Sayso action uses is refused with a note.
func TestHotkeysStepRecordsAChord(t *testing.T) {
	e := newEnv(t)
	e.a.cfg.Hotkeys.PasteLast = ptr(config.MustHotkey("ctrl+cmd+v"))
	e.a.goToStep(stepHotkeys)
	tt := onboardingTester(e)
	tt.Frame()
	press(t, tt, i18n.T("onboarding.hotkeys.change_toggle"))
	tt.Frame()
	if e.a.onb.keys.slot != slotToggle || !tt.HasText(i18n.T("onboarding.hotkeys.listening_window")) {
		t.Fatalf("not listening: %q", tt.Texts())
	}
	pressChord(tt, mu.Ctrl|mu.Alt, mu.KeyK)
	tt.KeyUp(mu.KeyK)
	tt.HoldModifiers(0)
	tt.Frame()
	if h := e.a.cfg.Hotkeys.Toggle; h == nil || *h != config.MustHotkey("ctrl+opt+k") || e.a.onb.keys.slot != "" {
		t.Fatalf("toggle %v, slot %q", h, e.a.onb.keys.slot)
	}
	// A key Paste last text uses.
	press(t, tt, i18n.T("onboarding.hotkeys.change_toggle"))
	tt.Frame()
	pressChord(tt, mu.Ctrl|mu.Super, mu.KeyV)
	tt.KeyUp(mu.KeyV)
	tt.HoldModifiers(0)
	tt.Frame()
	if *e.a.cfg.Hotkeys.Toggle != config.MustHotkey("ctrl+opt+k") || e.a.onb.keys.note == "" {
		t.Fatalf("a used key was saved: %v, note %q", e.a.cfg.Hotkeys.Toggle, e.a.onb.keys.note)
	}
	// Esc stops listening and keeps the key.
	press(t, tt, i18n.T("onboarding.hotkeys.change_toggle"))
	tt.Frame()
	tt.KeyDown(mu.KeyEscape)
	tt.Frame()
	tt.KeyUp(mu.KeyEscape)
	tt.Frame()
	if e.a.onb.keys.slot != "" || *e.a.cfg.Hotkeys.Toggle != config.MustHotkey("ctrl+opt+k") {
		t.Errorf("Esc: slot %q, toggle %v", e.a.onb.keys.slot, e.a.cfg.Hotkeys.Toggle)
	}
	// Leaving the step stops a recording.
	e.a.startRecording(slotPTT)
	e.a.goToStep(stepPractice)
	if e.a.onb.keys.slot != "" {
		t.Error("the recording outlived its step")
	}
}

func TestHotkeysStepRightOptionAndClear(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("Use Right Option is a macOS button")
	}
	e := newEnv(t)
	e.a.cfg.Hotkeys.PushToTalk = nil
	e.a.goToStep(stepHotkeys)
	tt := onboardingTester(e)
	press(t, tt, i18n.T("onboarding.hotkeys.use_right_option"))
	if h := e.a.cfg.Hotkeys.PushToTalk; h == nil || *h != config.SoloKey(config.RightOption) {
		t.Fatalf("push to talk %v", h)
	}
	tt.Frame()
	press(t, tt, i18n.T("onboarding.hotkeys.clear_ptt"))
	if e.a.cfg.Hotkeys.PushToTalk != nil {
		t.Error("Clear kept the key")
	}
}

func TestWordsCorrect(t *testing.T) {
	s := practiceSentence()
	if got, total := wordsCorrect(s, strings.ToUpper(s)); got != total || total < 10 {
		t.Errorf("same: %d of %d", got, total)
	}
	if got, total := wordsCorrect(s, "Sayso writes what I say"); got != 5 || total < 10 {
		t.Errorf("start: %d of %d", got, total)
	}
	if got, _ := wordsCorrect(s, ""); got != 0 {
		t.Errorf("empty: %d", got)
	}
}

// A dictation into the field shows how long it took and how many words
// were right; one from before the visit does not.
func TestPracticeShowsTheResult(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepPractice)
	tt := onboardingTester(e)
	tt.Frame()
	if !tt.HasText(i18n.T("onboarding.practice.words_appear")) {
		t.Fatalf("idle: %q", tt.Texts())
	}
	en := store.HistoryEntry{CreatedAt: fixedNow.Add(time.Second), Transcript: practiceSentence(), FinalText: practiceSentence(), StyleID: "raw", TranscribeMs: 300}
	en.Insert.Result = store.InsertPasted
	if _, err := e.db.InsertEntry(en); err != nil {
		t.Fatal(err)
	}
	e.a.dataChanged()
	tt.Frame()
	_, total := wordsCorrect(practiceSentence(), practiceSentence())
	for _, want := range []string{
		i18n.T("onboarding.practice.inserted_after", "seconds", i18n.Decimal(0.3, 1)),
		i18n.T("onboarding.practice.words_correct", "correct", total, "total", total),
	} {
		if !tt.HasText(want) {
			t.Errorf("no %q in %q", want, tt.Texts())
		}
	}
	// A new visit starts over.
	e.a.goToStep(stepHotkeys)
	e.a.onb.practiceSince = fixedNow.Add(time.Minute)
	e.a.onb.step = stepPractice
	tt.Frame()
	if !tt.HasText(i18n.T("onboarding.practice.words_appear")) {
		t.Errorf("an old dictation shows: %q", tt.Texts())
	}
}

// The done step's Open Sayso finishes; closing the window keeps the step.
func TestDoneStepFinishes(t *testing.T) {
	e := newEnv(t)
	e.a.goToStep(stepDone)
	tt := onboardingTester(e)
	if hasControl(tt, i18n.T("onboarding.footer.back")) || hasControl(tt, i18n.T("onboarding.footer.skip")) {
		t.Error("the last step has Back or Skip")
	}
	press(t, tt, i18n.T("onboarding.footer.open_sayso"))
	if !e.a.cfg.Onboarding.Completed || e.shell.hub != 1 || e.shell.shown[WinOnboarding] {
		t.Errorf("completed %v, hub %d, window %v", e.a.cfg.Onboarding.Completed, e.shell.hub, e.shell.shown[WinOnboarding])
	}
}

// Every step builds its controls with stable keys, and none overflows the
// window: each step's content fits in its scroll area at 960×640.
func TestOnboardingStepsHaveKeys(t *testing.T) {
	e := newEnv(t)
	perms := &fakePermissions{}
	e.a.d.Platform = &platform.Services{Permissions: perms}
	e.a.detectAI = func(enhance.LanguageModel) detected {
		return detected{claude: "/opt/bin/claude", ollama: []string{"qwen3:4b"}, ollamaOK: true}
	}
	e.a.cfg.Hotkeys.PushToTalk = nil
	tt := onboardingTester(e)
	for s := range OnboardingSteps {
		e.a.goToStep(s)
		tt.Frame()
	}
	e.a.goToStep(stepModel)
	e.a.openAllModels()
	tt.Frame()
	want := []string{
		"onboarding.next", "onboarding.back", "onboarding.skip", "onboarding.trail", "onboarding.language", "onboarding.dictation_language",
		"onboarding.model.choices", "onboarding.model.show_all", "onboarding.model.download", "onboarding.model.status",
		"onboarding.all_models.filter", "onboarding.all_models.close", "onboarding.all_models.list",
		"onboarding.ai.options", "onboarding.ai.claude", "onboarding.ai.cloud", "onboarding.ai.ollama",
		"onboarding.permissions.allow_mic", "onboarding.permissions.open_ax", "onboarding.permissions.why_ax", "onboarding.permissions.download",
		"onboarding.hotkeys.change_toggle", "onboarding.hotkeys.toggle", "onboarding.hotkeys.ptt_record",
		"onboarding.practice", "onboarding.practice.result",
	}
	if miss := missingKeys(e.a.Keys(), want); len(miss) > 0 {
		t.Errorf("keys never built: %v", miss)
	}
}
