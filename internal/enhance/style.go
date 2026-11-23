package enhance

import (
	"errors"
	"fmt"
	"io/fs"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/mobile/core/styles"
)

// Styles (sayso-core/src/style.rs). Built-in styles ship in the binary. A
// file <config>/styles/<id>.toml with the same id overrides one; "Reset"
// deletes that file.

const (
	RawStyleID     = styles.RawID
	DefaultStyleID = styles.DefaultID
	// BasePromptFile is the user's base prompt in the styles directory.
	BasePromptFile = "base.md"
	defaultInk     = "#1D1B18"
)

// CleanupFieldState is the state of a style file's cleanup key.
type CleanupFieldState int

const (
	CleanupAbsent      CleanupFieldState = iota // no key
	CleanupOff                                  // cleanup = false
	CleanupSettingsSet                          // a [cleanup] table (or true)
)

// CleanupField is the cleanup key of a style file.
type CleanupField struct {
	State    CleanupFieldState
	Settings CleanupSettings
}

// Style is an AI enhancement style.
type Style struct {
	ID          string
	Name        string
	Ink         string // seal color #RRGGBB
	Description string
	Prompt      string // empty means no AI ("Raw")
	Provider    *string
	Model       *string
	Temperature *float32
	TimeoutMs   *uint64
	// Standalone leaves the base prompt out (a translation, for example).
	Standalone bool
	Cleanup    CleanupField
	// InheritedCleanup is the built-in style's settings for an override with
	// no cleanup key. Not in the file.
	InheritedCleanup *CleanupSettings
}

// UsesAI is true for a style with a prompt.
func (s *Style) UsesAI() bool { return strings.TrimSpace(s.Prompt) != "" }

// CleanupSettingsOf is the settings a cleanup model uses, or nil.
func (s *Style) CleanupSettingsOf() *CleanupSettings {
	switch s.Cleanup.State {
	case CleanupSettingsSet:
		c := s.Cleanup.Settings
		return &c
	case CleanupOff:
		return nil
	}
	return s.InheritedCleanup
}

// IsRaw is true for a style with no prompt and no cleanup settings.
func (s *Style) IsRaw() bool { return !s.UsesAI() && s.CleanupSettingsOf() == nil }

// RunPlanKind is how a style runs on a provider.
type RunPlanKind int

const (
	PlanNothing     RunPlanKind = iota // no AI
	PlanPrompt                         // a provider that reads prompts runs the prompt
	PlanCleanup                        // a cleanup model runs with Settings
	PlanCleanupOnly                    // a cleanup model cleans up with the defaults
)

// RunPlan is a plan kind and, for PlanCleanup, its settings.
type RunPlan struct {
	Kind     RunPlanKind
	Settings Settings
}

// RunPlanOn is how this style runs on provider (docs/cleanup-models.md).
func (s *Style) RunPlanOn(p config.Provider) RunPlan {
	if s.IsRaw() {
		return RunPlan{}
	}
	if p.Kind == config.KindCleanupModel {
		model := ""
		if p.Model != nil {
			model = *p.Model
		}
		info, ok := FindCleanupModel(model)
		if !ok {
			return RunPlan{}
		}
		if c := s.CleanupSettingsOf(); c != nil && info.Runs(*c) {
			if known, ok := c.Known(); ok {
				return RunPlan{Kind: PlanCleanup, Settings: known}
			}
		}
		return RunPlan{Kind: PlanCleanupOnly}
	}
	if s.UsesAI() {
		return RunPlan{Kind: PlanPrompt}
	}
	return RunPlan{}
}

// StyleOrigin says where a style comes from.
type StyleOrigin int

const (
	OriginBuiltIn StyleOrigin = iota
	OriginOverridden
	OriginUser
)

// StyleEntry is a style in the library.
type StyleEntry struct {
	Style  Style
	Origin StyleOrigin
	Path   string // "" for a built-in
}

// StyleError is a style file failure. The texts are the Rust ones.
type StyleError struct {
	Kind    string // io, parse, bad_id
	Path    string
	Message string
}

