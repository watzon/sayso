package ui

import (
	"log"
	"os"
	"strings"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/paths"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
)

// Page is a page of the hub.
type Page int

const (
	PageHome Page = iota
	PageHistory
	PageDictionary
	PageStyles
	PageModels
	PageSettings
)

// Pages are the pages of the main sidebar, in order.
var Pages = []Page{PageHome, PageHistory, PageDictionary, PageStyles, PageModels, PageSettings}

func (p Page) key() string {
	return [...]string{"nav.home", "nav.history", "nav.dictionary", "nav.styles", "nav.models", "nav.settings"}[p]
}

func (p Page) title() string {
	return i18n.T([...]string{"hub.nav.home", "hub.nav.history", "hub.nav.dictionary", "hub.nav.styles", "hub.nav.models", "hub.nav.settings"}[p])
}

func (p Page) icon() string {
	return [...]string{"home", "history", "dictionary", "styles", "models", "settings"}[p]
}

// Section is a section of Settings.
type Section int

const (
	SecGeneral Section = iota
	SecDictation
	SecAudio
	SecProviders
	SecOverlay
	SecAppearance
	SecHistory
	SecApps
	SecUpdates
	SecAdvanced
	SecPermissions
)

// Sections are the Settings sections, in sidebar order: the Rust app's
// (hub/settings), with Updates, which the Go port keeps as a page, before
// Advanced.
var Sections = []Section{SecGeneral, SecDictation, SecProviders, SecOverlay, SecAudio, SecAppearance, SecHistory, SecPermissions, SecApps, SecUpdates, SecAdvanced}

func (s Section) key() string {
	return "settings.nav." + [...]string{"general", "dictation", "audio", "providers", "overlay", "appearance", "history", "apps", "updates", "advanced", "permissions"}[s]
}

func (s Section) title() string {
	if s == SecUpdates {
		return i18n.T("settings.general.updates")
	}
	return i18n.T("hub.settings_nav." + [...]string{"general", "dictation", "audio", "providers", "overlay", "appearance", "history_privacy", "connected_apps", "", "advanced", "permissions"}[s])
}

func (s Section) icon() string {
	return [...]string{"general", "mic", "audio", "sparkle", "overlay", "drop", "shield", "apps", "download", "code", "lock"}[s]
}

// Engine is what the interface needs of the speech engine. EngineAdapter
// implements it over engine.Client.
type Engine interface {
	dictation.Engine
	// Running is false while the engine starts, after a crash, or when it
	// is not installed.
	Running() bool
	// Download starts a download; Status shows its progress.
	Download(model string) error
	Delete(model string) error
}

// Dictation is the dictation controller as the interface drives it.
// *dictation.Controller implements it.
type Dictation interface {
	Toggle()
	Cancel()
	PushToTalkDown()
	PushToTalkUp()
	PickStyle(id string)
	Dismiss()
	Undo()
	LastText() string
}

// Deps is what App is built from. Every field but Config may be zero:
// the views show what is missing.
type Deps struct {
	Paths      paths.Paths
	ConfigPath string // "" never saves (tests)
	Config     config.Config
	Issues     []config.Issue
	Store      *store.Store
	Platform   *platform.Services
	Engine     Engine
	Dictation  Dictation
	// Styler runs a style again for History; nil hides nothing but does nothing.
	Styler dictation.Styler
	// Updater checks for and installs updates; nil in builds that do not.
	Updater Updater
	// NewPlayer makes the player of a recording (16 kHz mono); nil cannot play.
	NewPlayer func(samples []float32) Player
	Secrets   providers.SecretStore
	Apps      *bridge.AppsFile
	Host      *bridge.Host
	Version   string
	// ReleaseNotes is the text of release-notes.toml.
	ReleaseNotes string
	// Post runs fn on the UI thread; nil runs it at once.
	Post func(fn func())
	// Now is the clock; nil is time.Now.
	Now func() time.Time
	// Shell does what needs the desktop; nil does nothing.
	Shell Shell
}

