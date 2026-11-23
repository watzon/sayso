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

func TestArabicAndHebrewHaveSansAndSerifFallbacks(t *testing.T) {
	for _, locale := range []string{"ar", "ar-EG", "fa", "ur", "he"} {
		if UIFor(locale) == UI {
			t.Errorf("%s: UIFor has no fallbacks", locale)
		}
		if SerifFor(Display, locale) == Display {
			t.Errorf("%s: SerifFor has no fallbacks", locale)
		}
	}
	want := "Instrument Sans, SF Arabic, Geeza Pro, Segoe UI, Segoe UI Arabic, Tahoma, Noto Sans Arabic, Noto Naskh Arabic"
	if got := UIFor("ar"); got != want {
		t.Errorf("UIFor(ar) = %q, want %q", got, want)
	}
	for _, locale := range []string{"en", "pt-BR", "ja", "ru", ""} {
		if got := UIFor(locale); got != UI {
			t.Errorf("%q: UIFor = %q, want the bundled face alone", locale, got)
		}
	}
}
