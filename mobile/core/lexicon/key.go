package lexicon

import "unicode/utf8"

// A key is the letters a word is typed with: lower-case a to z, with accents
// folded ("café" → "cafe") and apostrophes dropped ("don't" → "dont"). The
// trie and the decoder work on keys; the word's spelling is kept apart.

// MaxKeyLen is the longest key the model holds and the decoder searches.
const MaxKeyLen = 32

// AppendKey appends the key of word to dst. It reports false when word has
// a character that is not a letter or an apostrophe, or folds to no letter.
// It does not allocate when dst has room.
func AppendKey(dst []byte, word []byte) ([]byte, bool) {
	start := len(dst)
	for i := 0; i < len(word); {
		c := word[i]
		if c < utf8.RuneSelf {
			i++
			switch {
			case c >= 'a' && c <= 'z':
				dst = append(dst, c)
			case c >= 'A' && c <= 'Z':
				dst = append(dst, c+'a'-'A')
			case c == '\'':
			default:
				return dst[:start], false
			}
			continue
		}
		r, size := utf8.DecodeRune(word[i:])
		i += size
		if r == '’' {
			continue
		}
		f := FoldRune(r)
		if f == 0 {
			return dst[:start], false
		}
		dst = append(dst, f)
	}
	return dst, len(dst) > start
}

// KeyOf is AppendKey for a string, for code off the hot path.
func KeyOf(word string) (string, bool) {
	var buf [MaxKeyLen * 2]byte
	k, ok := AppendKey(buf[:0], []byte(word))
	return string(k), ok
}

// FoldRune is the a-to-z letter a rune folds to, or 0. It covers ASCII and
// the Latin-1 and Latin Extended-A letters English borrows.
func FoldRune(r rune) byte {
	switch {
	case r >= 'a' && r <= 'z':
		return byte(r)
	case r >= 'A' && r <= 'Z':
		return byte(r) + 'a' - 'A'
	}
	switch r {
	case 'à', 'á', 'â', 'ã', 'ä', 'å', 'À', 'Á', 'Â', 'Ã', 'Ä', 'Å', 'ā', 'Ā', 'ă', 'Ă', 'ą', 'Ą':
		return 'a'
	case 'ç', 'Ç', 'ć', 'Ć', 'č', 'Č':
		return 'c'
	case 'ď', 'Ď':
		return 'd'
	case 'è', 'é', 'ê', 'ë', 'È', 'É', 'Ê', 'Ë', 'ē', 'Ē', 'ė', 'Ė', 'ę', 'Ę', 'ě', 'Ě':
		return 'e'
	case 'ğ', 'Ğ':
		return 'g'
	case 'ì', 'í', 'î', 'ï', 'Ì', 'Í', 'Î', 'Ï', 'ī', 'Ī', 'ı':
		return 'i'
	case 'ł', 'Ł':
		return 'l'
	case 'ñ', 'Ñ', 'ń', 'Ń', 'ň', 'Ň':
		return 'n'
	case 'ò', 'ó', 'ô', 'õ', 'ö', 'ø', 'Ò', 'Ó', 'Ô', 'Õ', 'Ö', 'Ø', 'ō', 'Ō', 'ő', 'Ő':
		return 'o'
	case 'ř', 'Ř':
		return 'r'
	case 'ś', 'Ś', 'š', 'Š', 'ş', 'Ş':
		return 's'
	case 'ť', 'Ť', 'ţ', 'Ţ':
		return 't'
	case 'ù', 'ú', 'û', 'ü', 'Ù', 'Ú', 'Û', 'Ü', 'ū', 'Ū', 'ů', 'Ů', 'ű', 'Ű':
		return 'u'
	case 'ý', 'ÿ', 'Ý', 'Ÿ':
		return 'y'
	case 'ź', 'Ź', 'ż', 'Ż', 'ž', 'Ž':
		return 'z'
	}
	return 0
}