func (e *StyleError) Error() string {
	switch e.Kind {
	case "io":
		return fmt.Sprintf("could not read %s: %s", e.Path, e.Message)
	case "parse":
		return fmt.Sprintf("%s is not a valid style file: %s", e.Path, e.Message)
	}
	return fmt.Sprintf("style id \"%s\" may contain only a-z, 0-9, and -", e.Message)
}

// DisplayNameKey is the i18n key for the name of a built-in style whose
// name is still the shipped English, else "" (show Style.Name).
func (s *Style) DisplayNameKey() string {
	if t, ok := styles.Find(s.ID); ok && t.Name == s.Name {
		return "styles.builtin." + s.ID + ".name"
	}
	return ""
}

// DisplayDescriptionKey is DisplayNameKey for the description.
func (s *Style) DisplayDescriptionKey() string {
	if t, ok := styles.Find(s.ID); ok && t.Description == s.Description {
		return "styles.builtin." + s.ID + ".description"
	}
	return ""
}

// BuiltinStyles are the seven styles Sayso ships: the shared texts of
// mobile/core/styles with the desktop's seal colors and cleanup settings.
func BuiltinStyles() []Style {
	cl := func(t Tone, l Layout, f Format) *CleanupSettings { c := NewCleanupSettings(t, l, f); return &c }
	look := map[string]struct {
		ink     string
		cleanup *CleanupSettings
	}{
		"clean":    {"#1D1B18", cl(ToneFormal, LayoutParagraphs, FormatPlain)},
		"polished": {"#2F3A4F", nil},
		"rambling": {"#6B4A6E", nil},
		"message":  {"#2D3C8C", cl(ToneCasual, LayoutParagraphs, FormatPlain)},
		"email":    {"#7A4A2B", cl(ToneFormal, LayoutParagraphs, FormatEmail)},
		"notes":    {"#3F6B5C", cl(ToneFormal, LayoutList, FormatPlain)},
		"raw":      {"#B8B1A4", nil},
	}
	out := make([]Style, 0, len(styles.Builtins))
	for _, b := range styles.Builtins {
		l := look[b.ID]
		st := Style{ID: b.ID, Name: b.Name, Ink: l.ink, Description: b.Description, Prompt: b.Prompt}
		if l.cleanup != nil {
			st.Cleanup = CleanupField{State: CleanupSettingsSet, Settings: *l.cleanup}
		}
		out = append(out, st)
	}
	return out
}

// DefaultBasePrompt is the cleanup rules and examples every style gets
// unless it is standalone (mobile/core/styles).
const DefaultBasePrompt = styles.DefaultBasePrompt

// SystemPrompt is the system prompt of a style. The transcript is data, not
// instructions. Only the dictionary words the transcript can contain go in.
func SystemPrompt(style *Style, base string, vocabulary []string, transcript string) string {
	return styles.SystemPrompt(style.Prompt, style.Standalone, base, vocabulary, transcript)
}

// ValidateStyleID allows only a-z, 0-9, and -.
func ValidateStyleID(id string) error {
	if !styles.ValidID(id) {
		return &StyleError{Kind: "bad_id", Message: id}
	}
	return nil
}

// StyleLibrary is the built-in styles merged with the files of a styles directory.
type StyleLibrary struct {
	Entries []StyleEntry
	// Errors are files that could not be loaded.
	Errors []string
	// UserBasePrompt is the user's base prompt; nil uses DefaultBasePrompt.
	UserBasePrompt *string
}

