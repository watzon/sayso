package i18n

import (
	"strings"
	"unicode"
)

// Resolve returns the locale for the setting general.language. "system", and
// a value that is not an available locale, take the first system language
// that has one. The last choice is English.
func Resolve(setting string, system []string) string {
	avail := Available()
	codes := make([]string, len(avail))
	for i, l := range avail {
		codes[i] = l.Code
	}
	return resolveIn(setting, system, codes)
}

func resolveIn(setting string, system, codes []string) string {
	if setting == Pseudo && HasPseudo {
		return Pseudo
	}
	if setting != System {
		for _, c := range codes {
			if strings.EqualFold(c, setting) {
				return c
			}
		}
	}
	for _, raw := range system {
		tag := parseTag(raw)
		for _, c := range codes {
			if parseTag(c) == tag {
				return c
			}
		}
		for _, c := range codes {
			if tag.sameLanguage(parseTag(c)) {
				return c
			}
		}
	}
	return DefaultLocale
}

// tag holds the parts of a language tag that matter here.
type tag struct {
	language string
	// script is "Hans" or "Hant" for Chinese, from the script or the region.
	script string
	region string
}

// parseTag parses "zh-Hans-CN", "pt_BR", or the POSIX "zh_TW.UTF-8".
func parseTag(s string) tag {
	if i := strings.IndexAny(s, ".@"); i >= 0 {
		s = s[:i]
	}
	parts := strings.FieldsFunc(s, func(r rune) bool { return r == '-' || r == '_' })
	// FieldsFunc drops empty parts, unlike a split; a leading separator
	// would otherwise make the language empty.
	var t tag
	if len(parts) == 0 {
		return t
	}
	t.language = strings.ToLower(parts[0])
	for _, p := range parts[1:] {
		switch {
		case len(p) == 4 && isAlpha(p):
			t.script = strings.ToUpper(p[:1]) + strings.ToLower(p[1:])
		case len(p) == 2, len(p) == 3 && isDigits(p):
			t.region = strings.ToUpper(p)
		}
	}
	if t.language == "zh" && t.script == "" {
		switch t.region {
		case "TW", "HK", "MO":
			t.script = "Hant"
		default:
			t.script = "Hans"
		}
	}
	return t
}

// sameLanguage is the same language, and for Chinese also the same script.
func (t tag) sameLanguage(o tag) bool {
	return t.language == o.language && (t.language != "zh" || t.script == o.script)
}

func isAlpha(s string) bool {
	for _, r := range s {
		if r > unicode.MaxASCII || !unicode.IsLetter(r) {
			return false
		}
	}
	return true
}

func isDigits(s string) bool {
	for _, r := range s {
		if r < '0' || r > '9' {
			return false
		}
	}
	return true
}
