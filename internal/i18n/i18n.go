// Package i18n is the text of the interface in each language. It is the Go
// port of crates/sayso-i18n.
//
// Each locale has a folder of YAML catalogs under locales/<code>/, compiled
// into the binary. The catalogs are flattened into dot-separated keys:
//
//	i18n.T("settings.general.title")
//	i18n.T("home.greeting", "name", "Ada")
//	i18n.TP("history.dictations", 3, nil)
//
// A text with a count has one key for each plural category of the locale
// (history.dictations.one, history.dictations.other).
package i18n

import (
	"embed"
	"fmt"
	"io/fs"
	"path"
	"runtime"
	"sort"
	"strings"
	"sync"

	"gopkg.in/yaml.v3"
)

//go:embed locales/*/*.yml
var localesFS embed.FS

// Locale is a locale that the Language menu shows.
type Locale struct {
	Code string
	// Name is the name of the language in that language: "Español", "日本語".
	Name string
	// Draft is a first translation that no native speaker has reviewed yet.
	Draft bool
}

// Locales are the locales with a catalog folder.
var Locales = []Locale{
	{Code: "en", Name: "English"},
	{Code: "es", Name: "Español"},
	{Code: "pt-BR", Name: "Português (Brasil)"},
	{Code: "zh-CN", Name: "简体中文"},
	{Code: "ja", Name: "日本語"},
	{Code: "de", Name: "Deutsch"},
	{Code: "fr", Name: "Français"},
	{Code: "it", Name: "Italiano"},
	{Code: "ko", Name: "한국어"},
	{Code: "ru", Name: "Русский"},
	{Code: "ar", Name: "العربية"},
}

const (
	// System is the value of general.language that follows the system.
	System = "system"
	// Pseudo is the development pseudo-locale: the English text with accents,
	// in brackets and padded by about 30%.
	Pseudo = "en-XA"
	// DefaultLocale is the fallback of every lookup.
	DefaultLocale = "en"
)

// HasPseudo reports whether this build offers the pseudo-locale.
const HasPseudo = true

// Available returns the locales that the Language menu offers and that
// "system" can select: those without the draft flag.
func Available() []Locale {
	var out []Locale
	for _, l := range Locales {
		if !l.Draft {
			out = append(out, l)
		}
	}
	return out
}

// SupportedLocales returns the codes of all locales with a catalog.
func SupportedLocales() []string {
	out := make([]string, len(Locales))
	for i, l := range Locales {
		out[i] = l.Code
	}
	return out
}

// Find returns the locale of a code, compared without regard to case.
func Find(code string) (Locale, bool) {
	for _, l := range Locales {
		if strings.EqualFold(l.Code, code) {
			return l, true
		}
	}
	return Locale{}, false
}

// IsValidSetting reports whether value can be general.language.
func IsValidSetting(value string) bool {
	if value == System || (value == Pseudo && HasPseudo) {
		return true
	}
	_, ok := Find(value)
	return ok
}

// LanguageName is the name of a language in the interface language:
// "Spanish" in English, "スペイン語" in Japanese. code is a dictation
// language ("af", "yue") or a locale ("pt-BR"). A code without a name in any
// catalog gives the code.
func LanguageName(code string) string {
	if text, ok := Translate("languages." + code); ok {
		return text
	}
	if l, ok := Find(code); ok {
		return T("language." + l.Code)
	}
	return code
}

// MenuName is how the Language menu shows the setting value code: its name
// in the interface language ("French" in English) and, after it, its name
// in that language ("Français"). native is empty where it would repeat
// name and for System and the pseudo-locale, which have no language of
// their own. The interface language is its native name alone: a catalog
// can name it more loosely ("Português" for pt-BR).
func MenuName(code string) (name, native string) {
	switch code {
	case System:
		return T("settings.general.language_system"), ""
	case Pseudo:
		return "Pseudo (en-XA)", ""
	}
	l, ok := Find(code)
	if !ok {
		return code, ""
	}
	if l.Code == CurrentLocale() {
		return l.Name, ""
	}
	name = T("language." + l.Code)
	if name == l.Name {
		return name, ""
	}
	return name, l.Name
}

var (
	catalogsOnce sync.Once
	catalogs     map[string]map[string]string

	mu      sync.RWMutex
	current = DefaultLocale
)

func loadCatalogs() {
	catalogs = make(map[string]map[string]string, len(Locales))
	for _, l := range Locales {
		m, err := loadLocale(localesFS, path.Join("locales", l.Code))
		if err != nil {
			panic(fmt.Sprintf("i18n: locale %s: %v", l.Code, err))
		}
		catalogs[l.Code] = m
	}
}

func loadLocale(fsys fs.FS, dir string) (map[string]string, error) {
	entries, err := fs.ReadDir(fsys, dir)
	if err != nil {
		return nil, err
	}
	out := map[string]string{}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".yml") {
			continue
		}
		data, err := fs.ReadFile(fsys, path.Join(dir, e.Name()))
		if err != nil {
			return nil, err
		}
		if err := flattenYAML(data, out); err != nil {
			return nil, fmt.Errorf("%s: %w", e.Name(), err)
		}
	}
	return out, nil
}

