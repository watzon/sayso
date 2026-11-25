package i18n

import (
	"fmt"
	"sort"
	"strings"
	"testing"
	"time"
)

// Leaf counts per locale, counted independently with Ruby's YAML parser.
var wantLeaves = map[string]int{
	"en": 1888, "es": 1916, "pt-BR": 1916, "zh-CN": 1860, "ja": 1860,
	"de": 1888, "fr": 1916, "it": 1916, "ko": 1860, "ru": 1944, "ar": 2000,
}

func use(t *testing.T, code string) {
	t.Helper()
	prev := CurrentLocale()
	SetLocale(code)
	t.Cleanup(func() { SetLocale(prev) })
}

func TestAllLocalesLoadWithLeafCounts(t *testing.T) {
	if len(Locales) != 11 || len(SupportedLocales()) != 11 {
		t.Fatalf("want 11 locales, got %d", len(Locales))
	}
	for _, l := range Locales {
		m := Messages(l.Code)
		if len(m) != wantLeaves[l.Code] {
			t.Errorf("%s: %d leaves, want %d", l.Code, len(m), wantLeaves[l.Code])
		}
		if _, ok := m["_version"]; ok {
			t.Errorf("%s: _version leaked into the catalog", l.Code)
		}
	}
	if n := len(Messages("en")); n < 1800 || n > 1950 {
		t.Errorf("en has %d leaves, expected about 1,870", n)
	}
	if Messages("xx") != nil {
		t.Error("unknown locale has a catalog")
	}
}

func TestFortyFilesPerLocale(t *testing.T) {
	for _, l := range Locales {
		entries, err := localesFS.ReadDir("locales/" + l.Code)
		if err != nil || len(entries) != 40 {
			t.Errorf("%s: %d files (err %v), want 40", l.Code, len(entries), err)
		}
	}
}

func TestEveryLocaleHasEveryEnglishKey(t *testing.T) {
	en := Messages("en")
	for _, l := range Locales {
		m := Messages(l.Code)
		var missing []string
		for k := range en {
			if _, ok := m[k]; !ok && !isPluralKey(k) {
				missing = append(missing, k)
			}
		}
		sort.Strings(missing)
		if len(missing) > 0 {
			t.Errorf("%s misses %d keys, first: %v", l.Code, len(missing), missing[:min(5, len(missing))])
		}
	}
}

// isPluralKey is a key under a plural text: locales differ in categories.
func isPluralKey(k string) bool {
	for _, c := range []string{".zero", ".one", ".two", ".few", ".many", ".other"} {
		if strings.HasSuffix(k, c) {
			return true
		}
	}
	return false
}

func TestEveryPluralHasItsCategories(t *testing.T) {
	for _, l := range Locales {
		m := Messages(l.Code)
		bases := map[string]bool{}
		for k := range m {
			if i := strings.LastIndexByte(k, '.'); i > 0 && isPluralKey(k) {
				bases[k[:i]] = true
			}
		}
		for base := range bases {
			for _, c := range append(PluralCategories(l.Code), "other") {
				if _, ok := m[base+"."+c]; !ok {
					t.Errorf("%s: %s has no .%s", l.Code, base, c)
				}
			}
		}
	}
}

func TestLookupAndInterpolation(t *testing.T) {
	use(t, "en")
	if got := T("date.today"); got != "Today" {
		t.Errorf("date.today = %q", got)
	}
	if got := T("date.pattern.day_month", "month", "September", "day", 28); got != "September 28" {
		t.Errorf("pattern = %q", got)
	}
	if got := TMap("date.pattern.day_month", map[string]any{"month": "May", "day": 1}); got != "May 1" {
		t.Errorf("TMap = %q", got)
	}
	if got := T("no.such.key"); got != "no.such.key" {
		t.Errorf("missing key = %q", got)
	}
	if got := interpolate("Hi, %{name}. %{name}!", map[string]any{"name": "Ada"}); got != "Hi, Ada. Ada!" {
		t.Errorf("interpolate = %q", got)
	}
	if got := interpolate("%{other} stays", map[string]any{"name": "Ada"}); got != "%{other} stays" {
		t.Errorf("unknown placeholder = %q", got)
	}
}

