package i18n

import "strings"

func languageOf(locale string) string {
	if i := strings.IndexAny(locale, "-_"); i >= 0 {
		return locale[:i]
	}
	return locale
}

// pluralCategory is the CLDR plural category of a whole number. Only the
// languages of the catalogs are here; another language gets the English rule.
func pluralCategory(locale string, n uint64) string {
	switch languageOf(locale) {
	case "ja", "zh", "ko":
		return "other"
	case "pt":
		if !strings.EqualFold(locale, "pt-PT") {
			switch {
			case n == 0 || n == 1:
				return "one"
			case isMillion(n):
				return "many"
			}
			return "other"
		}
		fallthrough
	case "es", "it":
		switch {
		case n == 1:
			return "one"
		case isMillion(n):
			return "many"
		}
		return "other"
	case "fr":
		switch {
		case n == 0 || n == 1:
			return "one"
		case isMillion(n):
			return "many"
		}
		return "other"
	case "ru":
		m10, m100 := n%10, n%100
		switch {
		case m10 == 1 && m100 != 11:
			return "one"
		case m10 >= 2 && m10 <= 4 && !(m100 >= 12 && m100 <= 14):
			return "few"
		}
		return "many"
	case "ar":
		m100 := n % 100
		switch {
		case n == 0:
			return "zero"
		case n == 1:
			return "one"
		case n == 2:
			return "two"
		case m100 >= 3 && m100 <= 10:
			return "few"
		case m100 >= 11:
			return "many"
		}
		return "other"
	}
	if n == 1 {
		return "one"
	}
	return "other"
}

// PluralCategories are the categories that the counts of a locale need.
func PluralCategories(locale string) []string {
	switch languageOf(locale) {
	case "ja", "zh", "ko":
		return []string{"other"}
	case "es", "pt", "fr", "it":
		return []string{"one", "many", "other"}
	case "ru":
		return []string{"one", "few", "many", "other"}
	case "ar":
		return []string{"zero", "one", "two", "few", "many", "other"}
	}
	return []string{"one", "other"}
}

// isMillion is CLDR "many" in the Romance languages: a whole number of
// millions.
func isMillion(n uint64) bool { return n != 0 && n%1_000_000 == 0 }