// Shell is what App asks of the desktop: windows and the system.
type Shell interface {
	Invalidate()
	ShowHub()
	ShowOverlay(show bool)
	ShowWindow(w Window, show bool)
	SetThemeMode(m config.ThemeMode)
	SetOverlaySize(s config.OverlaySize)
	OpenPath(path string)
	OpenURL(url string)
	Quit()
}

// Window is a secondary window.
type Window int

const (
	WinOnboarding Window = iota
	WinPermissions
	WinWhatsNew
	WinUpdate
	WinApproval
	WinPopover
)

// App is the model behind every view.
type App struct {
	d      Deps
	cfg    config.Config
	styles *enhance.StyleLibrary
	themes themeCache
	icons  appIcons

	// TrackKeys records the key of every element App.k builds (tests).
	TrackKeys bool
	keys      map[string]bool

	page         Page
	section      Section
	inSettings   bool
	secrets      secretsCache
	settingsFrom Page

	// Dictation, as the controller last reported it.
	state      dictation.State
	stateAt    time.Time
	wave       liveWave
	committed  string
	tentative  string
	lastNotice string
	noticeAt   time.Time
	incognito  bool

	home  homeState
	hist  historyState
	dict  dictState
	sty   stylesState
	mdl   modelsState
	set   settingsState
	ov    overlayState
	pop   popoverState
	onb   onboardingState
	news  whatsNewState
	upd   updateState
	asks  []*approvalAsk
	flash string // a one-line notice under the page title

	onbPracticeText string
	snap            snapshot
	// onConfig hears each published config (on the main goroutine).
	onConfig func(*config.Config)
}

// New builds the model.
func New(d Deps) *App {
	a := &App{d: d, cfg: d.Config}
	if a.d.Now == nil {
		a.d.Now = time.Now
	}
	if a.d.Version == "" {
		a.d.Version = "0.0.0"
	}
	a.reloadStyles()
	a.applyLocale()
	a.set.name = derefOr(a.cfg.General.Name, "")
	a.set.enginePath = derefOr(a.cfg.Advanced.EnginePath, "")
	a.onb.step = int(a.cfg.Onboarding.Step)
	a.news.load(a.d.ReleaseNotes, a.d.Version)
	a.publish()
	return a
}

// SetDictation sets the controller, which needs App as its observer and
// so comes after New.
func (a *App) SetDictation(d Dictation) { a.d.Dictation = d }

// SetEngine sets the speech engine once it started.
func (a *App) SetEngine(e Engine) { a.d.Engine = e }

// SetHost sets the Host API server.
func (a *App) SetHost(h *bridge.Host) { a.d.Host = h }

// SetStyler sets what History's Enhance runs.
func (a *App) SetStyler(s dictation.Styler) { a.d.Styler = s }

// OnConfig sets what hears each config the app publishes (on the main
// goroutine, after every change). It must not block.
func (a *App) OnConfig(fn func(*config.Config)) { a.onConfig = fn }

// SetUpdater sets the updater.
func (a *App) SetUpdater(u Updater) { a.d.Updater = u }

// Config is the current config.
func (a *App) Config() *config.Config { return &a.cfg }

// Page is the page the hub shows.
func (a *App) Page() Page { return a.page }

// InSettings is true while the hub shows Settings.
func (a *App) InSettings() bool { return a.inSettings }

// Section is the Settings section shown.
func (a *App) Section() Section { return a.section }

// Keys are the keys App.k built since TrackKeys was turned on.
func (a *App) Keys() map[string]bool { return a.keys }

func (a *App) now() time.Time { return a.d.Now() }

func (a *App) post(fn func()) {
	if a.d.Post != nil {
		a.d.Post(fn)
		return
	}
	fn()
}

