package i18n

import (
	"os"
	"strings"
	"sync/atomic"
)

// rtlLanguages are the languages of Sayso's locales, present or planned,
// that are written right to left.
var rtlLanguages = map[string]bool{"ar": true, "he": true, "fa": true, "ur": true}

// IsRTLTag reports whether a locale tag ("ar", "he-IL", "fa_IR") names a
// language written right to left. Only the language subtag counts.
func IsRTLTag(tag string) bool {
	lang, _, _ := strings.Cut(strings.ReplaceAll(tag, "_", "-"), "-")
	return rtlLanguages[strings.ToLower(lang)]
}

// directionOverride is 0 to follow the locale, 1 for left to right and 2
// for right to left (OverrideDirection).
var directionOverride atomic.Int32

// OverrideDirection lays the interface out "ltr" or "rtl" whatever the
// locale, or follows the locale again for "". It lets tests and the
// screenshot generator try a layout without its translation.
func OverrideDirection(d string) {
	switch d {
	case "ltr":
		directionOverride.Store(1)
	case "rtl":
		directionOverride.Store(2)
	default:
		directionOverride.Store(0)
	}
}

// IsRTL reports whether the interface reads right to left: the override,
// else MYGO_DIRECTION (MyGo's own switch to try a layout, which its
// windows follow too), else the current locale.
func IsRTL() bool {
	switch directionOverride.Load() {
	case 1:
		return false
	case 2:
		return true
	}
	switch os.Getenv("MYGO_DIRECTION") {
	case "ltr":
		return false
	case "rtl":
		return true
	}
	return IsRTLTag(CurrentLocale())
}