// flattenYAML adds the leaves of a catalog to out under their dotted keys.
// The top-level _version key is skipped.
func flattenYAML(data []byte, out map[string]string) error {
	var root map[string]any
	if err := yaml.Unmarshal(data, &root); err != nil {
		return err
	}
	delete(root, "_version")
	flatten("", root, out)
	return nil
}

func flatten(prefix string, v any, out map[string]string) {
	switch t := v.(type) {
	case map[string]any:
		for k, child := range t {
			flatten(join(prefix, k), child, out)
		}
	case map[any]any:
		for k, child := range t {
			flatten(join(prefix, fmt.Sprint(k)), child, out)
		}
	case nil:
	case string:
		out[prefix] = t
	default:
		out[prefix] = fmt.Sprint(t)
	}
}

func join(prefix, k string) string {
	if prefix == "" {
		return k
	}
	return prefix + "." + k
}

func catalog(locale string) map[string]string {
	catalogsOnce.Do(loadCatalogs)
	return catalogs[locale]
}

// Messages returns all keys and texts of a locale's catalog, or nil for a
// locale without one. The map is a copy.
func Messages(locale string) map[string]string {
	c := catalog(canonical(locale))
	if c == nil {
		return nil
	}
	out := make(map[string]string, len(c))
	for k, v := range c {
		out[k] = v
	}
	return out
}

// canonical returns the catalog code for a locale code compared without
// regard to case, or the input when there is none.
func canonical(locale string) string {
	if l, ok := Find(locale); ok {
		return l.Code
	}
	return locale
}

// SetLocale uses this locale for all text from now on. An unknown code
// selects English; the pseudo-locale is kept as it is.
func SetLocale(code string) {
	if strings.EqualFold(code, Pseudo) {
		code = Pseudo
	} else if l, ok := Find(code); ok {
		code = l.Code
	} else {
		code = DefaultLocale
	}
	mu.Lock()
	current = code
	mu.Unlock()
}

// CurrentLocale returns the current locale code.
func CurrentLocale() string {
	mu.RLock()
	defer mu.RUnlock()
	return current
}

// Translate returns the text of a key in the current locale, with the
// fallback to English. ok is false when no catalog has the key.
func Translate(key string) (string, bool) {
	return lookup(CurrentLocale(), key)
}

func lookup(locale, key string) (string, bool) {
	if locale == Pseudo {
		text, ok := catalog(DefaultLocale)[key]
		if !ok {
			return "", false
		}
		return pseudoTransform(text), true
	}
	if text, ok := catalog(locale)[key]; ok {
		return text, true
	}
	text, ok := catalog(DefaultLocale)[key]
	return text, ok
}

// T returns the text of a key in the current locale. args are name and value
// pairs ("name", "Ada") or one map[string]any. A key without a text gives
// the key.
func T(key string, args ...any) string {
	return TMap(key, argsToMap(args))
}

// TMap is T with the arguments in a map: each %{name} becomes its value.
func TMap(key string, vars map[string]any) string {
	text, ok := lookup(CurrentLocale(), key)
	if !ok {
		text = key
	}
	return interpolate(text, vars)
}

// TP returns the plural text of key for count: it looks up key.<category>
// and then key.other. count is also available as %{count}.
func TP(key string, count int, vars map[string]any) string {
	locale := CurrentLocale()
	n := uint64(count)
	if count < 0 {
		n = uint64(-int64(count))
	}
	text, ok := lookup(locale, key+"."+pluralCategory(locale, n))
	if !ok {
		text, ok = lookup(locale, key+".other")
	}
	if !ok {
		text = key
	}
	all := map[string]any{"count": count}
	for k, v := range vars {
		all[k] = v
	}
	return interpolate(text, all)
}

// osSuffix is the suffix of keys that differ by operating system.
func osSuffix(goos string) string {
	switch goos {
	case "darwin":
		return "_mac"
	case "windows":
		return "_windows"
	}
	return "_other"
}

// TOS returns the text of key with the suffix of this operating system
// (key_mac, key_windows, key_other). A system without its own text gets
// key_other, and then the plain key.
func TOS(key string, vars map[string]any) string {
	return tos(runtime.GOOS, key, vars)
}

func tos(goos, key string, vars map[string]any) string {
	locale := CurrentLocale()
	for _, k := range []string{key + osSuffix(goos), key + "_other", key} {
		if text, ok := lookup(locale, k); ok {
			return interpolate(text, vars)
		}
	}
	return key
}

func argsToMap(args []any) map[string]any {
	if len(args) == 1 {
		if m, ok := args[0].(map[string]any); ok {
			return m
		}
	}
	m := make(map[string]any, len(args)/2)
	for i := 0; i+1 < len(args); i += 2 {
		m[fmt.Sprint(args[i])] = args[i+1]
	}
	return m
}

// interpolate puts each value in the place of its %{name}.
func interpolate(text string, vars map[string]any) string {
	if len(vars) == 0 || !strings.Contains(text, "%{") {
		return text
	}
	names := make([]string, 0, len(vars))
	for k := range vars {
		names = append(names, k)
	}
	sort.Strings(names)
	rtl := IsRTL()
	for _, name := range names {
		v := fmt.Sprint(vars[name])
		if rtl {
			v = isolateArg(v)
		}
		text = strings.ReplaceAll(text, "%{"+name+"}", v)
	}
	return text
}