// invalidate redraws every window.
func (a *App) invalidate() {
	if a.d.Shell != nil {
		a.d.Shell.Invalidate()
	}
}

// overlayInvalidator is a shell that can redraw the overlay alone.
type overlayInvalidator interface{ InvalidateOverlay() }

// invalidateOverlay redraws the overlay, or every window when the shell
// cannot redraw it alone.
func (a *App) invalidateOverlay() {
	if s, ok := a.d.Shell.(overlayInvalidator); ok {
		s.InvalidateOverlay()
		return
	}
	a.invalidate()
}

// k gives the next element its stable key, for automation and identity.
func (a *App) k(c *mu.Context, key string) *mu.Context {
	if a.TrackKeys {
		if a.keys == nil {
			a.keys = map[string]bool{}
		}
		a.keys[key] = true
	}
	return c.Key(key)
}

func (a *App) saveConfig() {
	a.publish()
	if a.d.ConfigPath == "" {
		return
	}
	if err := a.cfg.Save(a.d.ConfigPath); err != nil {
		log.Printf("config: %v", err)
		a.flash = err.Error()
	}
}

func (a *App) applyLocale() {
	i18n.SetLocale(i18n.Resolve(a.cfg.General.Language, i18n.SystemLocales()))
}

// SetLanguage changes the interface language.
func (a *App) SetLanguage(code string) {
	a.cfg.General.Language = code
	a.applyLocale()
	a.saveConfig()
	a.invalidate()
}

func (a *App) reloadStyles() {
	if a.d.Paths.ConfigDir == "" {
		a.styles = enhance.LoadStyles("")
	} else {
		a.styles = enhance.LoadStyles(a.d.Paths.StylesDir())
	}
	if a.snap.cfg != nil {
		a.publish()
	}
}

// ---- Navigation ----

// SetPage shows a page of the hub.
func (a *App) SetPage(p Page) {
	if p == PageSettings {
		a.OpenSettings(a.section)
		return
	}
	a.page, a.inSettings = p, false
	a.invalidate()
}

// OpenSettings shows Settings at a section.
func (a *App) OpenSettings(s Section) {
	if !a.inSettings {
		a.settingsFrom = a.page
	}
	a.inSettings, a.section, a.page = true, s, PageSettings
	if s == SecProviders {
		a.refreshSecrets()
	}
	a.invalidate()
}

// CloseSettings goes back to the page before Settings.
func (a *App) CloseSettings() {
	a.inSettings = false
	a.page = a.settingsFrom
	if a.page == PageSettings {
		a.page = PageHome
	}
	a.invalidate()
}

// SetThemeMode sets Auto, Light or Dark.
func (a *App) SetThemeMode(m config.ThemeMode) {
	a.cfg.Appearance.Theme = m
	a.saveConfig()
	if a.d.Shell != nil {
		a.d.Shell.SetThemeMode(m)
	}
	a.invalidate()
}

// SetInk sets a named ink.
func (a *App) SetInk(n config.NamedInk) {
	a.cfg.Appearance.Ink = config.Ink{Named: n}
	a.saveConfig()
	a.invalidate()
}

// SetCustomInk sets a custom ink from "#RRGGBB"; false when it is no color.
func (a *App) SetCustomInk(hex string) bool {
	ink, err := config.ParseInk(strings.TrimSpace(hex))
	if err != nil {
		return false
	}
	a.cfg.Appearance.Ink = ink
	a.saveConfig()
	a.invalidate()
	return true
}

// ToggleSidebar collapses or expands the hub sidebar.
func (a *App) ToggleSidebar() {
	a.cfg.Appearance.SidebarCollapsed = !a.cfg.Appearance.SidebarCollapsed
	a.saveConfig()
	a.invalidate()
}

// ---- Dictation ----

// Toggle starts or stops a dictation.
func (a *App) Toggle() {
	if a.d.Dictation != nil {
		a.d.Dictation.Toggle()
	}
}

