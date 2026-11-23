package ui

import (
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
)

func inside(r, outer mu.Rect) bool {
	const e = 0.5
	return r.X >= outer.X-e && r.Y >= outer.Y-e && r.X+r.W <= outer.X+outer.W+e && r.Y+r.H <= outer.Y+outer.H+e
}

func overlaps(r, outer mu.Rect) bool {
	return r.X < outer.X+outer.W && outer.X < r.X+r.W && r.Y < outer.Y+outer.H && outer.Y < r.Y+r.H
}

// openSelectFits opens the select named trigger and checks that each
// option shows inside the popup and the window, and that End brings the
// last option into view. It returns the number of options.
func openSelectFits(t *testing.T, tt *mu.Tester, trigger string) int {
	t.Helper()
	press(t, tt, trigger)
	tt.Frame()
	panel, ok := tt.AccessByRole(mu.RoleListBox, "")
	if !ok {
		for _, a := range tt.AccessAll() {
			if a.Role == mu.RoleListBox {
				panel, ok = a, true
			}
		}
	}
	if !ok {
		t.Fatalf("%s: no popup", trigger)
	}
	window := mu.Rect{W: HubWidth, H: HubHeight}
	if !inside(panel.Bounds, window) {
		t.Errorf("%s: the popup %+v is outside the window", trigger, panel.Bounds)
	}
	var want string
	check := func(when string) (n int, last mu.Rect) {
		for _, a := range tt.AccessAll() {
			if a.Role != mu.RoleOption {
				continue
			}
			n++
			if a.Name == want {
				last = a.Bounds
			}
			// A scroll container marks what it built out of view; what
			// shows must be in the popup (its edge clips a line partly
			// out). Before the list scrolled, lines drew past the popup.
			if a.Offscreen {
				continue
			}
			if !overlaps(a.Bounds, panel.Bounds) || !inside(a.Bounds, window) {
				t.Errorf("%s, %s: option %q at %+v shows outside the popup %+v", trigger, when, a.Name, a.Bounds, panel.Bounds)
			}
		}
		return n, last
	}
	for _, a := range tt.AccessAll() {
		if a.Role == mu.RoleOption {
			want = a.Name
		}
	}
	n, _ := check("opened")
	tt.Key(0, mu.KeyEnd)
	tt.Frame()
	if _, last := check("after End"); !inside(last, panel.Bounds) || last.H < 20 {
		t.Errorf("%s: End left the last option %q out of view (%+v)", trigger, want, last)
	}
	return n
}

// A long select (Whisper's dictation languages) scrolls inside its popup
// instead of drawing its options down the page; End reaches the last one.
func TestSelectLongListScrollsInsidePopup(t *testing.T) {
	e := newEnv(t)
	e.a.cfg.Dictation.Model = "whisper-large-v3-turbo"
	e.a.OpenSettings(SecDictation)
	tt := e.hub(t)
	tt.Frame()
	if n := openSelectFits(t, tt, i18n.T("settings.dictation.dictation_language")); n < 25 {
		t.Errorf("%d options, want a long list", n)
	}
}

// The interface language select (12 options), Chris's screenshot.
func TestSelectInterfaceLanguageFits(t *testing.T) {
	e := newEnv(t)
	e.a.OpenSettings(SecGeneral)
	tt := e.hub(t)
	tt.Frame()
	if n := openSelectFits(t, tt, i18n.T("settings.general.interface_language")); n < 10 {
		t.Errorf("%d options", n)
	}
}

// languageOption finds the open option of the interface language select
// named name, alone or followed by its native name, and the texts inside it.
func languageOption(tt *mu.Tester, name string) (opt mu.AccessInfo, texts []mu.AccessInfo, ok bool) {
	all := tt.AccessAll()
	for _, a := range all {
		if a.Role == mu.RoleOption && (a.Name == name || strings.HasPrefix(a.Name, name+" ")) {
			opt, ok = a, true
			break
		}
	}
	if !ok {
		return opt, nil, false
	}
	for _, a := range all {
		if a.Role == mu.RoleText && inside(a.Bounds, opt.Bounds) {
			texts = append(texts, a)
		}
	}
	return opt, texts, true
}

