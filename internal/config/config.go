// Package config reads and writes config.toml, the single source of truth
// for settings (crates/sayso-core/src/config.rs). Every field has a default,
// so a missing or partial file works. An invalid value gives an Issue that
// Settings shows; Sayso never falls back silently.
package config

import (
	"errors"
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/models/languages"
)

// DefaultStyleID is the style of a new config.
const DefaultStyleID = "clean"

// Config is the whole file.
type Config struct {
	General      General
	Hotkeys      Hotkeys
	StyleHotkeys StyleHotkeys
	Dictation    Dictation
	Insertion    Insertion
	Overlay      Overlay
	Audio        Audio
	Sounds       Sounds
	AI           AI
	Speech       Speech
	Models       ModelsSection
	History      History
	Appearance   Appearance
	Updates      Updates
	Advanced     Advanced
	Onboarding   Onboarding
}

// Default is the config of a missing file.
func Default() Config {
	return Config{
		General:      General{DockIconWithHub: true, Language: SystemLanguage},
		Hotkeys:      defaultHotkeys(),
		StyleHotkeys: StyleHotkeys{},
		Dictation: Dictation{
			Language: "en", Model: models.DefaultModel(), LivePreview: true,
			MinDurationMs: 300, MaxDurationS: 600, PreloadModel: true,
		},
		Insertion:  Insertion{RestoreClipboard: true, RestoreDelayMs: 450, TypeInApps: []string{}, TrailingSpace: true},
		Overlay:    Overlay{Style: OverlayPill, NotchOnOtherDisplays: true, IdlePill: true, HideInFullscreen: true, FollowDisplay: true, ShowPreview: true, Size: OverlayLarge},
		Sounds:     Sounds{Enabled: true, Volume: 0.35, Start: true, Stop: true, Cancel: true},
		AI:         defaultAI(),
		Speech:     defaultSpeech(),
		History:    History{Enabled: true, SaveAudio: true, KeepAudioDays: u32p(30), SiteIcons: true},
		Appearance: Appearance{Theme: ThemeSystem, Ink: Ink{Named: InkSumi}, PaperTexture: true, SystemTitleBar: true},
		Updates:    Updates{Check: true},
	}
}

func u32p(v uint32) *uint32 { return &v }

// SystemLanguage is the general.language value that follows the system.
const SystemLanguage = "system"

// InterfaceLocales are the locale codes of the interface (crates/sayso-i18n).
var InterfaceLocales = []string{"en", "es", "pt-BR", "zh-CN", "ja", "de", "fr", "it", "ko", "ru"}

// IsValidInterfaceLanguage is true for "system" or a locale (letter case ignored).
func IsValidInterfaceLanguage(v string) bool {
	if v == SystemLanguage {
		return true
	}
	for _, l := range InterfaceLocales {
		if strings.EqualFold(l, v) {
			return true
		}
	}
	return false
}

// General is [general].
type General struct {
	LaunchAtLogin   bool
	DockIconWithHub bool
	// Name in the Home greeting; nil uses the account's first name.
	Name *string
	// Language: "system" or a locale code (docs/i18n.md).
	Language string
}

// DisplayName is the trimmed name, or "".
func (g General) DisplayName() string {
	if g.Name == nil {
		return ""
	}
	return strings.TrimSpace(*g.Name)
}

// CancelMode is how Esc cancels a recording.
type CancelMode string

const (
	CancelDoubleEscape CancelMode = "double_escape"
	CancelSingleEscape CancelMode = "single_escape"
)

// Hotkeys is [hotkeys]. A nil hotkey is cleared; the file writes it as "".
type Hotkeys struct {
	Toggle         *Hotkey
	PushToTalk     *Hotkey
	Cancel         CancelMode
	DoubleEscapeMs uint64
	PasteLast      *Hotkey
	CycleStyle     *Hotkey
	Incognito      *Hotkey
	// EnterSends: Enter while recording stops, and Sayso presses Enter after the insert.
	EnterSends bool
	// TakeEscape: Sayso takes every plain Esc from the app while recording.
	TakeEscape bool
}

func hk(h Hotkey) *Hotkey { return &h }

func defaultHotkeys() Hotkeys {
	return Hotkeys{
		Toggle: hk(ToggleDefault()), Cancel: CancelDoubleEscape, DoubleEscapeMs: 400,
		PasteLast: hk(PasteLastDefault()), CycleStyle: hk(MustHotkey("ctrl+opt+s")), TakeEscape: true,
	}
}

// NeedsInputMonitoring: push-to-talk and modifier-only toggles need global keyboard access.
func (h Hotkeys) NeedsInputMonitoring() bool {
	if h.PushToTalk != nil {
		return true
	}
	if h.Toggle != nil {
		_, ok := h.Toggle.ModifierKeys()
		return ok
	}
	return false
}

// EnterSendsUsable: on, and toggle and push-to-talk do not use Return.
func (h Hotkeys) EnterSendsUsable() bool { return h.EnterSends && !h.usesKey(KeyReturn) }