// CancelDictation cancels the dictation in progress.
func (a *App) CancelDictation() {
	if a.d.Dictation != nil {
		a.d.Dictation.Cancel()
	}
}

// Recording is true while the microphone records.
func (a *App) Recording() bool { return a.state.Kind == dictation.StateRecording }

// Busy is true while recording or processing.
func (a *App) Busy() bool {
	return a.state.Kind == dictation.StateRecording || a.state.Kind == dictation.StateProcessing
}

// Incognito is true while dictations are not saved.
func (a *App) Incognito() bool { return a.incognito }

// SetIncognito turns incognito on or off.
func (a *App) SetIncognito(on bool) {
	a.incognito = on
	a.publish()
	if on {
		a.showNotice(i18n.T("overlay.notice.incognito_on"))
	} else {
		a.showNotice(i18n.T("overlay.notice.incognito_off"))
	}
	a.invalidate()
}

// ActiveStyle is the id of the default style.
func (a *App) ActiveStyle() string {
	if a.cfg.AI.ActiveStyle != "" {
		if _, ok := a.styles.Get(a.cfg.AI.ActiveStyle); ok {
			return a.cfg.AI.ActiveStyle
		}
	}
	return "raw"
}

// SetActiveStyle makes a style the default.
func (a *App) SetActiveStyle(id string) {
	a.cfg.AI.ActiveStyle = id
	a.saveConfig()
	a.invalidate()
}

// CycleStyle makes the next style the default.
func (a *App) CycleStyle() {
	all := a.styles.Styles()
	if len(all) == 0 {
		return
	}
	cur := a.ActiveStyle()
	next := all[0].ID
	for i, s := range all {
		if s.ID == cur {
			next = all[(i+1)%len(all)].ID
		}
	}
	a.SetActiveStyle(next)
	if s, ok := a.styles.Get(next); ok {
		a.showNotice(i18n.T("overlay.notice.style", "name", styleName(s)))
	}
}

// PasteLast inserts the last dictation again.
func (a *App) PasteLast() {
	if a.d.Dictation == nil || a.d.Platform == nil || a.d.Platform.Inserter == nil {
		return
	}
	text := a.d.Dictation.LastText()
	if text == "" {
		return
	}
	ins := a.d.Platform.Inserter
	go ins.Insert(text, platform.InsertOptions{RestoreClipboard: a.cfg.Insertion.RestoreClipboard, FallbackToType: true})
}

// StateChanged implements dictation.Observer.
func (a *App) StateChanged(s dictation.State) {
	a.post(func() {
		if s.Kind == dictation.StateRecording && a.state.Kind != dictation.StateRecording {
			a.wave.reset()
			a.committed, a.tentative = "", ""
		}
		if a.state.Kind == dictation.StateProcessing && s.Kind != dictation.StateProcessing {
			a.dataChanged() // a new history entry
		}
		// The controller tells the hotkeys about the recording, off the
		// main thread.
		a.state, a.stateAt = s, a.now()
		a.syncOverlay()
		a.invalidate()
	})
}

// Notice implements dictation.Observer.
func (a *App) Notice(m dictation.Message) {
	a.post(func() {
		a.showNotice(i18n.TMap(m.Key, m.Args))
		a.invalidate()
	})
}

// Level implements dictation.Observer. It redraws nothing: only the
// overlay shows levels, and it draws a frame each refresh while recording.
// (A redraw of every window for each 20 ms level kept the main thread busy
// building the hub.)
func (a *App) Level(rms float32) {
	a.post(func() { a.wave.push(rms) })
}

// Preview implements dictation.Observer. Only the overlay shows it.
func (a *App) Preview(committed, tentative string) {
	a.post(func() {
		a.committed, a.tentative = committed, tentative
		a.invalidateOverlay()
	})
}

// noticeFor is how long a notice shows on the pill.
const noticeFor = 4 * time.Second

