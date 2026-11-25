// Package languages holds the language codes of the dictation setting and the
// model catalog. Codes are ISO 639-1
// where one exists, else ISO 639-3 ("yue").
package languages

import (
	"sort"
	"strings"
	"sync"
)

// Auto is the dictation.language value that lets the model detect the language.
const Auto = "auto"

type pair struct{ code, name string }

// Name is the English name of a code, or "" when Sayso does not know it.
func Name(code string) string {
	for _, p := range names {
		if p.code == code {
			return p.name
		}
	}
	return ""
}

// IsValid is true for Auto and for a code Sayso knows.
func IsValid(code string) bool { return code == Auto || Name(code) != "" }

var (
	allOnce sync.Once
	all     []string
)

// All is every known code, sorted by English name.
func All() []string {
	allOnce.Do(func() {
		list := append([]pair(nil), names...)
		sort.SliceStable(list, func(i, j int) bool { return list[i].name < list[j].name })
		for _, p := range list {
			all = append(all, p.code)
		}
	})
	return all
}

// FromSystem picks the dictation language for the system languages, in order
// of preference: the first one Sayso knows, else English. "zh-Hans-CN" or
// "pt_BR" give their language ("zh", "pt").
func FromSystem(system []string) string {
	for _, tag := range system {
		primary := tag
		if i := strings.IndexAny(tag, "-_.@"); i >= 0 {
			primary = tag[:i]
		}
		primary = strings.ToLower(primary)
		switch primary {
		case "nb":
			primary = "no"
		case "fil":
			primary = "tl"
		case "iw":
			primary = "he"
		case "in":
			primary = "id"
		case "jv":
			primary = "jw"
		}
		if Name(primary) != "" {
			return primary
		}
	}
	return "en"
}

func English() []string     { return englishList }
func Whisper() []string     { return whisperList }
func ParakeetV3() []string  { return parakeetv3List }
func Cohere() []string      { return cohereList }
func SenseVoice() []string  { return sensevoiceList }
func Nemotron() []string    { return nemotronList }
func AppleSpeech() []string { return applespeechList }