// Each interface language shows its name in the interface language and,
// in gray at the trailing edge, its own name; the interface language and
// System show one name. Chris's request, 2026-11-22.
func TestInterfaceLanguageShowsNativeNames(t *testing.T) {
	for _, tc := range []struct {
		locale   string
		rtl      bool
		pairs    [][2]string // name, native, in menu order
		single   []string
		pick     string
		pickCode string
	}{
		{"en", false, [][2]string{{"Spanish", "Español"}, {"Japanese", "日本語"}, {"French", "Français"}, {"Korean", "한국어"}, {"Arabic", "العربية"}},
			[]string{"System", "English"}, "German", "de"},
		{"ar", true, [][2]string{{"الإنجليزية", "English"}, {"اليابانية", "日本語"}, {"الفرنسية", "Français"}, {"الكورية", "한국어"}},
			[]string{"لغة النظام", "العربية"}, "الإنجليزية", "en"},
	} {
		t.Run(tc.locale, func(t *testing.T) {
			e := newEnv(t)
			i18n.SetLocale(tc.locale)
			e.a.cfg.General.Language = tc.locale
			e.a.OpenSettings(SecGeneral)
			tt := e.hub(t)
			tt.Frame()
			trigger := i18n.T("settings.general.interface_language")
			if n := openSelectFits(t, tt, trigger); n < 12 {
				t.Fatalf("%d options", n)
			}
			// The trigger shows the name alone.
			if b, ok := controlBounds(tt, trigger); !ok || b.W > 160 {
				t.Errorf("the trigger is %+v", b)
			}
			for _, pair := range tc.pairs {
				name, native := pair[0], pair[1]
				tt.Key(0, mu.KeyHome)
				tt.Frame()
				o, texts, ok := languageOption(tt, name)
				for ok && o.Offscreen {
					tt.Key(0, mu.KeyDown)
					tt.Frame()
					o, texts, ok = languageOption(tt, name)
				}
				if !ok {
					t.Errorf("no option %q", name)
					continue
				}
				if len(texts) != 2 || texts[0].Name != name || texts[1].Name != native {
					var got []string
					for _, a := range texts {
						got = append(got, a.Name)
					}
					t.Errorf("row %q shows %q, want %q then %q", name, got, name, native)
					continue
				}
				nb, db := texts[0].Bounds, texts[1].Bounds
				// The native name sits at the trailing edge, clear of the name.
				gap, edge := db.X-(nb.X+nb.W), (o.Bounds.X+o.Bounds.W)-(db.X+db.W)
				if tc.rtl {
					gap, edge = nb.X-(db.X+db.W), db.X-o.Bounds.X
				}
				if gap < 16 {
					t.Errorf("%q and %q are %.1f px apart", name, native, gap)
				}
				// The list scrolls, so the end keeps room for its bar.
				if edge < 0 || edge > barGutter+0.5 {
					t.Errorf("%q is %.1f px from the trailing edge of %+v", native, edge, o.Bounds)
				}
				if !inside(nb, o.Bounds) || !inside(db, o.Bounds) {
					t.Errorf("%q overflows its row %+v: %+v %+v", name, o.Bounds, nb, db)
				}
			}
			for _, name := range tc.single {
				tt.Key(0, mu.KeyHome)
				tt.Frame()
				if o, texts, ok := languageOption(tt, name); !ok || o.Name != name || len(texts) != 1 {
					t.Errorf("row %q: %q with %d texts, want the name alone", name, o.Name, len(texts))
				}
			}
			tt.Key(0, mu.KeyHome)
			tt.Frame()
			o, _, ok := languageOption(tt, tc.pick)
			if !ok {
				t.Fatalf("no option %q", tc.pick)
			}
			tt.ClickAt(o.Bounds.X+o.Bounds.W/2, o.Bounds.Y+o.Bounds.H/2)
			tt.Frame()
			if got := e.a.cfg.General.Language; got != tc.pickCode {
				t.Errorf("picking %q set %q, want %q", tc.pick, got, tc.pickCode)
			}
		})
	}
}