// showNotice shows a line on the overlay for a few seconds.
func (a *App) showNotice(text string) {
	a.lastNotice, a.noticeAt = text, a.now()
	a.syncOverlay()
	if a.d.Post == nil {
		return // tests clear it by hand
	}
	at := a.noticeAt
	time.AfterFunc(noticeFor, func() {
		a.post(func() {
			if a.noticeAt.Equal(at) {
				a.lastNotice = ""
				a.syncOverlay()
				a.invalidate()
			}
		})
	})
}

// noticeShowing is true while a notice from showNotice shows.
func (a *App) noticeShowing() bool {
	return a.lastNotice != "" && a.now().Sub(a.noticeAt) < noticeFor
}

// ---- Models ----

// FinalModel is the model of the final text.
func (a *App) FinalModel() models.ModelID {
	if a.cfg.Dictation.Model != "" {
		return a.cfg.Dictation.Model
	}
	return models.DefaultModel()
}

func (a *App) modelStatus(id models.ModelID) engine.ModelStatus {
	if a.d.Engine == nil {
		return engine.ModelStatus{}
	}
	return a.d.Engine.Status(string(id))
}

func (a *App) platformPermissions() platform.Permissions {
	if a.d.Platform == nil {
		return nil
	}
	return a.d.Platform.Permissions
}

// applyHotkeys gives the hotkey manager the hotkeys of the config.
func (a *App) applyHotkeys() {
	if a.d.Platform == nil || a.d.Platform.Hotkeys == nil {
		return
	}
	if err := a.d.Platform.Hotkeys.Configure(HotkeyConfig(&a.cfg)); err != nil {
		log.Printf("hotkeys: %v", err)
		a.flash = err.Error()
	}
}

// HotkeyConfig is the platform hotkey setup of a config.
func HotkeyConfig(cfg *config.Config) platform.HotkeyConfig {
	str := func(h *config.Hotkey) string {
		if h == nil {
			return ""
		}
		return h.String()
	}
	h := cfg.Hotkeys
	hc := platform.HotkeyConfig{
		Toggle: str(h.Toggle), PushToTalk: str(h.PushToTalk), PasteLast: str(h.PasteLast),
		CycleStyle: str(h.CycleStyle), Incognito: str(h.Incognito), CancelMode: string(h.Cancel),
		DoubleEscapeMs: int(h.DoubleEscapeMs), EnterSends: h.EnterSendsUsable(), TakeEscape: h.TakeEscapeUsable(),
		StyleHotkeys: map[string]string{},
	}
	for _, sh := range cfg.StyleHotkeys.All() {
		hc.StyleHotkeys[sh.Style] = sh.Hotkey.String()
	}
	return hc
}

func (a *App) engineRunning() bool { return a.d.Engine != nil && a.d.Engine.Running() }

// ---- helpers ----

func derefOr[T any](p *T, def T) T {
	if p == nil {
		return def
	}
	return *p
}

func ptr[T any](v T) *T { return &v }

// tildePath shows a path under the home folder as ~/….
func tildePath(path string) string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" || !strings.HasPrefix(path, home+string(os.PathSeparator)) {
		return path
	}
	return "~" + path[len(home):]
}

// computer is the word for this machine in texts ("Mac", "PC", "computer").
func computer() string { return i18n.TOS("os.computer", nil) }

func styleName(s *enhance.Style) string {
	if t, ok := i18n.Translate("styles.builtin." + s.ID + ".name"); ok && isBuiltin(s.ID) {
		return t
	}
	return s.Name
}

func styleDescription(s *enhance.Style) string {
	if t, ok := i18n.Translate("styles.builtin." + s.ID + ".description"); ok && isBuiltin(s.ID) {
		return t
	}
	return s.Description
}

func isBuiltin(id string) bool {
	for _, s := range enhance.BuiltinStyles() {
		if s.ID == id {
			return true
		}
	}
	return false
}
