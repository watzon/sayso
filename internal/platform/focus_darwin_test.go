package platform

import (
	"testing"
	"time"
)

// The window list decodes: each entry has an owner, some are normal
// windows (layer 0; desktop widgets sit far below it), and they have
// bounds. Reads only.
func TestMacWindowListDecodes(t *testing.T) {
	if err := bindFocus(); err != nil {
		t.Fatal(err)
	}
	list := cgWindows()
	if len(list) == 0 {
		t.Skip("no window server session")
	}
	normal := 0
	for _, w := range list {
		if w.pid <= 0 {
			t.Fatalf("window without an owner: %+v", w)
		}
		if w.layer == 0 && !w.bounds.Empty() {
			normal++
		}
	}
	if normal == 0 {
		t.Fatalf("no normal window with bounds: %+v", list)
	}
}

// Focused answers without blocking long, and Watch starts and stops
// cleanly, twice, releasing its observers.
func TestMacFocusWatchStartsAndStops(t *testing.T) {
	f := newMacFocus()
	done := make(chan struct{})
	go func() {
		defer close(done)
		if fr, ok := f.Focused(); ok && fr.Rect.Empty() {
			t.Errorf("focus ok with an empty frame: %+v", fr)
		}
	}()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("Focused blocked")
	}
	for range 2 {
		stop, err := f.Watch(func() {})
		if err != nil {
			t.Fatal(err)
		}
		stopped := make(chan struct{})
		go func() { stop(); stop(); close(stopped) }()
		select {
		case <-stopped:
		case <-time.After(5 * time.Second):
			t.Fatal("stop blocked")
		}
	}
}