// TakeEscapeUsable: on, and toggle and push-to-talk do not use Esc.
func (h Hotkeys) TakeEscapeUsable() bool { return h.TakeEscape && !h.usesKey(KeyEscape) }

func (h Hotkeys) usesKey(k KeyKind) bool {
	uses := func(x *Hotkey) bool { return x != nil && x.Kind == HotkeyChord && x.Key.Kind == k }
	return uses(h.Toggle) || uses(h.PushToTalk)
}

// StyleHotkeys is [style_hotkeys]: a hotkey per style id. An entry that does
// not parse stays in the file as it is, and Validate reports it.
type StyleHotkeys struct {
	keys    map[string]Hotkey
	invalid map[string]any
}

func (s StyleHotkeys) Get(style string) (Hotkey, bool) { h, ok := s.keys[style]; return h, ok }

// Set sets or clears the hotkey of a style; a bad entry of that style goes away.
func (s *StyleHotkeys) Set(style string, h *Hotkey) {
	delete(s.invalid, style)
	if h == nil {
		delete(s.keys, style)
		return
	}
	if s.keys == nil {
		s.keys = map[string]Hotkey{}
	}
	s.keys[style] = *h
}

// StyleHotkey is a style id and its hotkey.
type StyleHotkey struct {
	Style  string
	Hotkey Hotkey
}

// All lists the hotkeys in id order.
func (s StyleHotkeys) All() []StyleHotkey {
	out := make([]StyleHotkey, 0, len(s.keys))
	for _, id := range sortedKeys(s.keys) {
		out = append(out, StyleHotkey{id, s.keys[id]})
	}
	return out
}

func (s StyleHotkeys) IsEmpty() bool { return len(s.keys) == 0 && len(s.invalid) == 0 }

func readStyleHotkeys(v any) (StyleHotkeys, error) {
	m, ok := v.(map[string]any)
	if !ok {
		return StyleHotkeys{}, fmt.Errorf("invalid type: %s, expected a map", describe(v))
	}
	out := StyleHotkeys{keys: map[string]Hotkey{}, invalid: map[string]any{}}
	for id, value := range m {
		text, isStr := value.(string)
		if !isStr {
			out.invalid[id] = value
			continue
		}
		text = strings.TrimSpace(text)
		if text == "" {
			continue // An empty value is no key, as in [hotkeys].
		}
		h, err := ParseHotkey(text)
		if err != nil {
			out.invalid[id] = value
			continue
		}
		out.keys[id] = h
	}
	return out, nil
}

// PreviewChoice is "auto" or a streaming model.
type PreviewChoice = models.RoleChoice

// Dictation is [dictation].
type Dictation struct {
	// Language: a code such as "en", or "auto".
	Language string
	// Model: a local model id, or "<provider id>:<model>".
	Model             models.ModelID
	PreviewModel      PreviewChoice
	LivePreview       bool
	SpokenPunctuation bool
	MinDurationMs     uint64
	MaxDurationS      uint64
	// KeepModelMinutes: nil keeps the model in memory always.
	KeepModelMinutes *uint32
	PreloadModel     bool
}

// ModelsSection is [models]: the VAD and diarization models of jobs and live sessions.
type ModelsSection struct {
	Vad         models.RoleChoice
	Diarization models.RoleChoice
}

// Insertion is [insertion].
type Insertion struct {
	RestoreClipboard bool
	RestoreDelayMs   uint64
	TypeInApps       []string
	SmartSpacing     bool
	TrailingSpace    bool
}

// TextToInsert adds the trailing space when the text ends in a non-space.
func (i Insertion) TextToInsert(text string) string {
	r := []rune(text)
	if i.TrailingSpace && len(r) > 0 && !unicode.IsSpace(r[len(r)-1]) {
		return text + " "
	}
	return text
}

// OverlayStyle is the look of the overlay.
type OverlayStyle string

const (
	OverlayPill  OverlayStyle = "pill"
	OverlayNotch OverlayStyle = "notch"
)

// OverlaySize: large is the size of the design.
type OverlaySize string

const (
	OverlaySmall  OverlaySize = "small"
	OverlayMedium OverlaySize = "medium"
	OverlayLarge  OverlaySize = "large"
)

// Scale is the factor for shapes, gaps, and icons.
func (s OverlaySize) Scale() float32 {
	switch s {
	case OverlaySmall:
		return 0.75
	case OverlayMedium:
		return 0.87
	}
	return 1.0
}

// TextScale shrinks half as much as shapes.
func (s OverlaySize) TextScale() float32 { return 0.5 + 0.5*s.Scale() }

// Overlay is [overlay].
type Overlay struct {
	Style                OverlayStyle
	NotchOnOtherDisplays bool
	IdlePill             bool
	HideInFullscreen     bool
	FollowDisplay        bool
	ShowPreview          bool
	Size                 OverlaySize
}

// Audio is [audio].
type Audio struct {
	InputDevice *string
	PauseMedia  bool
}

