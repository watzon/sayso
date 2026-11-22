package ui

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/store"
)

// requiredKeys are the automation keys the brief names; every one must be
// built by some surface.
var requiredKeys = []string{
	"nav.home", "nav.history", "nav.dictionary", "nav.styles", "nav.models", "nav.settings", "nav.collapse",
	"theme.auto", "theme.light", "theme.dark",
	"settings.back", "settings.nav.general", "settings.nav.dictation", "settings.nav.audio", "settings.nav.providers",
	"settings.nav.overlay", "settings.nav.appearance", "settings.nav.history", "settings.nav.apps", "settings.nav.updates",
	"settings.nav.advanced", "settings.open_config",
	"home.dictate", "home.cancel", "home.change_style",
	"history.search", "history.filter.all", "history.filter.enhanced", "history.filter.audio", "history.copy", "history.copy_meta", "history.delete",
	"dict.filter", "dict.add_word", "dict.add_replacement", "dict.try_input",
	"styles.new", "styles.base_prompt", "styles.editor.save", "styles.editor.cancel",
	"models.search", "overlay.pill", "overlay.notch",
	"popover.toggle", "popover.incognito", "popover.open", "popover.quit",
	"onboarding.next", "onboarding.skip", "whats_new.close", "update.install",
}

func TestEveryRequiredKeyIsBuilt(t *testing.T) {
	e := newEnv(t)
	a := e.a
	e.addEntry(t, "hello world", "Notes", true)
	tt := e.hub(t)
	for _, p := range Pages {
		a.SetPage(p)
		tt.Frame()
	}
	for _, s := range Sections {
		a.OpenSettings(s)
		tt.Frame()
	}
	a.CloseSettings()
	// History with its menu open.
	a.SetPage(PageHistory)
	tt.Frame()
	press(t, tt, i18n.T("history.a11y.more_actions"))
	// Home while recording shows Cancel.
	a.SetPage(PageHome)
	a.state = dictation.State{Kind: dictation.StateRecording}
	tt.Frame()
	a.state = dictation.State{}
	// The style editor.
	a.SetPage(PageStyles)
	tt.Frame()
	press(t, tt, i18n.T("styles.new_style"))
	for _, s := range []struct {
		style config.OverlayStyle
		view  func(*mu.Context)
	}{{config.OverlayPill, a.OverlayView}, {config.OverlayNotch, a.OverlayView}} {
		a.cfg.Overlay.Style = s.style
		mu.NewTester(s.view, PillWindowW, PillWindowH).Frame()
	}
	for _, v := range []func(*mu.Context){a.PopoverView, a.OnboardingView, a.WhatsNewView, a.UpdateView} {
		mu.NewTester(v, OnboardingW, OnboardingH).Frame()
	}
	if miss := missingKeys(a.Keys(), requiredKeys); len(miss) > 0 {
		t.Errorf("keys never built: %v", miss)
	}
}

func TestSidebarNavigatesEveryPage(t *testing.T) {
	e := newEnv(t)
	tt := e.hub(t)
	for _, p := range Pages[:5] {
		press(t, tt, p.title())
		if e.a.Page() != p || e.a.InSettings() {
			t.Fatalf("after %q: page %v, in settings %v", p.title(), e.a.Page(), e.a.InSettings())
		}
		if !tt.HasText(p.title()) {
			t.Errorf("%q page shows %q", p.title(), tt.Texts())
		}
	}
}

func TestSettingsSidebarAndBack(t *testing.T) {
	e := newEnv(t)
	tt := e.hub(t)
	press(t, tt, i18n.T("hub.nav.dictionary"))
	press(t, tt, i18n.T("hub.nav.settings"))
	if !e.a.InSettings() {
		t.Fatal("Settings did not open")
	}
	for _, s := range Sections {
		press(t, tt, s.title())
		if e.a.Section() != s {
			t.Fatalf("after %q: section %v", s.title(), e.a.Section())
		}
		tt.Frame()
		if len(tt.Texts()) < 5 {
			t.Errorf("section %q is nearly empty: %q", s.title(), tt.Texts())
		}
	}
	press(t, tt, i18n.T("hub.sidebar.open_config_file"))
	if len(e.shell.opened) == 0 || e.shell.opened[len(e.shell.opened)-1] != e.paths.ConfigFile() {
		t.Errorf("open config file opened %q", e.shell.opened)
	}
	press(t, tt, i18n.T("hub.sidebar.back"))
	if e.a.InSettings() || e.a.Page() != PageDictionary {
		t.Errorf("back went to page %v, in settings %v", e.a.Page(), e.a.InSettings())
	}
}

