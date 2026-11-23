package ui

import (
	"fmt"
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/store"
)

// entryRowName is the accessible name of e's row in the history list.
func entryRowName(a *App, e store.HistoryEntry) string {
	return i18n.T("history.a11y.entry", "app", entryApp(e), "time", a.clockOf(e.CreatedAt))
}

func appOption(name string, n int) string {
	return i18n.T("history.a11y.app_option", "app", name, "count", formatCount(n))
}

// The App filter opens a sheet floating under its chip (Rust history.rs
// app_picker): the list under the toolbar stays where it is, the field
// filters the apps, a pick filters the list, and Escape or a press outside
// closes it.
func TestHistoryAppPickerFloats(t *testing.T) {
	e := newEnv(t)
	for i := 0; i < 18; i++ {
		e.addEntry(t, fmt.Sprintf("note %d", i), fmt.Sprintf("App%02d", i), false)
	}
	e.a.SetPage(PageHistory)
	tt := e.hub(t)
	firstRow := entryRowName(e.a, e.a.hist.entries[0])
	before, ok := controlBounds(tt, firstRow)
	if !ok {
		t.Fatalf("no row %q", firstRow)
	}
	press(t, tt, i18n.T("history.filter_app"))
	if !e.a.hist.appsOpen || !hasControl(tt, appOption("App00", 1)) {
		t.Fatal("the app picker did not open")
	}
	if after, _ := controlBounds(tt, firstRow); after.Y != before.Y {
		t.Errorf("opening the picker moved the first row from y=%v to y=%v", before.Y, after.Y)
	}
	// The rows scroll inside the sheet: the last app is past its bottom.
	sheet, _ := controlBounds(tt, appOption(i18n.T("history.all_apps"), 18))
	if last, ok := controlBounds(tt, appOption("App17", 1)); ok && last.Y < sheet.Y+298 {
		t.Errorf("App17 at y=%v: the list does not scroll (All apps at y=%v)", last.Y, sheet.Y)
	}
	// Name left, after the badge: 9 px padding, an 18 px badge, a 9 px gap.
	labelStarts(t, tt, HubWidth, []string{appOption(i18n.T("history.all_apps"), 18), appOption("App00", 1), appOption("App01", 1)}, 9, 18, 9+18+9)

	tt.Type("app0")
	tt.Frame()
	if e.a.hist.appQuery != "app0" || hasControl(tt, appOption("App12", 1)) || !hasControl(tt, appOption("App03", 1)) {
		t.Fatalf("filter %q: App12 shows %v, App03 shows %v", e.a.hist.appQuery, hasControl(tt, appOption("App12", 1)), hasControl(tt, appOption("App03", 1)))
	}
	press(t, tt, appOption("App03", 1))
	if e.a.hist.appsOpen || e.a.hist.app != "com.app03" {
		t.Fatalf("picking App03: open %v, app %q", e.a.hist.appsOpen, e.a.hist.app)
	}
	tt.Frame()
	if len(e.a.hist.entries) != 1 || e.a.hist.entries[0].App.Name != "App03" {
		t.Errorf("filtered to App03: %d entries", len(e.a.hist.entries))
	}

	// Escape closes it, and so does a press outside; the field starts empty.
	press(t, tt, "App03")
	if !e.a.hist.appsOpen || e.a.hist.appQuery != "" {
		t.Fatalf("reopened: open %v, query %q", e.a.hist.appsOpen, e.a.hist.appQuery)
	}
	tt.Key(0, mu.KeyEscape)
	tt.Frame()
	if e.a.hist.appsOpen {
		t.Error("Escape left the app picker open")
	}
	press(t, tt, "App03")
	tt.ClickAt(900, 700)
	tt.Frame()
	if e.a.hist.appsOpen {
		t.Error("a press outside left the app picker open")
	}
}

// A right-click on a row opens the actions of that entry, not of the one
// selected, in the same menu as ⋯ (Copy at the top, Delete last).
func TestHistoryRowContextMenu(t *testing.T) {
	e := newEnv(t)
	e.addEntry(t, "first apple note", "Notes", false)
	e.addEntry(t, "second banana mail", "Mail", false)
	e.a.SetPage(PageHistory)
	tt := e.hub(t)
	sel, other := e.a.hist.entries[0], e.a.hist.entries[1]
	if e.a.hist.selected != sel.ID || other.FinalText != "first apple note" {
		t.Fatalf("selected %d, want %d; second row %q", e.a.hist.selected, sel.ID, other.FinalText)
	}
	rightClick := func() {
		t.Helper()
		r, ok := controlBounds(tt, entryRowName(e.a, other))
		if !ok {
			t.Fatal("no second row")
		}
		tt.RightClickAt(r.X+40, r.Y+r.H/2)
		tt.Frame()
	}
	rightClick()
	names := []string{i18n.T("history.menu.copy"), i18n.T("history.menu.copy_metadata"), i18n.T("history.menu.enhance"), i18n.T("history.delete")}
	for _, n := range names {
		if _, ok := tt.AccessByRole(mu.RoleMenuItem, n); !ok {
			t.Fatalf("the context menu has no %q", n)
		}
	}
	if e.a.hist.menuOpen {
		t.Error("the right-click opened the ⋯ menu")
	}
	labelStarts(t, tt, HubWidth, names, 10, 14, 10+14+10)
	press(t, tt, i18n.T("history.menu.copy"))
	if tt.Clipboard() != other.FinalText {
		t.Errorf("Copy wrote %q, want the right-clicked %q", tt.Clipboard(), other.FinalText)
	}
	if tt.HasText(i18n.T("history.menu.copy_metadata")) {
		t.Error("the menu is still open after Copy")
	}

	rightClick()
	tt.Key(0, mu.KeyEscape)
	tt.Frame()
	if _, ok := tt.AccessByRole(mu.RoleMenuItem, i18n.T("history.menu.copy")); ok {
		t.Error("Escape left the context menu open")
	}

	rightClick()
	press(t, tt, i18n.T("history.delete"))
	if e.a.hist.selected != other.ID || !e.a.hist.confirm || !tt.HasText(i18n.T("history.confirm_delete")) {
		t.Fatalf("Delete: selected %d (want %d), confirm %v", e.a.hist.selected, other.ID, e.a.hist.confirm)
	}
	press(t, tt, i18n.T("history.delete"))
	if _, ok, _ := e.db.Get(other.ID); ok {
		t.Error("the right-clicked entry was not deleted")
	}
	if _, ok, _ := e.db.Get(sel.ID); !ok {
		t.Error("the selected entry was deleted")
	}
}