// Sounds is [sounds].
type Sounds struct {
	Enabled                     bool
	Volume                      float32
	Start, Stop, Cancel, Insert bool
}

// History is [history].
type History struct {
	Enabled       bool
	KeepTextDays  *uint32
	SaveAudio     bool
	KeepAudioDays *uint32
	SiteIcons     bool
}

// ThemeMode is the appearance theme.
type ThemeMode string

const (
	ThemeSystem ThemeMode = "system"
	ThemeLight  ThemeMode = "light"
	ThemeDark   ThemeMode = "dark"
)

// NamedInk is one of the inks of sayso-ink.
type NamedInk string

const (
	InkSumi      NamedInk = "sumi"
	InkIndigo    NamedInk = "indigo"
	InkIronGall  NamedInk = "iron_gall"
	InkSepia     NamedInk = "sepia"
	InkVerdigris NamedInk = "verdigris"
	InkOxblood   NamedInk = "oxblood"
)

var namedInks = []string{"sumi", "indigo", "iron_gall", "sepia", "verdigris", "oxblood"}

// Ink is a named ink, or a custom #RRGGBB color.
type Ink struct {
	Named  NamedInk
	Custom uint32
	// IsCustom selects Custom.
	IsCustom bool
}

func (i Ink) String() string {
	if i.IsCustom {
		return fmt.Sprintf("#%06X", i.Custom)
	}
	return string(i.Named)
}

// ParseInk reads a named ink or #RRGGBB.
func ParseInk(s string) (Ink, error) {
	for _, n := range namedInks {
		if s == n {
			return Ink{Named: NamedInk(n)}, nil
		}
	}
	hex := strings.TrimPrefix(strings.TrimSpace(s), "#")
	var v uint32
	if len(hex) == 6 {
		if _, err := fmt.Sscanf(hex, "%06x", &v); err == nil {
			return Ink{Custom: v, IsCustom: true}, nil
		}
	}
	return Ink{}, errors.New("data did not match any variant of untagged enum Ink")
}

// Appearance is [appearance].
type Appearance struct {
	Theme        ThemeMode
	Ink          Ink
	PaperTexture bool
	// ReduceMotion: nil follows the system.
	ReduceMotion     *bool
	SystemTitleBar   bool
	SidebarCollapsed bool
}

// Updates is [updates].
type Updates struct{ Check, Automatic bool }

// Advanced is [advanced].
type Advanced struct {
	EnginePath *string
	LogLevel   *string
}

// Onboarding is [onboarding].
type Onboarding struct {
	Completed bool
	Step      uint8
}

const keysStep = 4

// ReachedKeys is true from the step that sets the shortcuts on.
func (o Onboarding) ReachedKeys() bool { return o.Completed || o.Step >= keysStep }

// Problem is what is wrong with a value. Error() gives the English text for
// the log; the app shows the interface language.
type Problem struct {
	Kind ProblemKind
	// Text: the path and error of Unreadable, the parser message of Syntax
	// and BadSection, the id/model/language/value of the others.
	Text   string
	Path   string
	Hotkey Hotkey
}

// ProblemKind enumerates problems.
type ProblemKind int

const (
	ProblemUnreadable ProblemKind = iota
	ProblemSyntax
	ProblemBadSection
	ProblemVolumeRange
	ProblemAITimeoutTooShort
	ProblemPushToTalkIsToggle
	ProblemHotkeyConflictsWithToggle
	ProblemUnknownProvider
	ProblemUnknownModel
	ProblemNotAStreamingModel
	ProblemNotAVadModel
	ProblemNotADiarizationModel
	ProblemUnknownInterfaceLanguage
	ProblemUnknownDictationLanguage
	ProblemSpeechTimeoutTooShort
	ProblemBadProviderID
	ProblemDuplicateProviderID
	ProblemNeedsBaseURL
	ProblemBadHotkey
	ProblemStyleHotkeyNeedsKey
	ProblemHotkeyInUse
)

