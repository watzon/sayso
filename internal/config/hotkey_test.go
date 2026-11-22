package config

import (
	"errors"
	"strings"
	"testing"
	"time"
)

func TestParseAndFormatChords(t *testing.T) {
	cases := map[string]string{
		"opt+space":      "ctrl+%o+space",
		"ctrl+cmd+v":     "ctrl+%c+v",
		"Shift+F12":      "shift+f12",
		"alt+`":          "%o+`",
		"win+ctrl+/":     "ctrl+%c+/",
		"fn+ctrl+return": "fn+ctrl+return",
		"super+enter":    "%c+return",
		"meta+escape":    "%c+esc",
		"ctrl+delete":    "ctrl+backspace",
		"ctrl+5":         "ctrl+5",
	}
	for in, want := range cases {
		h, err := ParseHotkey(in)
		if err != nil {
			t.Fatalf("%s: %v", in, err)
		}
		for goos, names := range map[string][2]string{"darwin": {"opt", "cmd"}, "windows": {"alt", "win"}, "linux": {"alt", "super"}} {
			w := strings.NewReplacer("%o", names[0], "%c", names[1]).Replace(want)
			if in == "opt+space" {
				w = names[0] + "+space"
			}
			if got := h.formatFor(goos); got != w {
				t.Errorf("%s on %s: got %s want %s", in, goos, got, w)
			}
		}
	}
}

func TestParseSoloAndModifierSets(t *testing.T) {
	h, _ := ParseHotkey(" Right_Option ")
	if h != SoloKey(RightOption) {
		t.Fatal(h)
	}
	set, _ := ParseHotkey("left_option+left_command")
	if set.Kind != HotkeyModifierOnly || set.String() != "left_command+left_option" {
		t.Fatal(set.String())
	}
	if _, err := ParseHotkey("left_command+left_command"); !errors.Is(err, ErrWrongKeyCount) {
		t.Fatal(err)
	}
	if _, err := ParseHotkey(""); !errors.Is(err, ErrHotkeyEmpty) {
		t.Fatal(err)
	}
	if _, err := ParseHotkey("ctrl+a+b"); !errors.Is(err, ErrWrongKeyCount) {
		t.Fatal(err)
	}
	if _, err := ParseHotkey("ctrl+shift"); !errors.Is(err, ErrWrongKeyCount) {
		t.Fatal(err)
	}
	var uk *UnknownKeyError
	if _, err := ParseHotkey("ctrl+banana"); !errors.As(err, &uk) || uk.Name != "banana" {
		t.Fatal(err)
	}
	if _, err := ParseHotkey("ctrl+f21"); err == nil {
		t.Fatal("f21 accepted")
	}
}

func TestOverlaps(t *testing.T) {
	a := MustHotkey("left_control")
	b := MustHotkey("left_control+left_option")
	c := MustHotkey("ctrl+opt+d")
	if !a.Overlaps(b) || !b.Overlaps(a) || a.Overlaps(c) || !c.Overlaps(c) {
		t.Fatal("overlaps")
	}
	if MustHotkey("right_option").Overlaps(MustHotkey("left_option")) {
		t.Fatal("sides differ")
	}
}

func TestKeycaps(t *testing.T) {
	h := MustHotkey("ctrl+opt+shift+cmd+v")
	if got := strings.Join(h.keycapsFor("darwin"), ""); got != "⌃⌥⇧⌘V" {
		t.Fatal(got)
	}
	if got := strings.Join(h.keycapsFor("windows"), "+"); got != "Win+Ctrl+Alt+Shift+V" {
		t.Fatal(got)
	}
	if got := strings.Join(MustHotkey("left_command+right_shift").keycapsFor("linux"), ","); got != "Left Super,Right Shift" {
		t.Fatal(got)
	}
	if got := strings.Join(MustHotkey("fn+return").keycapsFor("darwin"), ","); got != "fn,↵" {
		t.Fatal(got)
	}
}

func TestDefaultsPerSystem(t *testing.T) {
	if toggleDefaultFor("darwin").formatFor("darwin") != "opt+space" ||
		toggleDefaultFor("windows").formatFor("windows") != "ctrl+space" ||
		toggleDefaultFor("linux").formatFor("linux") != "ctrl+alt+space" {
		t.Fatal("toggle defaults")
	}
	if pasteLastDefaultFor("darwin").formatFor("darwin") != "ctrl+cmd+v" ||
		pasteLastDefaultFor("windows").formatFor("windows") != "alt+shift+v" ||
		pasteLastDefaultFor("linux").formatFor("linux") != "ctrl+alt+v" {
		t.Fatal("paste defaults")
	}
}

func TestEveryPunctuationKeyRoundTrips(t *testing.T) {
	for _, p := range Punctuation {
		h, err := ParseHotkey("ctrl+" + string(p))
		if err != nil || h.Key.Kind != KeyPunctuation {
			t.Fatalf("%c: %v", p, err)
		}
		back, _ := ParseHotkey(h.String())
		if back != h {
			t.Fatalf("%c", p)
		}
	}
}

func TestDeliberateTapsHaveACeiling(t *testing.T) {
	now := time.Now()
	one := SetOf(LeftControl)
	two := SetOf(LeftControl, LeftShift)
	for _, c := range []struct {
		delay int
		want  bool
	}{{400, true}, {401, false}} {
		var cap ModifierCapture
		cap.PressTap(one, now)
		cap.PressTap(two, now.Add(350*time.Millisecond))
		_, ok := cap.ReleaseTap(LeftShift, now.Add(time.Duration(c.delay)*time.Millisecond))
		if ok != c.want || cap.IsActive() {
			t.Fatal(c.delay, ok)
		}
	}
	var cap ModifierCapture
	cap.PressTap(one, now)
	cap.Clear()
	cap.PressTap(one, now.Add(time.Second))
	if _, ok := cap.ReleaseTap(LeftControl, now.Add(1100*time.Millisecond)); !ok {
		t.Fatal("clearing resets the clock")
	}
}
