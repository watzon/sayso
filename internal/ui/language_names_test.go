package ui

import (
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models/languages"
)

// Every dictation language shows a name, never its bare code. The Whisper
// list used to end in "af", "ar", ... because only the interface locales
// had names.
func TestDictationLanguagesShowNames(t *testing.T) {
	prev := i18n.CurrentLocale()
	t.Cleanup(func() { i18n.SetLocale(prev) })
	codes := append(append([]string{}, languages.All()...), languages.Whisper()...)
	for _, locale := range i18n.SupportedLocales() {
		i18n.SetLocale(locale)
		for _, code := range codes {
			name := dictationLanguageName(code)
			if name == code || strings.HasPrefix(name, "languages.") || name == "" {
				t.Errorf("%s: %s shows as %q", locale, code, name)
			}
		}
		for _, code := range sortedLanguages() {
			if dictationLanguageName(code) == code {
				t.Errorf("%s: the sorted list has the bare code %s", locale, code)
			}
		}
	}
	// English and Arabic have their own name for every code; English matches
	// the names of the language table.
	i18n.SetLocale("en")
	en := map[string]string{}
	for _, code := range languages.Whisper() {
		en[code] = dictationLanguageName(code)
		if want := languages.Name(code); en[code] != want {
			t.Errorf("en: %s = %q, want %q", code, en[code], want)
		}
	}
	i18n.SetLocale("ar")
	ar := i18n.Messages("ar")
	for _, code := range languages.Whisper() {
		if _, ok := ar["languages."+code]; !ok {
			t.Errorf("ar: no name for %s", code)
		}
		if got := dictationLanguageName(code); got == en[code] && code != "auto" {
			t.Errorf("ar: %s shows the English name %q", code, got)
		}
	}
}
