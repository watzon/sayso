package i18n

import (
	"strconv"
	"strings"
)

// nbsp is the French and Russian group separator. CLDR gives French the
// narrow no-break space (U+202F), but the bundled fonts do not have it.
const nbsp = " "

type separators struct {
	decimal, group string
	// minGrouping is the fewest digits before the first separator: Spanish
	// writes "1234" but "12 345".
	minGrouping int
}

func separatorsFor(locale string) separators {
	switch languageOf(locale) {
	case "de", "it", "pt":
		return separators{",", ".", 1}
	case "es":
		return separators{",", ".", 2}
	case "fr", "ru":
		return separators{",", nbsp, 1}
	}
	return separators{".", ",", 1}
}

// Integer is a whole number with group separators in the current locale:
// "1,234", "1.234", "1 234".
func Integer(n uint64) string { return IntegerIn(CurrentLocale(), n) }

// Decimal is a number with places decimals in the current locale.
func Decimal(value float64, places int) string { return DecimalIn(CurrentLocale(), value, places) }

// IntegerIn is Integer in a given locale.
func IntegerIn(locale string, n uint64) string {
	s := separatorsFor(locale)
	digits := strconv.FormatUint(n, 10)
	if len(digits) < 4+s.minGrouping-1 {
		return digits
	}
	var b strings.Builder
	for i, d := range digits {
		if i > 0 && (len(digits)-i)%3 == 0 {
			b.WriteString(s.group)
		}
		b.WriteRune(d)
	}
	return b.String()
}

// DecimalIn is Decimal in a given locale.
func DecimalIn(locale string, value float64, places int) string {
	s := separatorsFor(locale)
	text := strconv.FormatFloat(value, 'f', places, 64)
	whole, fraction, hasFraction := strings.Cut(text, ".")
	sign := ""
	if strings.HasPrefix(whole, "-") {
		sign, whole = "-", whole[1:]
	}
	if n, err := strconv.ParseUint(whole, 10, 64); err == nil {
		whole = IntegerIn(locale, n)
	}
	if hasFraction {
		return sign + whole + s.decimal + fraction
	}
	return sign + whole
}