func TestLiveSwitching(t *testing.T) {
	use(t, "en")
	if CurrentLocale() != "en" {
		t.Fatal("not en")
	}
	SetLocale("es")
	if CurrentLocale() != "es" || T("date.today") != "Hoy" {
		t.Errorf("es: %q %q", CurrentLocale(), T("date.today"))
	}
	SetLocale("PT-br")
	if CurrentLocale() != "pt-BR" {
		t.Errorf("case-insensitive set gave %q", CurrentLocale())
	}
	SetLocale("xx")
	if CurrentLocale() != "en" {
		t.Errorf("unknown locale gave %q", CurrentLocale())
	}
	SetLocale("ja")
	if got := T("date.today"); got != "今日" {
		t.Errorf("ja today = %q", got)
	}
}

func TestFallsBackToEnglish(t *testing.T) {
	use(t, "de")
	// A key only English has resolves through the fallback.
	delete(catalog("de"), "date.today")
	defer func() { catalog("de")["date.today"] = "Heute" }()
	if got := T("date.today"); got != "Today" {
		t.Errorf("fallback = %q", got)
	}
}

func TestPluralCategories(t *testing.T) {
	cases := []struct {
		loc string
		n   uint64
		cat string
	}{
		{"en", 0, "other"}, {"en", 1, "one"}, {"en", 2, "other"}, {"en", 1_000_000, "other"},
		{"es", 0, "other"}, {"es", 1, "one"}, {"es", 21, "other"}, {"es", 2_000_000, "many"},
		{"pt-BR", 0, "one"}, {"pt-BR", 1, "one"}, {"pt-BR", 2, "other"}, {"pt-BR", 1_000_000, "many"}, {"pt-PT", 0, "other"},
		{"fr", 0, "one"}, {"fr", 1, "one"}, {"fr", 2, "other"}, {"fr", 1_000_000, "many"}, {"fr-CA", 0, "one"},
		{"de", 0, "other"}, {"de", 1, "one"}, {"de", 1_000_000, "other"},
		{"it", 0, "other"}, {"it", 1, "one"}, {"it", 1_000_000, "many"},
		{"ru", 0, "many"}, {"ru", 1, "one"}, {"ru", 2, "few"}, {"ru", 4, "few"}, {"ru", 5, "many"},
		{"ru", 11, "many"}, {"ru", 12, "many"}, {"ru", 14, "many"}, {"ru", 21, "one"}, {"ru", 22, "few"},
		{"ru", 25, "many"}, {"ru", 101, "one"}, {"ru", 111, "many"}, {"ru", 112, "many"}, {"ru", 1_000_000, "many"},
	}
	for _, c := range cases {
		if got := pluralCategory(c.loc, c.n); got != c.cat {
			t.Errorf("%s %d = %s, want %s", c.loc, c.n, got, c.cat)
		}
	}
	for _, loc := range []string{"zh-CN", "ja", "ko"} {
		for _, n := range []uint64{0, 1, 2, 1_000_000} {
			if got := pluralCategory(loc, n); got != "other" {
				t.Errorf("%s %d = %s", loc, n, got)
			}
		}
	}
}

func TestCategoriesCoverEveryCount(t *testing.T) {
	for _, loc := range []string{"en", "es", "pt-BR", "pt-PT", "fr", "it", "de", "ru", "ko", "ja", "zh-CN", "ar"} {
		cats := PluralCategories(loc)
		counts := []uint64{1_000_000, 2_000_000, 21_000_000}
		for n := uint64(0); n < 2000; n++ {
			counts = append(counts, n)
		}
		for _, n := range counts {
			got := pluralCategory(loc, n)
			ok := false
			for _, c := range cats {
				ok = ok || c == got
			}
			if !ok {
				t.Fatalf("%s %d: %s not in %v", loc, n, got, cats)
			}
		}
	}
}