// LoadStyles reads the styles directory.
func LoadStyles(dir string) *StyleLibrary {
	byID := map[string]StyleEntry{}
	var order []string
	for _, st := range BuiltinStyles() {
		order = append(order, st.ID)
		byID[st.ID] = StyleEntry{Style: st, Origin: OriginBuiltIn}
	}
	lib := &StyleLibrary{Errors: []string{}}
	var files []string
	if entries, err := os.ReadDir(dir); err == nil {
		for _, e := range entries {
			if filepath.Ext(e.Name()) == ".toml" {
				files = append(files, filepath.Join(dir, e.Name()))
			}
		}
	}
	sort.Strings(files)
	for _, path := range files {
		st, err := readStyle(path)
		if err != nil {
			lib.Errors = append(lib.Errors, err.Error())
			continue
		}
		existing, exists := byID[st.ID]
		builtIn := exists && existing.Origin == OriginBuiltIn
		origin := OriginUser
		if builtIn {
			st.InheritedCleanup = existing.Style.CleanupSettingsOf()
			origin = OriginOverridden
		}
		if !exists {
			order = append(order, st.ID)
		}
		byID[st.ID] = StyleEntry{Style: *st, Origin: origin, Path: path}
	}
	for _, id := range order {
		if e, ok := byID[id]; ok {
			lib.Entries = append(lib.Entries, e)
			delete(byID, id)
		}
	}
	if data, err := os.ReadFile(filepath.Join(dir, BasePromptFile)); err == nil {
		s := string(data)
		lib.UserBasePrompt = &s
	}
	return lib
}

// BasePrompt is the user's base prompt or the default.
func (l *StyleLibrary) BasePrompt() string {
	if l.UserBasePrompt != nil {
		return *l.UserBasePrompt
	}
	return DefaultBasePrompt
}

// Get is the style with an id.
func (l *StyleLibrary) Get(id string) (*Style, bool) {
	for i := range l.Entries {
		if l.Entries[i].Style.ID == id {
			return &l.Entries[i].Style, true
		}
	}
	return nil, false
}

// Styles is every style, in order.
func (l *StyleLibrary) Styles() []Style {
	out := make([]Style, len(l.Entries))
	for i, e := range l.Entries {
		out[i] = e.Style
	}
	return out
}

func readStyle(path string) (*Style, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, &StyleError{Kind: "io", Path: path, Message: err.Error()}
	}
	st, err := ParseStyle(string(data))
	if err != nil {
		return nil, &StyleError{Kind: "parse", Path: path, Message: err.Error()}
	}
	if err := ValidateStyleID(st.ID); err != nil {
		return nil, err
	}
	return st, nil
}

// ParseStyle reads the text of a style file.
func ParseStyle(text string) (*Style, error) {
	doc, err := config.ParseTOML(text)
	if err != nil {
		return nil, err
	}
	st := &Style{Ink: defaultInk}
	str := func(key string, dst *string, required bool) error {
		v, ok := doc[key]
		if !ok {
			if required {
				return fmt.Errorf("missing field `%s`", key)
			}
			return nil
		}
		s, ok := v.(string)
		if !ok {
			return fmt.Errorf("invalid type for `%s`, expected a string", key)
		}
		*dst = s
		return nil
	}
	optStr := func(key string, dst **string) error {
		if _, ok := doc[key]; !ok {
			return nil
		}
		var s string
		if err := str(key, &s, false); err != nil {
			return err
		}
		*dst = &s
		return nil
	}
	for _, f := range []func() error{
		func() error { return str("id", &st.ID, true) },
		func() error { return str("name", &st.Name, true) },
		func() error { return str("ink", &st.Ink, false) },
		func() error { return str("description", &st.Description, false) },
		func() error { return str("prompt", &st.Prompt, false) },
		func() error { return optStr("provider", &st.Provider) },
		func() error { return optStr("model", &st.Model) },
	} {
		if err := f(); err != nil {
			return nil, err
		}
	}
	if v, ok := doc["temperature"]; ok {
		var f float32
		switch x := v.(type) {
		case float64:
			f = float32(x)
		case int64:
			f = float32(x)
		default:
			return nil, errors.New("invalid type for `temperature`, expected f32")
		}
		st.Temperature = &f
	}
	if v, ok := doc["timeout_ms"]; ok {
		n, ok := v.(int64)
		if !ok || n < 0 {
			return nil, errors.New("invalid value for `timeout_ms`, expected u64")
		}
		u := uint64(n)
		st.TimeoutMs = &u
	}
	if v, ok := doc["standalone"]; ok {
		b, ok := v.(bool)
		if !ok {
			return nil, errors.New("invalid type for `standalone`, expected a boolean")
		}
		st.Standalone = b
	}
	if v, ok := doc["cleanup"]; ok {
		switch x := v.(type) {
		case bool:
			if x {
				st.Cleanup = CleanupField{State: CleanupSettingsSet}
			} else {
				st.Cleanup = CleanupField{State: CleanupOff}
			}
		case map[string]any:
			var c CleanupSettings
			for key, dst := range map[string]**string{"tone": &c.Tone, "layout": &c.Layout, "format": &c.Format} {
				if raw, ok := x[key]; ok {
					s, ok := raw.(string)
					if !ok {
						return nil, errors.New("data did not match any variant of untagged enum CleanupFieldFile")
					}
					*dst = &s
				}
			}
			st.Cleanup = CleanupField{State: CleanupSettingsSet, Settings: c}
		default:
			return nil, errors.New("data did not match any variant of untagged enum CleanupFieldFile")
		}
	}
	return st, nil
}