func (p Problem) Error() string {
	switch p.Kind {
	case ProblemUnreadable:
		return fmt.Sprintf("could not read %s: %s", p.Path, p.Text)
	case ProblemSyntax:
		return p.Text
	case ProblemBadSection:
		return p.Text + " (using defaults for this section)"
	case ProblemVolumeRange:
		return "must be between 0.0 and 1.0"
	case ProblemAITimeoutTooShort:
		return "timeouts must be at least 500 ms"
	case ProblemPushToTalkIsToggle:
		return "is the same key as hotkeys.toggle"
	case ProblemHotkeyConflictsWithToggle:
		return p.Hotkey.String() + " conflicts with hotkeys.toggle"
	case ProblemUnknownProvider:
		return fmt.Sprintf("no provider has the id \"%s\"", p.Text)
	case ProblemUnknownModel:
		return fmt.Sprintf("\"%s\" is not a known model", p.Text)
	case ProblemNotAStreamingModel:
		return fmt.Sprintf("\"%s\" cannot show a live preview. Use \"auto\" or a streaming model", p.Text)
	case ProblemNotAVadModel:
		return fmt.Sprintf("\"%s\" is not a VAD model. Sayso uses \"auto\"", p.Text)
	case ProblemNotADiarizationModel:
		return fmt.Sprintf("\"%s\" is not a diarization model. Sayso uses \"auto\"", p.Text)
	case ProblemUnknownInterfaceLanguage:
		return fmt.Sprintf("\"%s\" is not an interface language Sayso has. Sayso uses the language of the system", p.Text)
	case ProblemUnknownDictationLanguage:
		return fmt.Sprintf("\"%s\" is not a language code Sayso knows. Use \"auto\" or a code such as \"en\"", p.Text)
	case ProblemSpeechTimeoutTooShort:
		return "must be at least 1000 ms"
	case ProblemBadProviderID:
		return fmt.Sprintf("the provider id \"%s\" must not be empty or contain a colon", p.Text)
	case ProblemDuplicateProviderID:
		return fmt.Sprintf("two providers have the id \"%s\"", p.Text)
	case ProblemNeedsBaseURL:
		return fmt.Sprintf("the provider \"%s\" needs a base_url", p.Text)
	case ProblemBadHotkey:
		return fmt.Sprintf("\"%s\" is not a hotkey", p.Text)
	case ProblemStyleHotkeyNeedsKey:
		return p.Hotkey.String() + " has no regular key"
	case ProblemHotkeyInUse:
		return p.Hotkey.String() + " is already used by another Sayso hotkey"
	}
	return "unknown problem"
}

// Issue is a problem in config.toml. The app keeps running with the default for that value.
type Issue struct {
	Field   string
	Problem Problem
}

// Loaded is the result of Load.
type Loaded struct {
	Config Config
	Issues []Issue
	// Created: the file did not exist; Sayso writes the defaults on first save.
	Created bool
}

// Unreadable gives the defaults for a file that exists but cannot be read.
// Not Created, so the app does not write the defaults over the file.
func Unreadable(path string, err error) Loaded {
	return Loaded{Config: Default(), Issues: []Issue{{"config.toml", Problem{Kind: ProblemUnreadable, Path: path, Text: err.Error()}}}}
}

// Load reads config.toml. A missing file gives the defaults. A file that is
// not valid UTF-8 or cannot be read is an error (see Unreadable).
func Load(path string) (Loaded, error) {
	b, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return Loaded{Config: Default(), Created: true}, nil
	}
	if err != nil {
		return Loaded{}, &ReadError{path, err}
	}
	if !utf8Valid(b) {
		return Loaded{}, &ReadError{path, errors.New("stream did not contain valid UTF-8")}
	}
	return Parse(string(b)), nil
}

// ReadError is a config file that is there but cannot be read.
type ReadError struct {
	Path string
	Err  error
}

func (e *ReadError) Error() string { return fmt.Sprintf("could not read %s: %v", e.Path, e.Err) }
func (e *ReadError) Unwrap() error { return e.Err }

// Parse reads the text. A syntax error keeps all defaults and reports it; a
// bad value in one section keeps the rest of the file.
func Parse(text string) Loaded {
	doc, err := parseDocument(text)
	if err != nil {
		cfg := Default()
		issues := []Issue{{"config.toml", Problem{Kind: ProblemSyntax, Text: err.Error()}}}
		return Loaded{Config: cfg, Issues: append(issues, cfg.Validate()...)}
	}
	var issues []Issue
	cfg := fromSections(doc, &issues)
	return Loaded{Config: cfg, Issues: append(issues, cfg.Validate()...)}
}

func fromSections(doc map[string]any, issues *[]Issue) Config {
	cfg := Default()
	section := func(name string, read func(v any) error) {
		v, ok := doc[name]
		if !ok {
			return
		}
		if err := read(v); err != nil {
			*issues = append(*issues, Issue{name, Problem{Kind: ProblemBadSection, Text: err.Error()}})
		}
	}
	section("general", func(v any) error { return readInto(&cfg.General, readGeneral, v) })
	section("hotkeys", func(v any) error { return readInto(&cfg.Hotkeys, readHotkeys, v) })
	section("style_hotkeys", func(v any) error { return readInto(&cfg.StyleHotkeys, readStyleHotkeys, v) })
	section("dictation", func(v any) error { return readInto(&cfg.Dictation, readDictation, v) })
	section("insertion", func(v any) error { return readInto(&cfg.Insertion, readInsertion, v) })
	section("overlay", func(v any) error { return readInto(&cfg.Overlay, readOverlay, v) })
	section("audio", func(v any) error { return readInto(&cfg.Audio, readAudio, v) })
	section("sounds", func(v any) error { return readInto(&cfg.Sounds, readSounds, v) })
	section("ai", func(v any) error { return readInto(&cfg.AI, readAI, v) })
	section("speech", func(v any) error { return readInto(&cfg.Speech, readSpeech, v) })
	section("models", func(v any) error { return readInto(&cfg.Models, readModels, v) })
	section("history", func(v any) error { return readInto(&cfg.History, readHistory, v) })
	section("appearance", func(v any) error { return readInto(&cfg.Appearance, readAppearance, v) })
	section("updates", func(v any) error { return readInto(&cfg.Updates, readUpdates, v) })
	section("advanced", func(v any) error { return readInto(&cfg.Advanced, readAdvanced, v) })
	section("onboarding", func(v any) error { return readInto(&cfg.Onboarding, readOnboarding, v) })
	return cfg
}

