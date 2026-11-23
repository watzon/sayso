package theme

import "strings"

// serifFallbacks are the fonts for the characters the bundled serifs lack,
// per language (sayso-ui/src/fonts.rs fallbacks, Face::Serif): macOS first,
// then Windows, then Linux. Chinese and Japanese share characters with
// different shapes, so the system must not choose: Japanese text gets a
// Japanese serif and Chinese a Chinese one; Korean the same. The bundled
// fonts have no Cyrillic, and the system would draw Russian serif text in a
// sans face, so Russian gets Georgia. Other languages leave it to the system.
var serifFallbacks = map[string][]string{
	"ja": {"Hiragino Mincho ProN", "Yu Mincho", "Noto Serif CJK JP"},
	"zh": {"Songti SC", "SimSun", "Noto Serif CJK SC"},
	"ko": {"AppleMyungjo", "Batang", "Noto Serif CJK KR"},
	"ru": {"Georgia", "Noto Serif"},
	// Arabic script (Arabic, Persian, Urdu) in a Naskh, the book hand that
	// sits with a serif; Hebrew in its serif faces.
	"ar": {"Al Nile", "Geeza Pro", "Sakkal Majalla", "Traditional Arabic", "Noto Naskh Arabic"},
	"fa": {"Al Nile", "Geeza Pro", "Sakkal Majalla", "Traditional Arabic", "Noto Naskh Arabic"},
	"ur": {"Al Nile", "Geeza Pro", "Sakkal Majalla", "Traditional Arabic", "Noto Naskh Arabic"},
	"he": {"New Peninim MT", "Times New Roman", "David", "Noto Serif Hebrew"},
}

// sansFallbacks are the fonts for the scripts Instrument Sans lacks that
// the system could draw in a mismatched or blurry face: the system UI face
// of the script on macOS, then Windows, then Linux.
var sansFallbacks = map[string][]string{
	"ar": {"SF Arabic", "Geeza Pro", "Segoe UI", "Segoe UI Arabic", "Tahoma", "Noto Sans Arabic", "Noto Naskh Arabic"},
	"fa": {"SF Arabic", "Geeza Pro", "Segoe UI", "Segoe UI Arabic", "Tahoma", "Noto Sans Arabic", "Noto Naskh Arabic"},
	"ur": {"SF Arabic", "Geeza Pro", "Segoe UI", "Segoe UI Arabic", "Tahoma", "Noto Sans Arabic", "Noto Naskh Arabic"},
	"he": {"SF Hebrew", "Arial Hebrew", "Segoe UI", "Tahoma", "Noto Sans Hebrew"},
}

// SerifFor is a serif family (Display or Body) followed by the fallbacks
// of locale, as a comma-separated family list.
func SerifFor(family, locale string) string { return withFallbacks(family, serifFallbacks, locale) }

// UIFor is the interface face (UI) followed by the fallbacks of locale; UI
// alone for a locale whose script it has or the system draws well.
func UIFor(locale string) string { return withFallbacks(UI, sansFallbacks, locale) }

func withFallbacks(family string, fallbacks map[string][]string, locale string) string {
	lang, _, _ := strings.Cut(strings.ReplaceAll(locale, "_", "-"), "-")
	if fb := fallbacks[strings.ToLower(lang)]; len(fb) > 0 {
		return family + ", " + strings.Join(fb, ", ")
	}
	return family
}