func TestNarrowWindowShowsTheRail(t *testing.T) {
	e := newEnv(t)
	tt := mu.NewTester(e.a.HubView, 900, 600)
	if info, _ := tt.Access(i18n.T("hub.nav.history")); info.Bounds.W > railWidth {
		t.Errorf("the rail's History button is %v wide", info.Bounds.W)
	}
	if !hasControl(tt, i18n.T("hub.nav.history")) {
		t.Error("the rail's History button has no name")
	}
	tt.SetSize(HubWidth, HubHeight)
	if info, _ := tt.Access(i18n.T("hub.nav.history")); info.Bounds.W < railWidth {
		t.Errorf("the wide sidebar's History button is %v wide", info.Bounds.W)
	}
	press(t, tt, i18n.T("hub.sidebar.collapse"))
	if !e.a.cfg.Appearance.SidebarCollapsed {
		t.Error("collapse did not set appearance.sidebar_collapsed")
	}
}

func TestThemeControlSavesAndSwitchesPaper(t *testing.T) {
	e := newEnv(t)
	tt := e.hub(t)
	light := tt.Image().At(5, 400)
	press(t, tt, i18n.T("hub.sidebar.theme_dark"))
	if e.a.cfg.Appearance.Theme != config.ThemeDark || e.shell.theme != config.ThemeDark {
		t.Fatalf("theme %q, shell %q", e.a.cfg.Appearance.Theme, e.shell.theme)
	}
	tt.Frame()
	if dark := tt.Image().At(5, 400); dark == light {
		t.Error("the ground did not change color for dark")
	}
	saved, err := config.Load(e.paths.ConfigFile())
	if err != nil || saved.Config.Appearance.Theme != config.ThemeDark {
		t.Errorf("config file theme %q (%v)", saved.Config.Appearance.Theme, err)
	}
	// Auto follows the system.
	press(t, tt, i18n.T("hub.sidebar.theme_auto"))
	tt.SetDark(true)
	if !e.a.dark(true) || e.a.dark(false) {
		t.Error("Auto does not follow the system")
	}
}

func TestInkSwitching(t *testing.T) {
	e := newEnv(t)
	e.a.OpenSettings(SecAppearance)
	tt := e.hub(t)
	press(t, tt, "Indigo")
	if e.a.cfg.Appearance.Ink.Named != config.InkIndigo {
		t.Errorf("ink %v", e.a.cfg.Appearance.Ink)
	}
	press(t, tt, i18n.T("settings.appearance.custom_ink"))
	tt.Type("#2D3C8C")
	press(t, tt, i18n.T("settings.appearance.custom"))
	if !e.a.cfg.Appearance.Ink.IsCustom || e.a.cfg.Appearance.Ink.Custom != 0x2D3C8C {
		t.Errorf("custom ink %+v", e.a.cfg.Appearance.Ink)
	}
	press(t, tt, i18n.T("settings.appearance.custom_ink"))
	tt.Key(mu.Cmd, mu.KeyA)
	tt.Type("blue")
	press(t, tt, i18n.T("settings.appearance.custom"))
	tt.Frame()
	if !tt.HasText(i18n.T("settings.appearance.hex_error")) {
		t.Error("a bad hex shows no error")
	}
}

func TestLocaleSwitching(t *testing.T) {
	e := newEnv(t)
	tt := e.hub(t)
	for _, c := range []struct{ code, home string }{{"es", "Inicio"}, {"ja", "ホーム"}, {"en", "Home"}} {
		e.a.SetLanguage(c.code)
		tt.Frame()
		if !tt.HasText(c.home) {
			t.Errorf("%s: no %q in %q", c.code, c.home, tt.Texts()[:8])
		}
	}
	e.a.SetLanguage(i18n.Pseudo)
	tt.Frame()
	pseudo := false
	for _, s := range tt.Texts() {
		if strings.HasPrefix(s, "[") && strings.HasSuffix(s, "]") {
			pseudo = true
		}
	}
	if !pseudo {
		t.Errorf("en-XA shows no pseudo text: %q", tt.Texts()[:8])
	}
}