// readInto keeps the default of a section whose read failed.
func readInto[T any](dst *T, read func(any) (T, error), v any) error {
	got, err := read(v)
	if err != nil {
		return err
	}
	*dst = got
	return nil
}

func readGeneral(v any) (General, error) {
	g := Default().General
	r := newReader(v)
	r.boolean("launch_at_login", &g.LaunchAtLogin)
	r.boolean("dock_icon_with_hub", &g.DockIconWithHub)
	r.optStr("name", &g.Name)
	r.str("language", &g.Language)
	return g, r.err
}

func (r *reader) optHotkey(key string, dst **Hotkey) {
	v, ok := r.get(key)
	if !ok {
		return
	}
	s, isStr := v.(string)
	if !isStr {
		r.fail(key, fmt.Errorf("invalid type: %s, expected a string", describe(v)))
		return
	}
	if strings.TrimSpace(s) == "" {
		*dst = nil
		return
	}
	h, err := ParseHotkey(s)
	if err != nil {
		r.fail(key, err)
		return
	}
	*dst = &h
}

func readHotkeys(v any) (Hotkeys, error) {
	h := defaultHotkeys()
	r := newReader(v)
	r.optHotkey("toggle", &h.Toggle)
	r.optHotkey("push_to_talk", &h.PushToTalk)
	r.enum("cancel", []string{"double_escape", "single_escape"}, func(i int) {
		h.Cancel = []CancelMode{CancelDoubleEscape, CancelSingleEscape}[i]
	})
	r.u64("double_escape_ms", &h.DoubleEscapeMs)
	r.optHotkey("paste_last", &h.PasteLast)
	r.optHotkey("cycle_style", &h.CycleStyle)
	r.optHotkey("incognito", &h.Incognito)
	r.boolean("enter_sends", &h.EnterSends)
	r.boolean("take_escape", &h.TakeEscape)
	return h, r.err
}

func readDictation(v any) (Dictation, error) {
	d := Default().Dictation
	r := newReader(v)
	r.str("language", &d.Language)
	model := string(d.Model)
	r.str("model", &model)
	d.Model = models.ModelID(model)
	preview := d.PreviewModel.String()
	r.str("preview_model", &preview)
	d.PreviewModel = models.ParseRoleChoice(preview)
	r.boolean("live_preview", &d.LivePreview)
	r.boolean("spoken_punctuation", &d.SpokenPunctuation)
	r.u64("min_duration_ms", &d.MinDurationMs)
	r.u64("max_duration_s", &d.MaxDurationS)
	r.optU32("keep_model_minutes", &d.KeepModelMinutes)
	r.boolean("preload_model", &d.PreloadModel)
	return d, r.err
}

func readModels(v any) (ModelsSection, error) {
	var m ModelsSection
	r := newReader(v)
	vad, dia := "auto", "auto"
	r.str("vad", &vad)
	r.str("diarization", &dia)
	m.Vad, m.Diarization = models.ParseRoleChoice(vad), models.ParseRoleChoice(dia)
	return m, r.err
}

func readInsertion(v any) (Insertion, error) {
	i := Default().Insertion
	r := newReader(v)
	r.boolean("restore_clipboard", &i.RestoreClipboard)
	r.u64("restore_delay_ms", &i.RestoreDelayMs)
	r.strings("type_in_apps", &i.TypeInApps)
	r.boolean("smart_spacing", &i.SmartSpacing)
	r.boolean("trailing_space", &i.TrailingSpace)
	return i, r.err
}

func readOverlay(v any) (Overlay, error) {
	o := Default().Overlay
	r := newReader(v)
	r.enum("style", []string{"pill", "notch"}, func(i int) { o.Style = []OverlayStyle{OverlayPill, OverlayNotch}[i] })
	r.boolean("notch_on_other_displays", &o.NotchOnOtherDisplays)
	r.boolean("idle_pill", &o.IdlePill)
	r.boolean("hide_in_fullscreen", &o.HideInFullscreen)
	r.boolean("follow_display", &o.FollowDisplay)
	r.boolean("show_preview", &o.ShowPreview)
	r.enum("size", []string{"small", "medium", "large"}, func(i int) {
		o.Size = []OverlaySize{OverlaySmall, OverlayMedium, OverlayLarge}[i]
	})
	return o, r.err
}

func readAudio(v any) (Audio, error) {
	var a Audio
	r := newReader(v)
	r.optStr("input_device", &a.InputDevice)
	r.boolean("pause_media", &a.PauseMedia)
	return a, r.err
}