// EncodeStyle writes a style as toml-rs's to_string_pretty does.
func EncodeStyle(st *Style) string {
	t := config.NewTable()
	t.Set("id", st.ID).Set("name", st.Name).Set("ink", st.Ink).Set("description", st.Description).Set("prompt", st.Prompt)
	if st.Provider != nil {
		t.Set("provider", *st.Provider)
	}
	if st.Model != nil {
		t.Set("model", *st.Model)
	}
	if st.Temperature != nil && !math.IsNaN(float64(*st.Temperature)) {
		t.Set("temperature", config.F32(*st.Temperature))
	}
	if st.TimeoutMs != nil {
		t.Set("timeout_ms", *st.TimeoutMs)
	}
	if st.Standalone {
		t.Set("standalone", true)
	}
	switch st.Cleanup.State {
	case CleanupOff:
		t.Set("cleanup", false)
	case CleanupSettingsSet:
		c := config.NewTable()
		if st.Cleanup.Settings.Tone != nil {
			c.Set("tone", *st.Cleanup.Settings.Tone)
		}
		if st.Cleanup.Settings.Layout != nil {
			c.Set("layout", *st.Cleanup.Settings.Layout)
		}
		if st.Cleanup.Settings.Format != nil {
			c.Set("format", *st.Cleanup.Settings.Format)
		}
		t.Set("cleanup", c)
	}
	return t.Encode()
}

// SaveStyle writes a style file, for new styles and edits of built-in ones.
func SaveStyle(dir string, st *Style) (string, error) {
	if err := ValidateStyleID(st.ID); err != nil {
		return "", err
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", &StyleError{Kind: "io", Path: dir, Message: err.Error()}
	}
	path := filepath.Join(dir, st.ID+".toml")
	if err := writeAtomic(path, EncodeStyle(st)); err != nil {
		return "", &StyleError{Kind: "io", Path: path, Message: err.Error()}
	}
	return path, nil
}

// ResetStyle deletes a style file; for a built-in id this restores the default.
func ResetStyle(dir, id string) error {
	if err := ValidateStyleID(id); err != nil {
		return err
	}
	path := filepath.Join(dir, id+".toml")
	if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return &StyleError{Kind: "io", Path: path, Message: err.Error()}
	}
	return nil
}

// SaveBasePrompt writes the user's base prompt. An empty text is valid.
func SaveBasePrompt(dir, text string) error {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return &StyleError{Kind: "io", Path: dir, Message: err.Error()}
	}
	path := filepath.Join(dir, BasePromptFile)
	if err := writeAtomic(path, strings.TrimSpace(text)); err != nil {
		return &StyleError{Kind: "io", Path: path, Message: err.Error()}
	}
	return nil
}

// ResetBasePrompt deletes the user's base prompt.
func ResetBasePrompt(dir string) error {
	path := filepath.Join(dir, BasePromptFile)
	if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return &StyleError{Kind: "io", Path: path, Message: err.Error()}
	}
	return nil
}

// writeAtomic writes <path with .tmp extension> and renames it, as Rust's with_extension("tmp").
func writeAtomic(path, text string) error {
	tmp := strings.TrimSuffix(path, filepath.Ext(path)) + ".tmp"
	if err := os.WriteFile(tmp, []byte(text), 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