func TestHomeShowsGreetingStatsAndRecent(t *testing.T) {
	e := newEnv(t)
	e.a.cfg.General.Name = ptr("Ada")
	e.a.cfg.Hotkeys.Toggle = ptr(config.MustHotkey("opt+space"))
	id := e.addEntry(t, "the quick brown fox jumps", "Notes", false)
	tt := e.hub(t)
	if !tt.HasText("Good afternoon, Ada.") {
		t.Errorf("greeting missing: %q", tt.Texts())
	}
	if !tt.HasText("5") {
		t.Errorf("words today missing: %q", tt.Texts())
	}
	press(t, tt, i18n.T("popover.start_dictation"))
	if e.dict.count("toggle") != 1 {
		t.Errorf("dictate called %v", e.dict.calls)
	}
	e.a.state = dictation.State{Kind: dictation.StateRecording}
	tt.Frame()
	press(t, tt, i18n.T("styles.cancel"))
	if e.dict.count("cancel") != 1 {
		t.Errorf("cancel called %v", e.dict.calls)
	}
	e.a.state = dictation.State{}
	tt.Frame()
	press(t, tt, "Notes, the quick brown fox jumps")
	if e.a.Page() != PageHistory || e.a.hist.selected != id {
		t.Errorf("recent row went to page %v, entry %d", e.a.Page(), e.a.hist.selected)
	}
}

func TestHomeHistoryOffCard(t *testing.T) {
	e := newEnv(t)
	e.a.cfg.History.Enabled = false
	tt := e.hub(t)
	if !tt.HasText(i18n.T("home.history_off.title")) {
		t.Fatalf("no history-off card: %q", tt.Texts())
	}
	press(t, tt, i18n.T("home.history_off.turn_on"))
	if !e.a.cfg.History.Enabled {
		t.Error("turn on did not turn history on")
	}
}

func TestHistorySearchFilterMenuAndDelete(t *testing.T) {
	e := newEnv(t)
	e.addEntry(t, "first apple note", "Notes", false)
	e.addEntry(t, "second banana mail", "Mail", true)
	e.a.SetPage(PageHistory)
	tt := e.hub(t)
	if !tt.HasText("first apple note") || !tt.HasText("second banana mail") {
		t.Fatalf("entries missing: %q", tt.Texts())
	}
	press(t, tt, i18n.T("history.filter_audio"))
	tt.Frame()
	if tt.HasText("first apple note") {
		t.Error("Has audio shows an entry without audio")
	}
	press(t, tt, i18n.T("history.filter_all"))
	e.a.hist.query = "apple"
	tt.Frame()
	if tt.HasText("second banana mail") {
		t.Error("search shows an entry without the word")
	}
	e.a.hist.query = ""
	tt.Frame()
	e.a.selectEntry(e.a.hist.entries[0].ID)
	tt.Frame()
	press(t, tt, i18n.T("history.a11y.more_actions"))
	press(t, tt, i18n.T("history.menu.copy"))
	sel, _ := e.a.selectedEntry()
	if tt.Clipboard() != sel.FinalText {
		t.Errorf("clipboard %q, want %q", tt.Clipboard(), sel.FinalText)
	}
	press(t, tt, i18n.T("history.a11y.more_actions"))
	press(t, tt, i18n.T("history.menu.copy_metadata"))
	if !strings.Contains(tt.Clipboard(), "{") {
		t.Errorf("metadata %q", tt.Clipboard())
	}
	press(t, tt, i18n.T("history.a11y.more_actions"))
	press(t, tt, i18n.T("history.delete"))
	if !tt.HasText(i18n.T("history.confirm_delete")) {
		t.Fatal("no delete confirmation")
	}
	press(t, tt, i18n.T("history.delete"))
	if n, _ := e.db.Count(store.HistoryQuery{}); n != 1 {
		t.Errorf("%d entries after delete, want 1", n)
	}
}

type fakePlayer struct{ playing bool }

func (f *fakePlayer) Play()                   { f.playing = true }
func (f *fakePlayer) Pause()                  { f.playing = false }
func (f *fakePlayer) Seek(time.Duration)      {}
func (f *fakePlayer) Position() time.Duration { return 0 }
func (f *fakePlayer) Duration() time.Duration { return time.Second }
func (f *fakePlayer) Playing() bool           { return f.playing }
func (f *fakePlayer) Close()                  {}

func TestHistoryPlayerAndTrail(t *testing.T) {
	e := newEnv(t)
	pl := &fakePlayer{}
	e.a.d.NewPlayer = func([]float32) Player { return pl }
	e.addEntry(t, "with audio", "Notes", true)
	e.a.SetPage(PageHistory)
	tt := e.hub(t)
	if !tt.HasText(strings.ToUpper(i18n.T("history.trail.title"))) {
		t.Errorf("no trail: %q", tt.Texts())
	}
	press(t, tt, i18n.T("history.a11y.play"))
	if !pl.playing {
		t.Error("play did not play")
	}
	tt.Frame()
	press(t, tt, i18n.T("history.a11y.pause"))
	if pl.playing {
		t.Error("pause did not pause")
	}
}

