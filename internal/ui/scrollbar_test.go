package ui

import (
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
)

// barClearance is the least room between a scroll bar's track and the
// text or the buttons of the list it scrolls.
const barClearance = 4

// barTrack is where MyGo draws a scroll container's vertical bar, over the
// container's content: a track at its end edge, the left right to left.
func barTrack(box mu.Rect, rtl bool) mu.Rect {
	w := mu.ScrollbarStyle{}.Resolve(nil).TrackWidth
	if rtl {
		return mu.Rect{X: box.X, Y: box.Y, W: w, H: box.H}
	}
	return mu.Rect{X: box.X + box.W - w, Y: box.Y, W: w, H: box.H}
}

// clearOfBar checks that each text and button shown in the scroll
// container box keeps barClearance from its bar, and that something sits
// near the bar's edge, so that the check is not empty.
func clearOfBar(t *testing.T, tt *mu.Tester, box mu.Rect, rtl bool, what string) {
	t.Helper()
	track := barTrack(box, rtl)
	nearest := float32(1e9)
	for _, a := range tt.AccessAll() {
		if a.Offscreen || (a.Role != mu.RoleText && a.Role != mu.RoleButton) {
			continue
		}
		b := a.Bounds
		if cx := b.X + b.W/2; cx < box.X || cx > box.X+box.W || b.Y+b.H <= box.Y || b.Y >= box.Y+box.H {
			continue
		}
		gap := track.X - (b.X + b.W)
		if rtl {
			gap = b.X - (track.X + track.W)
		}
		nearest = min(nearest, gap)
		if gap < barClearance-0.01 {
			t.Errorf("%s: %q at %+v is %.1f px from the scroll bar's track %+v", what, a.Name, b, gap, track)
		}
	}
	if nearest > 30 {
		t.Errorf("%s: nothing near the scroll bar (nearest %.1f px), the check found no trailing content", what, nearest)
	}
}

// scrollAround is the scroll container that holds the point (x, y).
func scrollAround(t *testing.T, tt *mu.Tester, x, y float32) mu.Rect {
	t.Helper()
	found, ok := mu.Rect{}, false
	for _, a := range tt.AccessAll() {
		b := a.Bounds
		if a.Role == mu.RoleScroll && x >= b.X && x <= b.X+b.W && y >= b.Y && y <= b.Y+b.H {
			// The innermost: the page's own scroll holds the others.
			if !ok || b.W*b.H < found.W*found.H {
				found, ok = b, true
			}
		}
	}
	if !ok {
		t.Fatalf("no scroll container at %.0f, %.0f", x, y)
	}
	return found
}

// The Dictionary's scroll bars stay off the rules' Used counts and their
// edit and remove buttons, and off the word slips (Chris, 2026-11-22: the
// rules' bar covered the counts).
func TestDictionaryScrollBarsClearTheirLists(t *testing.T) {
	for _, locale := range []string{"en", "ar"} {
		t.Run(locale, func(t *testing.T) {
			e := newEnv(t)
			i18n.SetLocale(locale)
			e.a.cfg.General.Language = locale
			fillDictionary(t, e, 40, 30)
			tt := e.hub(t)
			tt.Frame()
			tt.Frame()
			rtl := locale == "ar"

			// The pointer on a rule shows its edit and remove buttons, at
			// the end of the row with the count.
			first, ok := tt.Access("Mac OS 1")
			if !ok {
				t.Fatal("no first rule")
			}
			tt.Move(first.Bounds.X+first.Bounds.W/2, first.Bounds.Y+first.Bounds.H/2)
			tt.Frame()
			if !hasControl(tt, i18n.T("dictionary.remove_rule", "from", "Mac OS 1")) {
				t.Fatal("no remove button on the hovered rule")
			}
			rules := scrollAround(t, tt, first.Bounds.X+first.Bounds.W/2, first.Bounds.Y+first.Bounds.H/2)
			clearOfBar(t, tt, rules, rtl, "rules")

			// The rows still line up with the table's heading.
			used := textBounds(t, tt, strings.ToUpper(i18n.T("dictionary.column.used")))
			count, ok := tt.Access("0")
			if !ok {
				t.Fatal("no Used count")
			}
			usedEnd, countEnd := used.X+used.W, count.Bounds.X+count.Bounds.W
			if rtl {
				usedEnd, countEnd = used.X, count.Bounds.X
			}
			if d := usedEnd - countEnd; d > 1 || d < -1 {
				t.Errorf("the counts end at %.1f, the Used heading at %.1f", countEnd, usedEnd)
			}

			words, ok := tt.AccessByRole(mu.RoleGroup, i18n.T("dictionary.words"))
			if !ok {
				t.Fatal("no words group")
			}
			clearOfBar(t, tt, words.Bounds, rtl, "words")
		})
	}
}

// The interface language select's list scrolls, and its bar stays off the
// language names in gray at the options' end.
func TestSelectScrollBarClearsDetails(t *testing.T) {
	for _, locale := range []string{"en", "ar"} {
		t.Run(locale, func(t *testing.T) {
			e := newEnv(t)
			i18n.SetLocale(locale)
			e.a.cfg.General.Language = locale
			e.a.OpenSettings(SecGeneral)
			tt := e.hub(t)
			tt.Frame()
			press(t, tt, i18n.T("settings.general.interface_language"))
			tt.Frame()
			var panel mu.AccessInfo
			for _, a := range tt.AccessAll() {
				if a.Role == mu.RoleListBox {
					panel = a
				}
			}
			if panel.Bounds.W == 0 {
				t.Fatal("no popup")
			}
			b := panel.Bounds
			list := scrollAround(t, tt, b.X+b.W/2, b.Y+b.H/2)
			clearOfBar(t, tt, list, locale == "ar", "languages")
		})
	}
}
