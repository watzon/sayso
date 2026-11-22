package ui

import (
	"bytes"
	"image"
	"image/png"
	"os"
	"testing"
	"time"

	"github.com/egoist/mygo"
)

func decodeIcon(t *testing.T, b []byte) image.Image {
	t.Helper()
	img, err := png.Decode(bytes.NewReader(b))
	if err != nil {
		t.Fatal(err)
	}
	return img
}

// The menu bar icon is the Rust app's: a 36 px waveform (18 points at 2x)
// in black on clear, which a template image needs to take the menu bar's
// colors; no filled square.
func TestTrayIconIsTheTemplateWaveform(t *testing.T) {
	for _, goos := range []string{"darwin", "windows"} {
		img := decodeIcon(t, trayIconFor(goos, false))
		if b := img.Bounds(); b.Dx() != 36 || b.Dy() != 36 {
			t.Fatalf("%s: %v", goos, b)
		}
		clear, opaque := 0, 0
		for y := 0; y < 36; y++ {
			for x := 0; x < 36; x++ {
				r, g, bl, a := img.At(x, y).RGBA()
				switch {
				case a == 0:
					clear++
				case r|g|bl != 0:
					t.Fatalf("%s: a colored pixel at %d,%d", goos, x, y)
				default:
					opaque++
				}
			}
		}
		// A filled square would cover the image; the waveform is a stroke.
		if clear < 36*36/2 || opaque == 0 {
			t.Errorf("%s: %d clear and %d inked pixels", goos, clear, opaque)
		}
	}
	src, err := os.ReadFile("../../assets/app/tray.png")
	if err != nil || !bytes.Equal(src, trayIconFor("darwin", false)) {
		t.Error("tray.png is not the copy of assets/app/tray.png: run go generate")
	}
	if lin, err := os.ReadFile("../../assets/app/icon-64.png"); err != nil || !bytes.Equal(lin, trayIconFor("linux", false)) {
		t.Error("tray-linux.png is not the copy of assets/app/icon-64.png: run go generate")
	}
}

// Incognito dims the icon to 40 % (Rust dim_icon).
func TestTrayIconDimsWhileIncognito(t *testing.T) {
	on, off := decodeIcon(t, trayIconFor("darwin", true)), decodeIcon(t, trayIconFor("darwin", false))
	for y := 0; y < 36; y++ {
		for x := 0; x < 36; x++ {
			_, _, _, a := off.At(x, y).RGBA()
			_, _, _, d := on.At(x, y).RGBA()
			if want := a * 4 / 10; d > want+0x101 || d+0x202 < want {
				t.Fatalf("at %d,%d alpha %d dims to %d", x, y, a, d)
			}
		}
	}
}

type trayShell struct {
	*fakeShell
	incognito []bool
}

func (s *trayShell) SetTrayIncognito(on bool) { s.incognito = append(s.incognito, on) }

func TestIncognitoDimsTheTray(t *testing.T) {
	e := newEnv(t)
	shell := &trayShell{fakeShell: e.shell}
	e.a.d.Shell = shell
	e.a.SetIncognito(true)
	e.a.SetIncognito(false)
	if len(shell.incognito) != 2 || !shell.incognito[0] || shell.incognito[1] {
		t.Errorf("tray told %v", shell.incognito)
	}
}

// A click on the tray icon opens the popover and a second one closes it;
// the click that arrives just after a press closed the popover leaves it
// closed instead of opening it again.
func TestPopoverToggle(t *testing.T) {
	var p popoverToggle
	at := time.Unix(1000, 0)
	if got := p.click(at, false); got != popoverShow {
		t.Fatalf("first click: %v", got)
	}
	if got := p.click(at.Add(time.Second), true); got != popoverHide {
		t.Fatalf("click on an open popover: %v", got)
	}
	// A press outside, or on the icon, closed it; the icon's click comes 80 ms later.
	p.closed(at.Add(2 * time.Second))
	if got := p.click(at.Add(2*time.Second+80*time.Millisecond), false); got != popoverNothing {
		t.Errorf("the closing click opened it again: %v", got)
	}
	if got := p.click(at.Add(3*time.Second), false); got != popoverShow {
		t.Errorf("a later click: %v", got)
	}
}

// Closing the popover gives the keyboard back to the app the user came
// from, unless the hub takes it (Open Sayso) or the press that dismissed
// the popover landed in a window of Sayso the user works in. Which window
// AppKit makes key afterwards plays no part (Rust popover.rs hide).
func TestPopoverGivesKeyboardBack(t *testing.T) {
	for _, c := range []struct {
		exit         popoverExit
		pressInSayso bool
		want         bool
	}{
		{exitGiveBack, false, true},
		{exitGiveBack, true, true}, // Esc or the icon, even with the pointer over the hub
		{exitKeep, false, false},
		{exitDismissed, false, true}, // a press in another app or on the desktop
		{exitDismissed, true, false}, // a press in the hub
	} {
		if got := popoverGivesBack(c.exit, c.pressInSayso); got != c.want {
			t.Errorf("popoverGivesBack(%v, %v) = %v, want %v", c.exit, c.pressInSayso, got, c.want)
		}
	}
}

// A press over the hub or a secondary window counts as inside Sayso; the
// popover and the overlay are not in the list Desktop passes.
func TestPointInWindows(t *testing.T) {
	boxes := []mygo.Rectangle{{X: 100, Y: 100, Width: 800, Height: 600}}
	for _, c := range []struct {
		p    mygo.Point
		want bool
	}{
		{mygo.Point{X: 100, Y: 100}, true},
		{mygo.Point{X: 899, Y: 699}, true},
		{mygo.Point{X: 900, Y: 300}, false},
		{mygo.Point{X: 50, Y: 300}, false},
	} {
		if got := pointInWindows(c.p, boxes); got != c.want {
			t.Errorf("pointInWindows(%v) = %v, want %v", c.p, got, c.want)
		}
	}
	if pointInWindows(mygo.Point{X: 1, Y: 1}, nil) {
		t.Error("no windows: inside")
	}
}
