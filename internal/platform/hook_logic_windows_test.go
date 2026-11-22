package platform

import (
	"reflect"
	"testing"
	"time"
)

const (
	keyD = 0x44
	keyL = 0x4C
)

func kdown(vk uint16, held ...uint16) rawKey {
	return rawKey{vk: vk, down: true, held: modKeysOf(held...)}
}
func kup(vk uint16, held ...uint16) rawKey { return rawKey{vk: vk, held: modKeysOf(held...)} }

func logicWith(t *testing.T, cfg HotkeyConfig) *hookLogic {
	t.Helper()
	l := newHookLogic()
	if _, err := l.configure(cfg); err != nil {
		t.Fatal(err)
	}
	return l
}

func kinds(out hookOutcome) []HotkeyEventKind {
	var k []HotkeyEventKind
	for _, e := range out.events {
		k = append(k, e.Kind)
	}
	return k
}

func want(t *testing.T, what string, got hookOutcome, k ...HotkeyEventKind) {
	t.Helper()
	if !reflect.DeepEqual(kinds(got), k) {
		t.Fatalf("%s: events %v, want %v", what, kinds(got), k)
	}
}

func TestParsesHotkeys(t *testing.T) {
	for in, ok := range map[string]bool{
		"alt+space": true, "ctrl+win+v": true, "right_option": true, "ctrl+shift+d": true, "win+f5": true,
		"left_command+left_option": true, "right_alt": true, "ctrl+`": true, "": false, "ctrl+shift": false,
		"ctrl+a+b": false, "left_shift+left_shift": false, "ctrl+f21": false, "ctrl+nope": false,
	} {
		if _, err := parseHotkey(in); (err == nil) != ok {
			t.Errorf("%q: %v", in, err)
		}
	}
	h, _ := parseHotkey("left_command+left_option")
	if h.shape != shapeModifierOnly || !h.set.has(leftCommand) || !h.set.has(leftOption) || h.set.len() != 2 {
		t.Fatalf("%+v", h)
	}
	for vk := uint16(0); vk < 256; vk++ {
		if k, ok := keyFromVK(vk); ok {
			if back, _ := vkOf(k); back != vk {
				t.Errorf("vk %#x round trips to %#x", vk, back)
			}
		}
	}
	if sidedVK(vkShift, 0x36, false) != vkRShift || sidedVK(vkControl, 0, true) != vkRControl || sidedVK(vkMenu, 0, false) != vkLMenu {
		t.Fatal("sided keys")
	}
}

func TestATapToggleEndsWithin400msAndLongHoldsDoNotToggle(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{Toggle: "left_shift"})
	l.handle(kdown(vkLShift), now)
	l.handle(kdown(vkLShift, vkLShift), now.Add(390*time.Millisecond))
	want(t, "held 401 ms", l.handle(kup(vkLShift, vkLShift), now.Add(401*time.Millisecond)))
	l.handle(kdown(vkLShift), now.Add(time.Second))
	want(t, "tap", l.handle(kup(vkLShift, vkLShift), now.Add(1100*time.Millisecond)), HotkeyToggle)
}

func TestModifierExpansionDoesNotRenewTheTapTime(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{Toggle: "left_shift+left_control"})
	l.handle(kdown(vkLShift), now)
	l.handle(kdown(vkLControl, vkLShift), now.Add(390*time.Millisecond))
	want(t, "late", l.handle(kup(vkLControl, vkLShift, vkLControl), now.Add(401*time.Millisecond)))
	l.handle(kup(vkLShift, vkLShift), now.Add(500*time.Millisecond))
	l.handle(kdown(vkLShift), now.Add(time.Second))
	l.handle(kdown(vkLControl, vkLShift), now.Add(time.Second+100*time.Millisecond))
	want(t, "in time", l.handle(kup(vkLControl, vkLShift, vkLControl), now.Add(time.Second+200*time.Millisecond)), HotkeyToggle)
}