func TestTPUsesCatalogs(t *testing.T) {
	// app.*.dictations is a plural in every locale; find it from the catalog.
	var key string
	for k := range Messages("en") {
		if strings.HasSuffix(k, ".dictations.one") {
			key = strings.TrimSuffix(k, ".one")
		}
	}
	if key == "" {
		t.Skip("no dictations plural")
	}
	vars := func(n int) map[string]any { return map[string]any{"number": n} }
	use(t, "en")
	if got := TP(key, 1, vars(1)); got != "1 dictation" {
		t.Errorf("en 1 = %q", got)
	}
	if got := TP(key, 3, vars(3)); got != "3 dictations" {
		t.Errorf("en 3 = %q", got)
	}
	SetLocale("ru")
	for n, want := range map[int]string{1: "1 диктовка", 2: "2 диктовки", 5: "5 диктовок", 21: "21 диктовка"} {
		if got := TP(key, n, vars(n)); got != want {
			t.Errorf("ru %d = %q, want %q", n, got, want)
		}
	}
	SetLocale("ja")
	if a, b := TP(key, 1, vars(1)), TP(key, 2, vars(2)); strings.Contains(a, "2") || !strings.Contains(b, "2") {
		t.Errorf("ja: %q %q", a, b)
	}
	// count is a built-in argument, and a missing key gives the key.
	SetLocale("en")
	if got := TP("nope", 2, nil); got != "nope" {
		t.Errorf("missing = %q", got)
	}
}

func TestTPFallsBackToOther(t *testing.T) {
	use(t, "en")
	catalog("en")["x.test.other"] = "%{count} things"
	defer delete(catalog("en"), "x.test.other")
	if got := TP("x.test", 1, nil); got != "1 things" {
		t.Errorf("fallback to other = %q", got)
	}
}

func TestOSSuffixes(t *testing.T) {
	use(t, "en")
	for goos, want := range map[string]string{"darwin": "Mac", "windows": "PC", "linux": "computer", "freebsd": "computer"} {
		if got := tos(goos, "os.computer", nil); got != want {
			t.Errorf("%s = %q, want %q", goos, got, want)
		}
	}
	if got := tos("darwin", "os.secret_store", nil); got != "the Keychain" {
		t.Errorf("secret_store = %q", got)
	}
	// accessibility has _mac and _other only: Windows falls to _other.
	if got := tos("windows", "os.permission.accessibility", nil); got != "Paste access" {
		t.Errorf("accessibility windows = %q", got)
	}
	if got := tos("darwin", "os.permission.accessibility", nil); got != "Accessibility" {
		t.Errorf("accessibility mac = %q", got)
	}
	// A key with no suffix versions resolves to itself.
	if got := tos("linux", "os.permission.microphone", nil); got != "Microphone" {
		t.Errorf("plain key = %q", got)
	}
	if got := tos("linux", "os.absent", nil); got != "os.absent" {
		t.Errorf("absent = %q", got)
	}
	if got := TOS("os.computer", nil); got == "os.computer" {
		t.Error("TOS found nothing on this OS")
	}
}

func TestPseudoLocale(t *testing.T) {
	cases := map[string]string{"Save": "[Šåvé~~]", "Hi %{name}": "[Hî %{name}~]", "%{x": "[%{x~]"}
	for in, want := range cases {
		if got := pseudoTransform(in); got != want {
			t.Errorf("pseudo(%q) = %q, want %q", in, got, want)
		}
	}
	use(t, Pseudo)
	if CurrentLocale() != Pseudo {
		t.Fatalf("locale = %q", CurrentLocale())
	}
	if got := T("date.today"); got != "[Tödåý~~]" {
		t.Errorf("pseudo today = %q", got)
	}
	got := T("date.pattern.day_month", "month", "May", "day", 3)
	if !strings.HasPrefix(got, "[") || !strings.HasSuffix(got, "]") || !strings.Contains(got, "May 3") {
		t.Errorf("pseudo pattern = %q", got)
	}
	// Roughly 30% padding on a long text.
	long := "Dictate anywhere with your voice"
	if p := pseudoTransform(long); len(p) < len(long)*13/10 {
		t.Errorf("pseudo too short: %q", p)
	}
}