func TestDictionaryWordsRulesAndTry(t *testing.T) {
	e := newEnv(t)
	e.a.SetPage(PageDictionary)
	tt := e.hub(t)
	press(t, tt, i18n.T("dictionary.placeholder.new_word"))
	tt.Type("Sayso")
	press(t, tt, i18n.T("dictionary.add_word"))
	if w, _ := e.db.ListWords(); len(w) != 1 || w[0].Text != "Sayso" {
		t.Fatalf("words %v", w)
	}
	press(t, tt, i18n.T("dictionary.placeholder.from"))
	tt.Type("git hub")
	press(t, tt, i18n.T("dictionary.placeholder.to"))
	tt.Type("GitHub")
	press(t, tt, i18n.T("dictionary.add_rule"))
	if r, _ := e.db.ListReplacements(); len(r) != 1 || r[0].To != "GitHub" {
		t.Fatalf("rules %v", r)
	}
	press(t, tt, i18n.T("dictionary.try.title"))
	tt.Type("I pushed it to git hub")
	if !tt.HasText("I pushed it to GitHub") {
		t.Errorf("try a phrase: %q", tt.Texts())
	}
	press(t, tt, i18n.T("dictionary.placeholder.new_word"))
	tt.Type("sayso")
	press(t, tt, i18n.T("dictionary.add_word"))
	tt.Frame()
	if !tt.HasText(i18n.T("dictionary.error.word_exists", "word", "sayso")) {
		t.Error("a duplicate word shows no error")
	}
}

func TestStylesEditorSavesAFile(t *testing.T) {
	e := newEnv(t)
	e.a.SetPage(PageStyles)
	tt := mu.NewTester(e.a.HubView, HubWidth, 1800) // tall enough for every style row
	press(t, tt, i18n.T("styles.new_style"))
	if !e.a.sty.editing {
		t.Fatal("the editor did not open")
	}
	press(t, tt, i18n.T("styles.save"))
	tt.Frame()
	if !tt.HasText(i18n.T("styles.error.needs_name")) {
		t.Error("saving without a name shows no error")
	}
	press(t, tt, i18n.T("styles.editor.name"))
	tt.Type("My Slack")
	press(t, tt, i18n.T("styles.editor.prompt"))
	tt.Type("Keep it short.")
	press(t, tt, i18n.T("styles.save"))
	if e.a.sty.editing {
		t.Fatalf("the editor stayed open: %s", e.a.sty.err)
	}
	if _, err := os.Stat(filepath.Join(e.paths.StylesDir(), "my-slack.toml")); err != nil {
		t.Fatal(err)
	}
	if _, ok := e.a.styles.Get("my-slack"); !ok {
		t.Error("the library did not reload the new style")
	}
	tt.Frame()
	press(t, tt, i18n.T("styles.a11y.make_default", "name", "My Slack"))
	if e.a.ActiveStyle() != "my-slack" {
		t.Errorf("default style %q", e.a.ActiveStyle())
	}
	press(t, tt, i18n.T("styles.base.button"))
	if !e.a.sty.baseOpen {
		t.Error("the base prompt drawer did not open")
	}
}

func TestModelsSearchSortAndDownload(t *testing.T) {
	e := newEnv(t)
	e.a.SetPage(PageModels)
	tt := e.hub(t)
	all := len(e.a.modelList(0))
	if all == 0 {
		t.Fatal("no local models in the catalog")
	}
	e.a.mdl.query = "whisper"
	tt.Frame()
	for _, m := range e.a.modelList(0) {
		if !strings.Contains(strings.ToLower(m.Name+string(m.ID)+m.Vendor), "whisper") {
			t.Errorf("search kept %s", m.ID)
		}
	}
	press(t, tt, i18n.T("models.group.size"))
	list := e.a.modelList(0)
	for i := 1; i < len(list); i++ {
		if list[i].SizeBytes < list[i-1].SizeBytes {
			t.Fatalf("not sorted by size at %d", i)
		}
	}
	m := list[0]
	e.a.SetPage(PageModels)
	tt.Frame()
	press(t, tt, i18n.T("models.download_size", "size", modelsFormat(m.SizeBytes)))
	deadline := time.Now().Add(2 * time.Second)
	for e.eng.downloads() == 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if e.eng.downloads() == 0 {
		t.Error("download did not reach the engine")
	}
	e.eng.setState(string(m.ID), engine.ModelStatus{State: engine.StateReady})
	tt.Frame()
	press(t, tt, i18n.T("models.row.use"))
	if e.a.cfg.Dictation.Model == "" {
		t.Error("use did not choose the model")
	}
}