func readSounds(v any) (Sounds, error) {
	s := Default().Sounds
	r := newReader(v)
	r.boolean("enabled", &s.Enabled)
	r.f32("volume", &s.Volume)
	r.boolean("start", &s.Start)
	r.boolean("stop", &s.Stop)
	r.boolean("cancel", &s.Cancel)
	r.boolean("insert", &s.Insert)
	return s, r.err
}

func readHistory(v any) (History, error) {
	h := Default().History
	r := newReader(v)
	r.boolean("enabled", &h.Enabled)
	r.optU32("keep_text_days", &h.KeepTextDays)
	r.boolean("save_audio", &h.SaveAudio)
	r.optU32("keep_audio_days", &h.KeepAudioDays)
	r.boolean("site_icons", &h.SiteIcons)
	return h, r.err
}

func readAppearance(v any) (Appearance, error) {
	a := Default().Appearance
	r := newReader(v)
	r.enum("theme", []string{"system", "light", "dark"}, func(i int) {
		a.Theme = []ThemeMode{ThemeSystem, ThemeLight, ThemeDark}[i]
	})
	if iv, ok := r.get("ink"); ok {
		s, isStr := iv.(string)
		ink, err := ParseInk(s)
		if !isStr || err != nil {
			r.fail("ink", errors.New("data did not match any variant of untagged enum Ink"))
		} else {
			a.Ink = ink
		}
	}
	r.boolean("paper_texture", &a.PaperTexture)
	r.optBool("reduce_motion", &a.ReduceMotion)
	r.boolean("system_title_bar", &a.SystemTitleBar)
	r.boolean("sidebar_collapsed", &a.SidebarCollapsed)
	return a, r.err
}

func readUpdates(v any) (Updates, error) {
	u := Default().Updates
	r := newReader(v)
	r.boolean("check", &u.Check)
	r.boolean("automatic", &u.Automatic)
	return u, r.err
}

func readAdvanced(v any) (Advanced, error) {
	var a Advanced
	r := newReader(v)
	r.optStr("engine_path", &a.EnginePath)
	r.optStr("log_level", &a.LogLevel)
	return a, r.err
}

func readOnboarding(v any) (Onboarding, error) {
	var o Onboarding
	r := newReader(v)
	r.boolean("completed", &o.Completed)
	r.u8("step", &o.Step)
	return o, r.err
}

// Validate checks the values that parse but are wrong.
func (c Config) Validate() []Issue {
	var issues []Issue
	issue := func(field string, p Problem) { issues = append(issues, Issue{field, p}) }
	if !(c.Sounds.Volume >= 0 && c.Sounds.Volume <= 1) {
		issue("sounds.volume", Problem{Kind: ProblemVolumeRange})
	}
	if c.AI.HTTPTimeoutMs < 500 || c.AI.CLITimeoutMs < 500 {
		issue("ai", Problem{Kind: ProblemAITimeoutTooShort})
	}
	if toggle := c.Hotkeys.Toggle; toggle != nil {
		for _, f := range []struct {
			field string
			h     *Hotkey
		}{
			{"hotkeys.push_to_talk", c.Hotkeys.PushToTalk},
			{"hotkeys.paste_last", c.Hotkeys.PasteLast},
			{"hotkeys.cycle_style", c.Hotkeys.CycleStyle},
			{"hotkeys.incognito", c.Hotkeys.Incognito},
		} {
			if f.h != nil && f.h.ConflictsWithToggle(*toggle) {
				if f.field == "hotkeys.push_to_talk" && *f.h == *toggle {
					issue(f.field, Problem{Kind: ProblemPushToTalkIsToggle})
				} else {
					issue(f.field, Problem{Kind: ProblemHotkeyConflictsWithToggle, Hotkey: *f.h})
				}
			}
		}
	}
	for _, id := range sortedKeys(c.StyleHotkeys.invalid) {
		v := c.StyleHotkeys.invalid[id]
		text, ok := v.(string)
		if !ok {
			text = formatValue(fromDecoded(v))
		}
		issue("style_hotkeys."+id, Problem{Kind: ProblemBadHotkey, Text: text})
	}
	_, refused := c.StyleHotkeyCheck()
	for _, r := range refused {
		issue("style_hotkeys."+r.Style, r.Problem)
	}
	if c.AI.DefaultProvider != nil {
		if _, ok := c.AI.Provider(*c.AI.DefaultProvider); !ok {
			issue("ai.default_provider", Problem{Kind: ProblemUnknownProvider, Text: *c.AI.DefaultProvider})
		}
	}
	if _, ok := models.FindWith(c.Speech.Catalog(), c.Dictation.Model); !ok {
		issue("dictation.model", Problem{Kind: ProblemUnknownModel, Text: string(c.Dictation.Model)})
	}
	if !c.Dictation.PreviewModel.IsAuto() {
		id := c.Dictation.PreviewModel.Model
		m, ok := models.FindLocal(id)
		switch {
		case !ok:
			issue("dictation.preview_model", Problem{Kind: ProblemUnknownModel, Text: string(id)})
		case !m.LivePreview:
			issue("dictation.preview_model", Problem{Kind: ProblemNotAStreamingModel, Text: string(id)})
		}
	}
	if !c.Models.Vad.IsAuto() && !models.IsVadModel(c.Models.Vad.Model) {
		issue("models.vad", Problem{Kind: ProblemNotAVadModel, Text: string(c.Models.Vad.Model)})
	}
	if !c.Models.Diarization.IsAuto() && !models.IsDiarizationModel(c.Models.Diarization.Model) {
		issue("models.diarization", Problem{Kind: ProblemNotADiarizationModel, Text: string(c.Models.Diarization.Model)})
	}
	if !IsValidInterfaceLanguage(c.General.Language) {
		issue("general.language", Problem{Kind: ProblemUnknownInterfaceLanguage, Text: c.General.Language})
	}
	if !languages.IsValid(c.Dictation.Language) {
		issue("dictation.language", Problem{Kind: ProblemUnknownDictationLanguage, Text: c.Dictation.Language})
	}
	if c.Speech.TimeoutMs < 1000 {
		issue("speech.timeout_ms", Problem{Kind: ProblemSpeechTimeoutTooShort})
	}
	for i, p := range c.Speech.Providers {
		if p.ID == "" || strings.Contains(p.ID, ":") {
			issue("speech.providers", Problem{Kind: ProblemBadProviderID, Text: p.ID})
		}
		for _, q := range c.Speech.Providers[:i] {
			if q.ID == p.ID {
				issue("speech.providers", Problem{Kind: ProblemDuplicateProviderID, Text: p.ID})
				break
			}
		}
		if p.Kind == SpeechOpenAICompatible && p.BaseURLResolved() == "" {
			issue("speech.providers", Problem{Kind: ProblemNeedsBaseURL, Text: p.ID})
		}
	}
	return issues
}

