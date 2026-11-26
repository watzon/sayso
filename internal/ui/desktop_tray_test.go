package ui

import (
	"slices"
	"testing"

	"github.com/egoist/mygo"

	"github.com/watzon/sayso/internal/i18n"
)

// fakeTray records what the tray path asks of the icon.
type fakeTray struct {
	menus        []*mygo.Menu
	popups       int
	click, right func()
}

func (t *fakeTray) OnClick(fn func()) func()      { t.click = fn; return func() {} }
func (t *fakeTray) OnRightClick(fn func()) func() { t.right = fn; return func() {} }
func (t *fakeTray) SetMenu(m *mygo.Menu)          { t.menus = append(t.menus, m) }
func (t *fakeTray) PopUpMenu()                    { t.popups++ }

func (t *fakeTray) labels() []string {
	if len(t.menus) == 0 || t.menus[len(t.menus)-1] == nil {
		return nil
	}
	var out []string
	for _, it := range t.menus[len(t.menus)-1].Items() {
		if it.Type != mygo.MenuItemSeparator {
			out = append(out, it.Label)
		}
	}
	return out
}

// On Linux the AppIndicator never sends a right click and opens the menu
// it holds on a click, so the menu must be on the icon from the start
// (issue 86: the tray showed an empty menu).
func TestLinuxTrayHasItsMenuFromTheStart(t *testing.T) {
	d, _ := fakeDesktop(t)
	tr := &fakeTray{}
	d.wireTray(tr, true)
	want := []string{i18n.T("tray.toggle_dictation"), i18n.T("tray.open_hub"), i18n.T("tray.quit")}
	if got := tr.labels(); !slices.Equal(got, want) {
		t.Fatalf("menu at start %q, want %q", got, want)
	}
	if tr.click == nil {
		t.Fatal("the icon's activation (a double click on GNOME) no longer opens the popover")
	}
}

// The attached menu follows the interface language.
func TestLinuxTrayMenuFollowsTheLanguage(t *testing.T) {
	d, _ := fakeDesktop(t)
	d.a.d.Shell = d
	tr := &fakeTray{}
	d.wireTray(tr, true)
	d.a.SetLanguage("de")
	if got, want := tr.labels(), i18n.T("tray.quit"); len(got) != 3 || got[2] != want || want == "Quit Sayso" {
		t.Fatalf("menu after the language change %q, want its last item %q", got, want)
	}
}

// On macOS and Windows a menu on the icon would take the primary click
// from the popover: the menu goes on only for the secondary click.
func TestTrayMenuOnlyForTheSecondaryClick(t *testing.T) {
	d, _ := fakeDesktop(t)
	tr := &fakeTray{}
	d.wireTray(tr, false)
	if len(tr.menus) != 0 {
		t.Fatalf("a menu was set at start: %q", tr.labels())
	}
	d.a.d.Shell = d
	d.a.SetLanguage("de")
	if len(tr.menus) != 0 {
		t.Fatalf("a language change set a menu: %q", tr.labels())
	}
	tr.right()
	if len(tr.labels()) != 3 || tr.popups != 1 {
		t.Fatalf("right click: menu %q, %d pop-ups", tr.labels(), tr.popups)
	}
}