func TestSettingsSwitchesSave(t *testing.T) {
	e := newEnv(t)
	e.a.OpenSettings(SecOverlay)
	tt := e.hub(t)
	press(t, tt, i18n.T("settings.overlay.style_notch"))
	if e.a.cfg.Overlay.Style != config.OverlayNotch {
		t.Fatal("notch not chosen")
	}
	tt.Frame()
	before := e.a.cfg.Overlay.NotchOnOtherDisplays
	press(t, tt, i18n.T("settings.overlay.notch_other_displays"))
	tt.Frame()
	if e.a.cfg.Overlay.NotchOnOtherDisplays == before {
		t.Error("the switch did not change notch_on_other_displays")
	}
	press(t, tt, i18n.T("settings.overlay.size_large"))
	saved, _ := config.Load(e.paths.ConfigFile())
	if saved.Config.Overlay.Size != config.OverlayLarge || saved.Config.Overlay.Style != config.OverlayNotch {
		t.Errorf("saved overlay %+v", saved.Config.Overlay)
	}
}

func TestClearHistoryAsksFirst(t *testing.T) {
	e := newEnv(t)
	e.addEntry(t, "one", "Notes", false)
	e.a.OpenSettings(SecHistory)
	tt := e.hub(t)
	press(t, tt, i18n.T("settings.history.clear_history"))
	if n, _ := e.db.Count(store.HistoryQuery{}); n != 1 {
		t.Fatal("cleared without asking")
	}
	press(t, tt, i18n.T("settings.history.delete_all"))
	if n, _ := e.db.Count(store.HistoryQuery{}); n != 0 {
		t.Errorf("%d entries after clear", n)
	}
}

func TestProvidersAddAndRemove(t *testing.T) {
	e := newEnv(t)
	e.a.OpenSettings(SecProviders)
	tt := e.hub(t)
	for _, f := range [][2]string{{i18n.T("settings.providers.name"), "My Server"}, {i18n.T("settings.providers.base_url"), "http://localhost:8080/v1"}, {i18n.T("settings.providers.api_key"), "sk-test"}} {
		press(t, tt, f[0])
		tt.Type(f[1])
	}
	press(t, tt, i18n.T("settings.providers.add_provider"))
	pr, ok := e.a.cfg.AI.Provider("my-server")
	if !ok || pr.APIKeyAccount == nil || e.a.d.Secrets.Get(*pr.APIKeyAccount) != "sk-test" {
		t.Fatalf("provider %+v, %v", pr, ok)
	}
	tt.Frame()
	press(t, tt, i18n.T("settings.providers.remove"))
	if len(e.a.cfg.AI.Providers) != 0 || e.a.d.Secrets.Get("provider.my-server") != "" {
		t.Error("remove left the provider or its key")
	}
}