// RefusedStyleHotkey is a style hotkey that cannot work, and why.
type RefusedStyleHotkey struct {
	Style   string
	Problem Problem
}

// StyleHotkeyCheck gives the style hotkeys that can work, in id order, and the
// problem of each that cannot. A style hotkey needs a regular key and must
// not overlap a [hotkeys] hotkey or a style hotkey before it.
func (c Config) StyleHotkeyCheck() ([]StyleHotkey, []RefusedStyleHotkey) {
	var taken []Hotkey
	for _, h := range []*Hotkey{c.Hotkeys.Toggle, c.Hotkeys.PushToTalk, c.Hotkeys.PasteLast, c.Hotkeys.CycleStyle, c.Hotkeys.Incognito} {
		if h != nil {
			taken = append(taken, *h)
		}
	}
	var usable []StyleHotkey
	var refused []RefusedStyleHotkey
	for _, sh := range c.StyleHotkeys.All() {
		if _, modOnly := sh.Hotkey.ModifierKeys(); modOnly {
			refused = append(refused, RefusedStyleHotkey{sh.Style, Problem{Kind: ProblemStyleHotkeyNeedsKey, Hotkey: sh.Hotkey}})
			continue
		}
		inUse := false
		for _, t := range taken {
			if t.Overlaps(sh.Hotkey) {
				inUse = true
				break
			}
		}
		if inUse {
			refused = append(refused, RefusedStyleHotkey{sh.Style, Problem{Kind: ProblemHotkeyInUse, Hotkey: sh.Hotkey}})
			continue
		}
		taken = append(taken, sh.Hotkey)
		usable = append(usable, sh)
	}
	return usable, refused
}

func optHotkeyText(h *Hotkey) string {
	if h == nil {
		return ""
	}
	return h.String()
}

