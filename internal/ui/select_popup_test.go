package ui

import (
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
