package platform

import "testing"

// Sayso's overlay and tray popover are tool windows that do not activate;
// its hub is a normal window.
func TestOwnFloating(t *testing.T) {
	if !ownFloating(0, overlayTitle) {
		t.Error("the overlay counted as focus")
	}
	if !ownFloating(wsExToolWindow, "") || !ownFloating(wsExNoActivate, "") {
		t.Error("the popover counted as focus")
	}
	if ownFloating(0, "Sayso") {
		t.Error("the hub did not count as focus")
	}
}