func TestOverlayPillStates(t *testing.T) {
	e := newEnv(t)
	a := e.a
	a.cfg.Overlay.IdlePill = true
	a.cfg.Hotkeys.Toggle = ptr(config.MustHotkey("opt+space"))
	tt := mu.NewTester(a.OverlayView, PillWindowW, PillWindowH)
	if !tt.HasText(i18n.T("overlay.to_dictate")) {
		t.Errorf("idle pill: %q", tt.Texts())
	}
	press(t, tt, i18n.T("overlay.a11y.start"))
	if e.dict.count("toggle") != 1 {
		t.Error("the idle pill does not start a dictation")
	}
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	if !e.shell.overlay || !a.OverlayClickThrough() {
		t.Error("recording should show the overlay and let clicks through")
	}
	a.cfg.Overlay.ShowPreview = true
	a.Preview("hello", "wor")
	tt.Frame()
	if !hasControl(tt, i18n.T("overlay.a11y.stop")) {
		t.Errorf("recording pill: %q", tt.Texts())
	}
	a.StateChanged(dictation.State{Kind: dictation.StateProcessing})
	tt.Frame()
	if !tt.HasText(i18n.T("overlay.stage.transcribing")) {
		t.Errorf("processing pill: %q", tt.Texts())
	}
	a.StateChanged(dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeCancelled}})
	tt.Frame()
	press(t, tt, i18n.T("overlay.undo"))
	if e.dict.count("undo") != 1 {
		t.Error("undo did not reach the controller")
	}
	a.StateChanged(dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeInserted, StyleName: "Clean"}})
	tt.Frame()
	if !tt.HasText(i18n.T("overlay.inserted") + " · Clean") {
		t.Errorf("done pill: %q", tt.Texts())
	}
	a.StateChanged(dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeError, Message: "Mic broke"}})
	tt.Frame()
	if !tt.HasText("Mic broke") {
		t.Errorf("failed pill: %q", tt.Texts())
	}
	a.Notice(dictation.Message{Key: "overlay.notice.no_engine"})
	a.StateChanged(dictation.State{Kind: dictation.StateIdle})
	tt.Frame()
	if !tt.HasText(i18n.T("overlay.notice.no_engine")) {
		t.Errorf("an idle notice does not show: %q", tt.Texts())
	}
	a.lastNotice = ""
	a.cfg.Overlay.IdlePill = false
	a.StateChanged(dictation.State{Kind: dictation.StateIdle})
	if e.shell.overlay {
		t.Error("idle without the idle pill should hide the overlay")
	}
}

func TestOverlaySizesZoom(t *testing.T) {
	e := newEnv(t)
	e.a.cfg.Overlay.IdlePill = true
	e.a.cfg.Overlay.Size = config.OverlayMedium
	tt := mu.NewTester(e.a.OverlayView, PillWindowW, PillWindowH)
	medium := inkWidth(tt)
	e.a.cfg.Overlay.Size = config.OverlayLarge
	tt.Frame()
	large := inkWidth(tt)
	e.a.cfg.Overlay.Size = config.OverlaySmall
	tt.Frame()
	small := inkWidth(tt)
	if !(small < medium && medium < large) {
		t.Errorf("drawn widths small %d, medium %d, large %d", small, medium, large)
	}
}

// inkWidth is the width of what the transparent overlay draws, in pixels.
func inkWidth(tt *mu.Tester) int {
	img := tt.Image()
	b := img.Bounds()
	lo, hi := b.Max.X, b.Min.X
	for y := b.Min.Y; y < b.Max.Y; y++ {
		for x := b.Min.X; x < b.Max.X; x++ {
			if img.RGBAAt(x, y).A > 0 {
				lo, hi = min(lo, x), max(hi, x)
			}
		}
	}
	return hi - lo
}

func TestOverlayNotch(t *testing.T) {
	e := newEnv(t)
	a := e.a
	a.cfg.Overlay.Style = config.OverlayNotch
	a.cfg.Overlay.IdlePill = true
	if a.OverlayVisible() {
		t.Error("the notch shows while idle")
	}
	a.StateChanged(dictation.State{Kind: dictation.StateRecording})
	tt := mu.NewTester(a.OverlayView, NotchWindowW, NotchWindowH)
	press(t, tt, i18n.T("overlay.a11y.name"))
	if e.dict.count("toggle") != 1 {
		t.Error("clicking the notch does not stop")
	}
	a.StateChanged(dictation.State{Kind: dictation.StateShowing, Notice: dictation.Notice{Kind: dictation.NoticeInserted}})
	tt.Frame()
	if !tt.HasText(i18n.T("overlay.inserted")) {
		t.Errorf("notch slip: %q", tt.Texts())
	}
}

func TestPopover(t *testing.T) {
	e := newEnv(t)
	tt := mu.NewTester(e.a.PopoverView, PopoverW, PopoverH)
	press(t, tt, i18n.T("popover.start_dictation"))
	if e.dict.count("toggle") != 1 {
		t.Error("popover toggle")
	}
	press(t, tt, i18n.T("styles.builtin.email.name"))
	if e.a.ActiveStyle() != "email" {
		t.Errorf("style %q", e.a.ActiveStyle())
	}
	press(t, tt, i18n.T("popover.incognito"))
	tt.Frame()
	if !e.a.Incognito() || !e.a.ControllerIncognito() {
		t.Error("incognito did not turn on for the controller")
	}
	press(t, tt, i18n.T("popover.open_sayso"))
	if e.shell.hub != 1 {
		t.Error("open did not show the hub")
	}
	press(t, tt, i18n.T("popover.quit"))
	if !e.shell.quit {
		t.Error("quit did not quit")
	}
}

