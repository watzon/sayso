package theme

import "testing"

func TestSerifForGivesCJKAndRussianTheirOwnFonts(t *testing.T) {
	for locale, want := range map[string]string{
		"ja":    "Fraunces, Hiragino Mincho ProN, Yu Mincho, Noto Serif CJK JP",
		"zh-CN": "Fraunces, Songti SC, SimSun, Noto Serif CJK SC",
		"ko":    "Fraunces, AppleMyungjo, Batang, Noto Serif CJK KR",
		"ru":    "Fraunces, Georgia, Noto Serif",
		"pt_BR": "Fraunces",
		"en":    "Fraunces",
		"":      "Fraunces",
	} {
		if got := SerifFor(Display, locale); got != want {
			t.Errorf("%q: got %q, want %q", locale, got, want)
		}
	}
}