func TestSettings(t *testing.T) {
	for _, v := range []string{System, Pseudo, "en", "PT-br", "zh-CN"} {
		if !IsValidSetting(v) {
			t.Errorf("%q should be valid", v)
		}
	}
	if IsValidSetting("xx") {
		t.Error("xx should be invalid")
	}
	if len(Available()) != 11 {
		t.Errorf("available = %d", len(Available()))
	}
	use(t, "en")
	if LanguageName("es") != "Spanish" || LanguageName("zz") != "zz" {
		t.Errorf("language names: %q %q", LanguageName("es"), LanguageName("zz"))
	}
}

var allCodes = []string{"en", "es", "pt-BR", "zh-CN", "ja", "de", "fr", "it", "ko", "ru", "ar"}

func r(setting string, system ...string) string { return resolveIn(setting, system, allCodes) }

func TestResolve(t *testing.T) {
	eq := func(got, want string) {
		t.Helper()
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	}
	eq(r("ja", "es-ES"), "ja")
	eq(r("pt-br", "en"), "pt-BR")
	eq(r("system", "nl-NL", "es-MX", "en-US"), "es")
	eq(r("system", "en-GB"), "en")
	eq(r("system", "ja-JP"), "ja")
	eq(r("xx", "ja-JP"), "ja")
	eq(r("system", "de-AT"), "de")
	eq(r("system", "de_CH.UTF-8"), "de")
	eq(r("system", "fr-CA"), "fr")
	eq(r("system", "it-CH"), "it")
	eq(r("system", "ko-KR"), "ko")
	eq(r("system", "ru-UA"), "ru")
	eq(r("system", "ar-SA"), "ar")
	eq(r("system", "ar-EG"), "ar")
	eq(r("system", "ar_AE.UTF-8"), "ar")
	eq(r("system", "ar"), "ar")
	eq(r("AR", "en"), "ar")
	eq(r("system", "pt-PT"), "pt-BR")
	eq(r("system", "pt_BR.UTF-8"), "pt-BR")
	eq(r("system", "zh-Hans-CN"), "zh-CN")
	eq(r("system", "zh-Hans-SG"), "zh-CN")
	eq(r("system", "zh_CN.UTF-8"), "zh-CN")
	eq(r("system", "zh"), "zh-CN")
	// Traditional Chinese never maps to zh-CN.
	eq(r("system", "zh-Hant-TW"), "en")
	eq(r("system", "zh-TW"), "en")
	eq(r("system", "zh-HK", "ja"), "ja")
	eq(r("system", "zh-MO"), "en")
	eq(r("system", "zh-Hant-CN"), "en")
	eq(r("system"), "en")
	eq(r("system", "C", "POSIX", "nl"), "en")
	eq(r(Pseudo, "ja"), Pseudo)
	eq(Resolve("de", nil), "de")
}

func TestNumbers(t *testing.T) {
	// Arabic uses Western digits with the English separators.
	for _, loc := range []string{"en", "ja", "zh-CN", "ko", "ar", "ar-EG"} {
		for n, want := range map[uint64]string{999: "999", 1234: "1,234", 1_234_567: "1,234,567"} {
			if got := IntegerIn(loc, n); got != want {
				t.Errorf("%s %d = %q", loc, n, got)
			}
		}
		if got := DecimalIn(loc, 1.25, 1); got != "1.2" {
			t.Errorf("%s 1.25 = %q", loc, got)
		}
	}
	checks := [][3]string{
		{IntegerIn("de", 1234), "1.234", "de"},
		{IntegerIn("it", 1_234_567), "1.234.567", "it"},
		{IntegerIn("pt-BR", 1234), "1.234", "pt-BR"},
		{DecimalIn("de", 1.5, 1), "1,5", "de dec"},
		{DecimalIn("pt-BR", 12345.0, 1), "12.345,0", "pt dec"},
		{IntegerIn("es", 1234), "1234", "es 4"},
		{IntegerIn("es", 12345), "12.345", "es 5"},
		{DecimalIn("es", 1.5, 1), "1,5", "es dec"},
		{IntegerIn("fr", 1234), "1 234", "fr"},
		{IntegerIn("ru", 1_234_567), "1 234 567", "ru"},
		{DecimalIn("fr", 94.12, 1), "94,1", "fr dec"},
		{DecimalIn("de", -1234.5, 1), "-1.234,5", "neg"},
		{DecimalIn("de", 45.4, 0), "45", "no places"},
	}
	for _, c := range checks {
		if c[0] != c[1] {
			t.Errorf("%s: got %q, want %q", c[2], c[0], c[1])
		}
	}
	use(t, "de")
	if Integer(1234) != "1.234" || Decimal(1.5, 1) != "1,5" {
		t.Error("current-locale helpers ignore the locale")
	}
}