func TestOnboardingWalksSevenSteps(t *testing.T) {
	e := newEnv(t)
	tt := mu.NewTester(e.a.OnboardingView, OnboardingW, OnboardingH)
	press(t, tt, i18n.T("onboarding.footer.get_started"))
	for i := 1; i < OnboardingSteps-1; i++ {
		if e.a.OnboardingStep() != i {
			t.Fatalf("step %d, want %d", e.a.OnboardingStep(), i)
		}
		tt.Frame()
		press(t, tt, i18n.T("onboarding.footer.skip"))
	}
	tt.Frame()
	if !tt.HasText(i18n.T("onboarding.done.title")) {
		t.Fatalf("last step: %q", tt.Texts())
	}
	press(t, tt, i18n.T("onboarding.footer.open_sayso"))
	if !e.a.cfg.Onboarding.Completed || e.shell.shown[WinOnboarding] || e.shell.hub != 1 {
		t.Errorf("finish: completed %v, window %v, hub %d", e.a.cfg.Onboarding.Completed, e.shell.shown[WinOnboarding], e.shell.hub)
	}
}

func TestWhatsNewShowsAndCloses(t *testing.T) {
	e := newEnv(t)
	if !e.a.HasWhatsNew() {
		t.Fatal("notes for 0.16.0 not found")
	}
	tt := mu.NewTester(e.a.WhatsNewView, WhatsNewW, WhatsNewH)
	if !tt.HasText("Sayso on Go") || !tt.HasText("A new engine room") {
		t.Errorf("what's new: %q", tt.Texts())
	}
	e.shell.shown[WinWhatsNew] = true
	press(t, tt, i18n.T("whats_new.done"))
	if e.shell.shown[WinWhatsNew] {
		t.Error("done did not close the window")
	}
	if _, err := os.Stat(e.paths.StateFile()); err != nil {
		t.Errorf("seen not saved: %v", err)
	}
}

type fakeUpdater struct{ checks, installs int }

func (f *fakeUpdater) CheckNow()         { f.checks++ }
func (f *fakeUpdater) Install()          { f.installs++ }
func (f *fakeUpdater) SetAutoCheck(bool) {}
func (f *fakeUpdater) SetAutomatic(bool) {}

func TestUpdateWindowAndCheck(t *testing.T) {
	e := newEnv(t)
	u := &fakeUpdater{}
	e.a.SetUpdater(u)
	e.a.OpenSettings(SecUpdates)
	tt := e.hub(t)
	pressLast(t, tt, i18n.T("update.action.check")) // the switch above has the same name
	if u.checks != 1 {
		t.Error("check now did not check")
	}
	e.a.upd.status.Kind = 2 // available
	tt2 := mu.NewTester(e.a.UpdateView, UpdateW, UpdateH)
	press(t, tt2, i18n.T("update.action.update_now"))
	if u.installs != 1 {
		t.Error("install did not install")
	}
}

func TestApprovalDialogAnswers(t *testing.T) {
	e := newEnv(t)
	ask := &approvalAsk{q: bridge.ApprovalQuestion{Name: "Notes Helper", Capabilities: []bridge.Capability{bridge.CapAudioMic}}, answer: make(chan bool, 1)}
	e.a.asks = append(e.a.asks, ask)
	tt := mu.NewTester(e.a.ApprovalView, ApprovalW, ApprovalH)
	if !tt.HasText(i18n.T("apps.approval.title", "app", "Notes Helper")) || !tt.HasText(i18n.T("apps.capability.audio_mic")) {
		t.Errorf("approval: %q", tt.Texts())
	}
	press(t, tt, i18n.T("apps.approval.allow"))
	if v := <-ask.answer; !v {
		t.Error("allow answered no")
	}
	if len(e.a.asks) != 0 {
		t.Error("the question stayed")
	}
}

func TestDarkAndLightRenderEverySurface(t *testing.T) {
	e := newEnv(t)
	var out strings.Builder
	if err := Smoke(e.a, &out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "80 renders ok") {
		t.Errorf("smoke: %s", out.String())
	}
}

