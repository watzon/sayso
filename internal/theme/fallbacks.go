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
}

// SerifFor is a serif family (Display or Body) followed by the fallbacks
// of locale, as a comma-separated family list.
func SerifFor(family, locale string) string {
	lang, _, _ := strings.Cut(strings.ReplaceAll(locale, "_", "-"), "-")
	if fb := serifFallbacks[lang]; len(fb) > 0 {
		return family + ", " + strings.Join(fb, ", ")
	}
	return family
}
