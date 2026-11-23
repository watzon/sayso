package i18n

import "testing"

// The Language menu names each language in the interface language and, in
// gray after it, in its own language, unless the two are the same.
func TestMenuName(t *testing.T) {
	t.Cleanup(func() { SetLocale(DefaultLocale) })
	type pair struct{ name, native string }
	for locale, want := range map[string]map[string]pair{
		"en": {
			"en":    {"English", ""},
			"fr":    {"French", "Français"},
			"ja":    {"Japanese", "日本語"},
			"ar":    {"Arabic", "العربية"},
			"zh-CN": {"Chinese, Simplified", "简体中文"},
			System:  {"System", ""},
			Pseudo:  {"Pseudo (en-XA)", ""},
		},
		"ar": {
			"ar":   {"العربية", ""},
			"en":   {"الإنجليزية", "English"},
			"fr":   {"الفرنسية", "Français"},
			"ja":   {"اليابانية", "日本語"},
			System: {"لغة النظام", ""},
		},
		"zh-CN": {
			"zh-CN": {"简体中文", ""},
			"ko":    {"韩语", "한국어"},
		},
	} {
		SetLocale(locale)
		for code, w := range want {
			name, native := MenuName(code)
			if name != w.name || native != w.native {
				t.Errorf("%s: MenuName(%q) = %q, %q; want %q, %q", locale, code, name, native, w.name, w.native)
			}
		}
	}
	// Every locale names every language of the menu in its own words, and
	// shows the native name only where it differs.
	for _, locale := range SupportedLocales() {
		SetLocale(locale)
		for _, l := range Available() {
			name, native := MenuName(l.Code)
			if name == "" || name == "language."+l.Code {
				t.Errorf("%s: no name for %s", locale, l.Code)
			}
			if native == name {
				t.Errorf("%s: %s repeats %q", locale, l.Code, name)
			}
			if native != "" && native != l.Name {
				t.Errorf("%s: %s native = %q, want %q", locale, l.Code, native, l.Name)
			}
		}
		if _, native := MenuName(locale); native != "" {
			t.Errorf("%s: the active language shows its native name %q again", locale, native)
		}
	}
}