func TestAMouseClickCancelsATapAndTheNextTapWorks(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{Toggle: "left_shift"})
	if !l.needsMouse() {
		t.Fatal("a modifier tap needs the mouse hook")
	}
	l.handle(kdown(vkLShift), now)
	l.cancelModifierTap()
	want(t, "after a click", l.handle(kup(vkLShift, vkLShift), now))
	l.handle(kdown(vkLShift), now)
	want(t, "next tap", l.handle(kup(vkLShift, vkLShift), now), HotkeyToggle)
}

func TestARegularKeyMakesModifiersAShortcutNotATap(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{Toggle: "left_control"})
	l.handle(kdown(vkLControl), now)
	l.handle(kdown(keyL, vkLControl), now)
	l.handle(kup(keyL, vkLControl), now)
	want(t, "ctrl+l", l.handle(kup(vkLControl, vkLControl), now))
}

func TestChordPushToTalkAfterAMissedCtrlRelease(t *testing.T) {
	now := time.Now()
	for binding, expected := range map[string][]HotkeyEventKind{"shift+d": {HotkeyPushToTalkDown}, "ctrl+shift+d": nil} {
		l := logicWith(t, HotkeyConfig{PushToTalk: binding})
		l.handle(kdown(vkLControl), now)
		// Ctrl's release happened on the secure desktop; async state is now up.
		want(t, binding, l.handle(kdown(keyD, vkLShift), now), expected...)
	}
}

func TestSoloPushToTalkStartsAtOnceAndMasksAlt(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{PushToTalk: "right_option"})
	out := l.handle(kdown(vkRMenu), now)
	want(t, "down", out, HotkeyPushToTalkDown)
	if !out.mask {
		t.Fatal("a held Right Alt must not open the menu bar")
	}
	want(t, "repeat", l.handle(kdown(vkRMenu, vkRMenu), now))
	want(t, "up", l.handle(kup(vkRMenu, vkRMenu), now), HotkeyPushToTalkUp)
}

func TestAltGrCtrlIsIgnored(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{PushToTalk: "right_option"})
	fake := rawKey{vk: vkLControl, scan: 0x21D, down: true}
	want(t, "fake ctrl", l.handle(fake, now))
	want(t, "right alt", l.handle(kdown(vkRMenu, vkLControl), now), HotkeyPushToTalkDown)
}

func TestModifierOnlyPushToTalkNeedsExactlyItsKeys(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{PushToTalk: "left_control+left_command"})
	want(t, "ctrl", l.handle(kdown(vkLControl), now))
	want(t, "ctrl+win", l.handle(kdown(vkLWin, vkLControl), now), HotkeyPushToTalkDown)
	want(t, "shift too", l.handle(kdown(vkLShift, vkLControl, vkLWin), now), HotkeyPushToTalkUp)
}

func TestInjectedKeysNeverTrigger(t *testing.T) {
	l := logicWith(t, HotkeyConfig{PushToTalk: "right_option"})
	ev := kdown(vkRMenu)
	ev.flags = llkhfInjected
	want(t, "injected", l.handle(ev, time.Now()))
}

func TestChordsFireOnceAndTheirKeyNeverReachesTheApp(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{Toggle: "ctrl+space", PasteLast: "alt+shift+v", StyleHotkeys: map[string]string{"formal": "ctrl+alt+1"}})
	out := l.handle(kdown(vkSpace, vkLControl), now)
	want(t, "toggle", out, HotkeyToggle)
	if !out.swallow {
		t.Fatal("the chord key reached the app")
	}
	if out := l.handle(kdown(vkSpace, vkLControl), now); len(out.events) != 0 || !out.swallow {
		t.Fatalf("repeat: %+v", out)
	}
	if out := l.handle(kup(vkSpace, vkLControl), now); !out.swallow {
		t.Fatal("the chord release reached the app")
	}
	if out := l.handle(kdown(vkSpace), now); len(out.events) != 0 || out.swallow {
		t.Fatalf("a plain space: %+v", out)
	}
	l.handle(kup(vkSpace), now)
	out = l.handle(kdown(vkV, vkLMenu, vkLShift), now)
	want(t, "paste last", out, HotkeyPasteLast)
	if !out.mask {
		t.Fatal("Alt of the chord must be masked")
	}
	out = l.handle(kdown('1', vkLControl, vkLMenu), now)
	if len(out.events) != 1 || out.events[0] != (HotkeyEvent{Kind: HotkeyStyle, StyleID: "formal"}) {
		t.Fatalf("%+v", out.events)
	}
	want(t, "ctrl+shift+space is another chord", l.handle(kdown(vkSpace, vkLControl, vkLShift), now))
}