// ToTOML writes the config the way toml-rs to_string_pretty writes it.
func (c Config) ToTOML() string {
	root := newTable()
	g := newTable().Set("launch_at_login", c.General.LaunchAtLogin).Set("dock_icon_with_hub", c.General.DockIconWithHub)
	if c.General.Name != nil {
		g.Set("name", *c.General.Name)
	}
	g.Set("language", c.General.Language)
	root.Set("general", g)

	h := c.Hotkeys
	root.Set("hotkeys", newTable().
		Set("toggle", optHotkeyText(h.Toggle)).
		Set("push_to_talk", optHotkeyText(h.PushToTalk)).
		Set("cancel", string(h.Cancel)).
		Set("double_escape_ms", h.DoubleEscapeMs).
		Set("paste_last", optHotkeyText(h.PasteLast)).
		Set("cycle_style", optHotkeyText(h.CycleStyle)).
		Set("incognito", optHotkeyText(h.Incognito)).
		Set("enter_sends", h.EnterSends).
		Set("take_escape", h.TakeEscape))

	if !c.StyleHotkeys.IsEmpty() {
		sh := newTable()
		entries := map[string]any{}
		for id, v := range c.StyleHotkeys.invalid {
			entries[id] = fromDecoded(v)
		}
		for id, k := range c.StyleHotkeys.keys {
			entries[id] = k.String()
		}
		for _, id := range sortedKeys(entries) {
			sh.Set(id, entries[id])
		}
		root.Set("style_hotkeys", sh)
	}

	d := c.Dictation
	dt := newTable().Set("language", d.Language).Set("model", string(d.Model)).Set("preview_model", d.PreviewModel.String()).
		Set("live_preview", d.LivePreview).Set("spoken_punctuation", d.SpokenPunctuation).
		Set("min_duration_ms", d.MinDurationMs).Set("max_duration_s", d.MaxDurationS)
	if d.KeepModelMinutes != nil {
		dt.Set("keep_model_minutes", *d.KeepModelMinutes)
	}
	dt.Set("preload_model", d.PreloadModel)
	root.Set("dictation", dt)

	in := c.Insertion
	typeIn := in.TypeInApps
	if typeIn == nil {
		typeIn = []string{}
	}
	root.Set("insertion", newTable().Set("restore_clipboard", in.RestoreClipboard).Set("restore_delay_ms", in.RestoreDelayMs).
		Set("type_in_apps", stringsToAny(typeIn)).Set("smart_spacing", in.SmartSpacing).Set("trailing_space", in.TrailingSpace))

	o := c.Overlay
	root.Set("overlay", newTable().Set("style", string(o.Style)).Set("notch_on_other_displays", o.NotchOnOtherDisplays).
		Set("idle_pill", o.IdlePill).Set("hide_in_fullscreen", o.HideInFullscreen).Set("follow_display", o.FollowDisplay).
		Set("show_preview", o.ShowPreview).Set("size", string(o.Size)))

	au := newTable()
	if c.Audio.InputDevice != nil {
		au.Set("input_device", *c.Audio.InputDevice)
	}
	root.Set("audio", au.Set("pause_media", c.Audio.PauseMedia))

	s := c.Sounds
	root.Set("sounds", newTable().Set("enabled", s.Enabled).Set("volume", f32(s.Volume)).Set("start", s.Start).
		Set("stop", s.Stop).Set("cancel", s.Cancel).Set("insert", s.Insert))

	root.Set("ai", c.AI.toTable())
	root.Set("speech", c.Speech.toTable())
	root.Set("models", newTable().Set("vad", c.Models.Vad.String()).Set("diarization", c.Models.Diarization.String()))

	hi := c.History
	ht := newTable().Set("enabled", hi.Enabled)
	if hi.KeepTextDays != nil {
		ht.Set("keep_text_days", *hi.KeepTextDays)
	}
	ht.Set("save_audio", hi.SaveAudio)
	if hi.KeepAudioDays != nil {
		ht.Set("keep_audio_days", *hi.KeepAudioDays)
	}
	root.Set("history", ht.Set("site_icons", hi.SiteIcons))

	ap := c.Appearance
	at := newTable().Set("theme", string(ap.Theme)).Set("ink", ap.Ink.String()).Set("paper_texture", ap.PaperTexture)
	if ap.ReduceMotion != nil {
		at.Set("reduce_motion", *ap.ReduceMotion)
	}
	root.Set("appearance", at.Set("system_title_bar", ap.SystemTitleBar).Set("sidebar_collapsed", ap.SidebarCollapsed))

	root.Set("updates", newTable().Set("check", c.Updates.Check).Set("automatic", c.Updates.Automatic))
	adv := newTable()
	if c.Advanced.EnginePath != nil {
		adv.Set("engine_path", *c.Advanced.EnginePath)
	}
	if c.Advanced.LogLevel != nil {
		adv.Set("log_level", *c.Advanced.LogLevel)
	}
	root.Set("advanced", adv)
	root.Set("onboarding", newTable().Set("completed", c.Onboarding.Completed).Set("step", c.Onboarding.Step))
	return root.Encode()
}

// backupSuffix names the copy of the file before the first cleanup model.
const backupSuffix = ".before-cleanup-models"

// Save writes the file atomically (temp file and rename). The first save with
// cleanup models first copies the old file to config.toml.before-cleanup-models,
// because an older Sayso drops [[ai.cleanup_models]] when it saves.
func (c Config) Save(path string) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	text := c.ToTOML()
	if strings.Contains(text, "[[ai.cleanup_models]]") {
		backUpBeforeCleanupModels(path)
	}
	return WriteAtomic(path, text)
}

func backUpBeforeCleanupModels(path string) {
	old, err := os.ReadFile(path)
	if err != nil || strings.Contains(string(old), "cleanup_models") {
		return
	}
	backup := path + backupSuffix
	if _, err := os.Stat(backup); err == nil {
		return
	}
	if err := os.WriteFile(backup, old, 0o644); err != nil {
		slog.Warn("could not back up config.toml before cleanup models", "path", backup, "err", err)
	}
}

// WriteAtomic writes text to path through <path without extension>.tmp and a rename.
func WriteAtomic(path, text string) error {
	tmp := strings.TrimSuffix(path, filepath.Ext(path)) + ".tmp"
	if err := os.WriteFile(tmp, []byte(text), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func utf8Valid(b []byte) bool { return utf8.Valid(b) }
