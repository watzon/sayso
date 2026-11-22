package i18n

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// pseudoTransform turns "Save" into "[Šåvé~~]". Placeholders stay as they
// are; the padding makes the text about 30% longer.
func pseudoTransform(text string) string {
	var b strings.Builder
	b.WriteByte('[')
	letters := 0
	rest := text
	for rest != "" {
		if strings.HasPrefix(rest, "%{") {
			if end := strings.IndexByte(rest, '}'); end >= 0 {
				b.WriteString(rest[:end+1])
				rest = rest[end+1:]
				continue
			}
		}
		c, size := utf8.DecodeRuneInString(rest)
		if unicode.IsLetter(c) {
			letters++
		}
		b.WriteRune(accent(c))
		rest = rest[size:]
	}
	b.WriteString(strings.Repeat("~", (letters+2)/3))
	b.WriteByte(']')
	return b.String()
}

var accents = map[rune]rune{
	'a': 'å', 'c': 'ç', 'e': 'é', 'i': 'î', 'n': 'ñ', 'o': 'ö', 's': 'š', 'u': 'ü', 'y': 'ý', 'z': 'ž',
	'A': 'Å', 'C': 'Ç', 'E': 'É', 'I': 'Î', 'N': 'Ñ', 'O': 'Ö', 'S': 'Š', 'U': 'Ü', 'Y': 'Ý', 'Z': 'Ž',
}

func accent(c rune) rune {
	if a, ok := accents[c]; ok {
		return a
	}
	return c
}
