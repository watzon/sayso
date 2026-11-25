package ui

import (
	"fmt"
	"slices"
	"strings"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
)

func TestWhatsNewWindowHasAFixedSize(t *testing.T) {
	e := newEnv(t)
	opts, ok := (&Desktop{a: e.a}).windowOptions(WinWhatsNew)
	if !ok {
		t.Fatal("no options for What's New")
	}
	if !opts.DisableResize || !opts.DisableMaximize || !opts.DisableFullScreen {
		t.Errorf("resize disabled %v, maximize disabled %v, full screen disabled %v; want all", opts.DisableResize, opts.DisableMaximize, opts.DisableFullScreen)
	}
	if opts.Width != WhatsNewW || opts.Height != WhatsNewH {
		t.Errorf("size %dx%d, want %dx%d", opts.Width, opts.Height, WhatsNewW, WhatsNewH)
	}
}

// The whole sheet of What's New moves the window; the list under it still
// scrolls, by wheel and by finger, and its buttons still click.
func TestWhatsNewSheetKeepsItsControls(t *testing.T) {
	e := newEnv(t)
	notes := []string{"[[release]]\nversion = \"0.16.0\"\ntitle = \"Sayso on Go\""}
	for i := range 12 {
		notes = append(notes, fmt.Sprintf("[[release.item]]\ntitle = \"Change %d\"\ntext = \"What changed, told in a line or two.\"", i))
	}
	e.a.news.load(strings.Join(notes, "\n\n"), "0.16.0")
	e.shell.shown[WinWhatsNew] = true
	tt := mu.NewTester(e.a.WhatsNewView, WhatsNewW, WhatsNewH)
	tt.Frame()
	list := mustBounds(t, tt, i18n.T("whats_new.list_label"))
	cx, cy := list.X+list.W/2, list.Y+list.H/2

	top := textBounds(t, tt, "Change 0").Y
	tt.Scroll(cx, cy, 0, 200)
	tt.Frame()
	scrolled := textBounds(t, tt, "Change 0").Y
	if scrolled >= top {
		t.Fatalf("the wheel did not scroll the list: the first item at %v, then %v", top, scrolled)
	}
	tt.FingerDown(cx, cy)
	for dy := float32(20); dy <= 160; dy += 20 {
		tt.FingerMove(cx, cy+dy)
	}
	tt.FingerUp(cx, cy+160)
	tt.Frame()
	if panned := textBounds(t, tt, "Change 0").Y; panned <= scrolled {
		t.Errorf("a finger did not scroll the list back: the first item at %v, then %v", scrolled, panned)
	}

	// A double click on the bare sheet, above the eyebrow, neither closes
	// the window nor presses a control.
	tt.ClickAt(WhatsNewW/2, 8)
	tt.ClickAt(WhatsNewW/2, 8)
	tt.Frame()
	if !e.shell.shown[WinWhatsNew] || len(e.shell.opened) != 0 {
		t.Fatalf("the sheet's double click: window shown %v, opened %q", e.shell.shown[WinWhatsNew], e.shell.opened)
	}

	press(t, tt, i18n.T("whats_new.all_changes"))
	if !slices.Contains(e.shell.opened, "https://github.com/watzon/sayso/releases") {
		t.Errorf("all changes opened %q", e.shell.opened)
	}
	press(t, tt, i18n.T("whats_new.done"))
	if e.shell.shown[WinWhatsNew] {
		t.Error("done did not close the window")
	}
}