func TestEnterSendsOnceWhileRecordingAndIsSwallowed(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{EnterSends: true})
	if out := l.handle(kdown(vkReturn), now); out.swallow || len(out.events) != 0 {
		t.Fatal("Enter is taken only while recording")
	}
	l.handle(kup(vkReturn), now)
	l.setRecording(true)
	out := l.handle(kdown(vkReturn), now)
	want(t, "enter", out, HotkeyEnterSend)
	if !out.swallow {
		t.Fatal("not swallowed")
	}
	if out := l.handle(kdown(vkReturn), now); !out.swallow || len(out.events) != 0 {
		t.Fatalf("repeat: %+v", out)
	}
	if out := l.handle(kup(vkReturn), now); !out.swallow {
		t.Fatal("release reached the app")
	}
	want(t, "second press sends no second submit", l.handle(kdown(vkReturn), now))
}

func TestEscapeCancelsOnASecondPressWithinTheWindow(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{CancelMode: "double_escape", DoubleEscapeMs: 400})
	want(t, "not recording", l.handle(kdown(vkEscape), now))
	l.handle(kup(vkEscape), now)
	l.setRecording(true)
	want(t, "first", l.handle(kdown(vkEscape), now))
	l.handle(kup(vkEscape), now)
	want(t, "second", l.handle(kdown(vkEscape), now.Add(300*time.Millisecond)), HotkeyCancel)
	l.handle(kup(vkEscape), now)
	want(t, "late", l.handle(kdown(vkEscape), now.Add(time.Second)))
	l.handle(kup(vkEscape), now)
	want(t, "late second", l.handle(kdown(vkEscape), now.Add(2*time.Second)))
	l.handle(kup(vkEscape), now)
	want(t, "ctrl+esc", l.handle(kdown(vkEscape, vkLControl), now.Add(2100*time.Millisecond)))
}

func TestSingleEscapeIsTakenAndOffNeverCancels(t *testing.T) {
	now := time.Now()
	l := logicWith(t, HotkeyConfig{CancelMode: "single_escape", TakeEscape: true})
	l.setRecording(true)
	l.handle(kdown(vkLControl), now) // a release lost on the secure desktop
	out := l.handle(kdown(vkEscape), now)
	want(t, "single", out, HotkeyCancel)
	if !out.swallow {
		t.Fatal("Esc reached the app")
	}
	off := logicWith(t, HotkeyConfig{CancelMode: "off", TakeEscape: true})
	off.setRecording(true)
	want(t, "off", off.handle(kdown(vkEscape), now))
}

func TestPruneForgetsKeysReleasedOnTheSecureDesktop(t *testing.T) {
	now := time.Now()
	for vk, binding := range map[uint16]string{vkLWin: "left_command", vkLControl: "left_control"} {
		l := logicWith(t, HotkeyConfig{Toggle: binding})
		l.handle(kdown(vk), now)
		l.handle(kdown(keyL, vk), now)
		// Both releases were lost.
		l.prune(vk, func(uint16) bool { return false })
		l.handle(kdown(vk), now)
		want(t, binding, l.handle(kup(vk, vk), now), HotkeyToggle)
	}
}

func TestAReconfigureReleasesAHeldPushToTalk(t *testing.T) {
	l := logicWith(t, HotkeyConfig{PushToTalk: "right_option"})
	l.handle(kdown(vkRMenu), time.Now())
	released, err := l.configure(HotkeyConfig{PushToTalk: "left_option"})
	if err != nil || len(released) != 1 || released[0].Kind != HotkeyPushToTalkUp {
		t.Fatalf("%v %v", released, err)
	}
	if _, err := l.configure(HotkeyConfig{PushToTalk: "fn"}); err == nil {
		t.Fatal("Windows cannot see fn")
	}
}