func TestComputeStats(t *testing.T) {
	now := time.Date(2026, 11, 18, 12, 0, 0, 0, time.Local) // a Wednesday
	entries := []store.StatsEntry{
		{CreatedAt: now.Add(-time.Hour), DurationMs: 30000, Words: 80},   // today: 2 min typing, 0.5 speaking
		{CreatedAt: now.AddDate(0, 0, -2), DurationMs: 30000, Words: 40}, // Monday
		{CreatedAt: now.AddDate(0, 0, -6), DurationMs: 0, Words: 999},    // last week, no duration
	}
	s := ComputeStats(entries, now)
	if s.WordsToday != 80 || s.MinutesSavedToday != 2 || s.TodayIndex != 2 {
		t.Errorf("today: %+v", s)
	}
	if s.WeekByDay[0] != 40 || s.WeekByDay[2] != 80 || s.WordsThisWeek != 120 {
		t.Errorf("week: %+v", s.WeekByDay)
	}
	if s.PaceWPM != 120 {
		t.Errorf("pace %d, want 120", s.PaceWPM)
	}
	if ComputeStats(nil, now).PaceWPM != 0 {
		t.Error("pace without data")
	}
}

func TestParseAndRunCommands(t *testing.T) {
	cases := map[string]Command{
		"--toggle": {Kind: CmdToggle}, "--cancel": {Kind: CmdCancel}, "--paste-last": {Kind: CmdPasteLast},
		"--cycle-style": {Kind: CmdCycleStyle}, "--incognito": {Kind: CmdIncognito}, "--style=email": {Kind: CmdStyle, Style: "email"},
		"--push-to-talk-down": {Kind: CmdPushToTalkDown}, "--push-to-talk-up": {Kind: CmdPushToTalkUp},
		"--push-to-talk-up-submit": {Kind: CmdPushToTalkUpSubmit}, "--open": {Kind: CmdOpen}, "--style=": {}, "--bogus": {},
	}
	for arg, want := range cases {
		if got := ParseCommand([]string{arg}); got != want {
			t.Errorf("%s: %+v, want %+v", arg, got, want)
		}
	}
	e := newEnv(t)
	for _, c := range []Command{{Kind: CmdToggle}, {Kind: CmdCancel}, {Kind: CmdStyle, Style: "notes"}, {Kind: CmdPushToTalkDown}, {Kind: CmdPushToTalkUp}} {
		e.a.RunCommand(c)
	}
	want := []string{"toggle", "cancel", "style:notes", "ptt-down", "ptt-up"}
	if strings.Join(e.dict.calls, ",") != strings.Join(want, ",") {
		t.Errorf("calls %v, want %v", e.dict.calls, want)
	}
	before := e.a.ActiveStyle()
	e.a.RunCommand(Command{Kind: CmdCycleStyle})
	if e.a.ActiveStyle() == before {
		t.Error("cycle style did not change the style")
	}
	e.a.RunCommand(Command{Kind: CmdIncognito})
	if !e.a.Incognito() {
		t.Error("incognito command")
	}
	e.a.RunCommand(Command{Kind: CmdOpen})
	if e.shell.hub != 1 {
		t.Error("open command")
	}
}

func TestHotkeyConfigOfConfig(t *testing.T) {
	cfg := config.Default()
	cfg.Hotkeys.Toggle = ptr(config.MustHotkey("opt+space"))
	cfg.Hotkeys.PushToTalk = nil
	hc := HotkeyConfig(&cfg)
	if hc.Toggle == "" || hc.PushToTalk != "" || hc.CancelMode != string(cfg.Hotkeys.Cancel) {
		t.Errorf("%+v", hc)
	}
}

func modelsFormat(n uint64) string { return models.FormatSize(n) }

// devicesStub lists fixed microphones.
type devicesStub struct{ micStub }

func (devicesStub) ListDevices() ([]platform.AudioDevice, error) {
	return []platform.AudioDevice{{ID: "BuiltInMicrophoneDevice", Name: "MacBook Pro Microphone"}, {ID: "USBMic", Name: "Desk Mic"}}, nil
}

// The Rust app saved input_device as "coreaudio:<UID>"; Settings showed
// "System default" because it compared that with the bare UID.
func TestSettingsAudioShowsADeviceSavedByTheRustApp(t *testing.T) {
	e := newEnv(t)
	e.a.d.Platform = &platform.Services{Audio: devicesStub{}}
	e.a.cfg.Audio.InputDevice = ptr("coreaudio:USBMic")
	e.a.OpenSettings(SecAudio)
	tt := e.hub(t)
	for _, a := range tt.AccessAll() {
		if a.Name == i18n.T("settings.audio.input") && a.Role != mu.RoleText {
			if a.Value != "Desk Mic" {
				t.Fatalf("input device shows %q, want Desk Mic", a.Value)
			}
			return
		}
	}
	t.Fatalf("no input device control; texts %q", tt.Texts())
}