func TestDates(t *testing.T) {
	d := time.Date(2026, time.September, 28, 12, 0, 0, 0, time.UTC)
	use(t, "en")
	for got, want := range map[string]string{
		DayMonthShort(d): "Sep 28", DayMonth(d): "September 28",
		WeekdayDayMonth(d): "Monday, September 28", DayMonthYear(d): "September 28, 2026",
		Today(): "Today", Yesterday(): "Yesterday",
	} {
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	}
	SetLocale("es")
	if got := DayMonth(d); !strings.Contains(got, "septiembre") || !strings.Contains(got, "28") {
		t.Errorf("es = %q", got)
	}
	SetLocale("ja")
	if got := DayMonth(d); !strings.Contains(got, "9月") {
		t.Errorf("ja = %q", got)
	}
	// Every month and weekday has a text in every locale.
	for _, l := range Locales {
		SetLocale(l.Code)
		for m := time.January; m <= time.December; m++ {
			if strings.HasPrefix(Month(m), "date.") || strings.HasPrefix(MonthShort(m), "date.") {
				t.Errorf("%s month %d missing", l.Code, m)
			}
		}
		for w := time.Sunday; w <= time.Saturday; w++ {
			if strings.HasPrefix(Weekday(w), "date.") {
				t.Errorf("%s weekday %d missing", l.Code, w)
			}
		}
	}
}

func TestSystemLocalesEnv(t *testing.T) {
	t.Setenv("LANGUAGE", "fr:de")
	t.Setenv("LC_ALL", "")
	t.Setenv("LC_MESSAGES", "")
	t.Setenv("LANG", "es_ES.UTF-8")
	got := envLocales()
	if strings.Join(got, ",") != "fr,de,es_ES.UTF-8" {
		t.Errorf("env locales = %v", got)
	}
	_ = SystemLocales() // must not panic on any OS
}

func TestArabicPluralCategories(t *testing.T) {
	for n, want := range map[uint64]string{
		0: "zero", 1: "one", 2: "two", 3: "few", 10: "few", 11: "many", 99: "many",
		100: "other", 101: "other", 102: "other", 103: "few", 111: "many", 200: "other", 1_000_000: "other",
	} {
		if got := pluralCategory("ar", n); got != want {
			t.Errorf("ar %d = %s, want %s", n, got, want)
		}
		if got := pluralCategory("ar-EG", n); got != want {
			t.Errorf("ar-EG %d = %s, want %s", n, got, want)
		}
	}
}

func TestArabicTP(t *testing.T) {
	use(t, "ar")
	for n, want := range map[int]string{
		0: "0 كلمة", 1: "كلمة واحدة", 2: "كلمتان", 3: "3 كلمات", 11: "11 كلمةً", 100: "100 كلمة",
	} {
		if got := TP("home.checklist.words", n, nil); got != want {
			t.Errorf("ar %d = %q, want %q", n, got, want)
		}
	}
}

// A key with only English one and other still resolves in Arabic for every
// category, through other.
func TestArabicTPFallsBackToOther(t *testing.T) {
	use(t, "ar")
	catalog("en")["x.test.one"] = "%{count} thing"
	catalog("en")["x.test.other"] = "%{count} things"
	defer delete(catalog("en"), "x.test.one")
	defer delete(catalog("en"), "x.test.other")
	for _, n := range []int{0, 2, 3, 11, 100} {
		if got, want := TP("x.test", n, nil), fmt.Sprintf("%d things", n); got != want {
			t.Errorf("ar %d = %q, want %q", n, got, want)
		}
	}
	catalog("ar")["x.test.other"] = "%{count} أشياء"
	defer delete(catalog("ar"), "x.test.other")
	if got := TP("x.test", 3, nil); got != "3 أشياء" {
		t.Errorf("ar other = %q", got)
	}
}

func TestArabicDates(t *testing.T) {
	d := time.Date(2026, time.September, 28, 12, 0, 0, 0, time.UTC)
	use(t, "ar")
	for got, want := range map[string]string{
		DayMonth(d): "28 سبتمبر", WeekdayDayMonth(d): "الاثنين، 28 سبتمبر",
		DayMonthYear(d): "28 سبتمبر 2026", Today(): "اليوم",
	} {
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	}
}

func TestArabicIsAvailable(t *testing.T) {
	l, ok := Find("ar")
	if !ok || l.Name != "العربية" || l.Draft {
		t.Fatalf("ar = %+v, %v", l, ok)
	}
	if !IsValidSetting("ar") || !IsValidSetting("AR") {
		t.Error("ar should be a valid setting")
	}
	if got := Resolve("system", []string{"ar-SA", "en-US"}); got != "ar" {
		t.Errorf("ar-SA resolves to %q", got)
	}
}

// Every locale has the arguments of the English text. A plural form may
// leave out an argument (Arabic writes "كلمتان", two words, without the
// number), but it must not add one.
func TestPlaceholdersMatchEnglish(t *testing.T) {
	en := Messages("en")
	args := func(s string) map[string]bool {
		out := map[string]bool{}
		for {
			i := strings.Index(s, "%{")
			if i < 0 {
				return out
			}
			j := strings.IndexByte(s[i:], '}')
			if j < 0 {
				return out
			}
			out[s[i+2:i+j]] = true
			s = s[i+j+1:]
		}
	}
	for _, l := range Locales {
		for k, v := range Messages(l.Code) {
			got := args(v)
			if isPluralKey(k) {
				base := k[:strings.LastIndexByte(k, '.')]
				want := args(en[base+".other"])
				want["count"] = true
				for a := range got {
					if !want[a] {
						t.Errorf("%s %s: %%{%s} is not in the English text", l.Code, k, a)
					}
				}
				continue
			}
			want, ok := en[k]
			if !ok {
				t.Errorf("%s %s: English has no such key", l.Code, k)
				continue
			}
			if w := args(want); !sameSet(got, w) {
				t.Errorf("%s %s: arguments %v, English has %v", l.Code, k, got, w)
			}
		}
	}
}

func sameSet(a, b map[string]bool) bool {
	if len(a) != len(b) {
		return false
	}
	for k := range a {
		if !b[k] {
			return false
		}
	}
	return true
}

// A dictation language that is not an interface locale gets its name, not
// its code (the Whisper list used to end in "af", "ar", ...).
func TestLanguageNameOfDictationLanguages(t *testing.T) {
	use(t, "en")
	for code, want := range map[string]string{"af": "Afrikaans", "ar": "Arabic", "pt": "Portuguese", "zh": "Chinese", "yue": "Cantonese", "es": "Spanish", "pt-BR": "Portuguese"} {
		if got := LanguageName(code); got != want {
			t.Errorf("en %s = %q, want %q", code, got, want)
		}
	}
	SetLocale("ar")
	for code, want := range map[string]string{"af": "الأفريكانية", "ar": "العربية", "en": "الإنجليزية", "zh-CN": "الصينية المبسّطة"} {
		if got := LanguageName(code); got != want {
			t.Errorf("ar %s = %q, want %q", code, got, want)
		}
	}
	if got := LanguageName("zz"); got != "zz" {
		t.Errorf("unknown = %q", got)
	}
}
