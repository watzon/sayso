package platform

import (
	"reflect"
	"strings"
	"testing"
	"time"
)

// Test key codes. They are opaque to the engine; only equality matters.
const (
	codeA      uint16 = 1
	codeV      uint16 = 2
	codeSpace  uint16 = 3
	codeEsc    uint16 = 4
	codeReturn uint16 = 5
	codeSlash  uint16 = 6
	codeTab    uint16 = 7
)

var t0 = time.Unix(1_700_000_000, 0)

func at(ms int) time.Time { return t0.Add(time.Duration(ms) * time.Millisecond) }

func mustBindings(t *testing.T, cfg HotkeyConfig) hotkeyBindings {
	t.Helper()
	b, err := parseBindings(cfg)
	if err != nil {
		t.Fatalf("parseBindings: %v", err)
	}
	return b
}

func engineWith(t *testing.T, cfg HotkeyConfig, canSwallow bool) *hotkeyEngine {
	t.Helper()
	e := newHotkeyEngine()
	e.setCanSwallow(canSwallow)
	e.configure(mustBindings(t, cfg))
	return e
}

// mod is a modifier event: the modifier m changed and held is the new state.
func mod(m soloModifier, held modifierSet) rawKeyEvent {
	return rawKeyEvent{kind: rawModifiers, held: held, mods: held.modifiers(), changed: m, hasChanged: true}
}

func down(code uint16, key hotkeyKey, held modifierSet) rawKeyEvent {
	return rawKeyEvent{kind: rawKeyDown, code: code, key: key, held: held, mods: held.modifiers()}
}

func repeat(code uint16, key hotkeyKey, held modifierSet) rawKeyEvent {
	ev := down(code, key, held)
	ev.repeat = true
	return ev
}

func up(code uint16, key hotkeyKey, held modifierSet) rawKeyEvent {
	return rawKeyEvent{kind: rawKeyUp, code: code, key: key, held: held, mods: held.modifiers()}
}

var (
	kSpace  = hotkeyKey{kind: keySpace}
	kEsc    = hotkeyKey{kind: keyEscape}
	kReturn = hotkeyKey{kind: keyReturn}
	kA      = letterKey('a')
	kV      = letterKey('v')
	kSlash  = punctKey('/')
)

func kinds(events []HotkeyEvent) []HotkeyEventKind {
	var out []HotkeyEventKind
	for _, e := range events {
		out = append(out, e.Kind)
	}
	return out
}

func expectEvents(t *testing.T, what string, got keyDecision, want ...HotkeyEventKind) {
	t.Helper()
	if !reflect.DeepEqual(kinds(got.events), want) {
		t.Errorf("%s: events %v, want %v", what, kinds(got.events), want)
	}
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

func TestParseHotkeyForms(t *testing.T) {
	cases := []struct {
		in   string
		want hotkey
	}{
		{"opt+space", hotkey{kind: hotkeyChord, mods: modifiers{option: true}, key: kSpace}},
		{"ctrl+cmd+v", hotkey{kind: hotkeyChord, mods: modifiers{control: true, command: true}, key: kV}},
		{"Ctrl+Alt+Space", hotkey{kind: hotkeyChord, mods: modifiers{control: true, option: true}, key: kSpace}},
		{"super+/", hotkey{kind: hotkeyChord, mods: modifiers{command: true}, key: kSlash}},
		{"fn+shift+f5", hotkey{kind: hotkeyChord, mods: modifiers{function: true, shift: true}, key: fKey(5)}},
		{"f20", hotkey{kind: hotkeyChord, key: fKey(20)}},
		{"cmd+`", hotkey{kind: hotkeyChord, mods: modifiers{command: true}, key: punctKey('`')}},
		{"ctrl+\\", hotkey{kind: hotkeyChord, mods: modifiers{control: true}, key: punctKey('\\')}},
		{"right_option", hotkey{kind: hotkeyModifierOnly, keys: setOf(modRightOption)}},
		{"fn", hotkey{kind: hotkeyModifierOnly, keys: setOf(modFn)}},
		{"left_command+left_option", hotkey{kind: hotkeyModifierOnly, keys: setOf(modLeftCommand, modLeftOption)}},
	}
	for _, c := range cases {
		got, err := parseHotkey(c.in)
		if err != nil {
			t.Errorf("parseHotkey(%q): %v", c.in, err)
			continue
		}
		if got != c.want {
			t.Errorf("parseHotkey(%q) = %+v, want %+v", c.in, got, c.want)
		}
	}
	for _, bad := range []string{"", "ctrl", "ctrl+a+b", "ctrl+f21", "ctrl+{", "hyper+a", "right_option+right_option"} {
		if _, err := parseHotkey(bad); err == nil {
			t.Errorf("parseHotkey(%q) accepted", bad)
		}
	}
}

func TestHotkeyStringRoundTrips(t *testing.T) {
	for _, s := range []string{"opt+space", "ctrl+cmd+v", "fn+shift+f5", "right_option", "fn+left_command+left_option"} {
		h, err := parseHotkey(s)
		if err != nil {
			t.Fatal(err)
		}
		again, err := parseHotkey(h.String())
		if err != nil || again != h {
			t.Errorf("%q -> %q -> %+v (%v)", s, h.String(), again, err)
		}
	}
}

func TestBindingsRejectModifierOnlyForChordActionsAndOverlaps(t *testing.T) {
	if _, err := parseBindings(HotkeyConfig{PasteLast: "right_option"}); err == nil {
		t.Error("paste-last accepted a modifier on its own")
	}
	if _, err := parseBindings(HotkeyConfig{StyleHotkeys: map[string]string{"formal": "left_shift"}}); err == nil {
		t.Error("a style hotkey accepted a modifier on its own")
	}
	if _, err := parseBindings(HotkeyConfig{Toggle: "left_command", PushToTalk: "left_command+left_option"}); err == nil {
		t.Error("overlapping modifier-only toggle and push-to-talk accepted")
	}
	if _, err := parseBindings(HotkeyConfig{Toggle: "opt+space", PushToTalk: "opt+space"}); err == nil {
		t.Error("the same chord for toggle and push-to-talk accepted")
	}
	if _, err := parseBindings(HotkeyConfig{CancelMode: "triple"}); err == nil {
		t.Error("unknown cancel mode accepted")
	}
	if _, err := parseBindings(HotkeyConfig{Toggle: "opt+space", PushToTalk: "right_option", PasteLast: "ctrl+cmd+v"}); err != nil {
		t.Errorf("valid bindings rejected: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Modifier-only toggle: the 400 ms tap
// ---------------------------------------------------------------------------

func TestModifierToggleTapAt400msStillToggles(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_command"}, false)
	expectEvents(t, "press", e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(0)))
	expectEvents(t, "release at 400ms", e.handle(mod(modLeftCommand, 0), at(400)), HotkeyToggle)
}

func TestModifierToggleLongHoldDoesNotToggle(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_command"}, false)
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(0))
	expectEvents(t, "release at 401ms", e.handle(mod(modLeftCommand, 0), at(401)))
	// The next clean tap works.
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(1000))
	expectEvents(t, "next tap", e.handle(mod(modLeftCommand, 0), at(1100)), HotkeyToggle)
}

func TestTapDeadlineCountsFromTheFirstModifier(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_command+left_option"}, false)
	both := setOf(modLeftCommand, modLeftOption)
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(0))
	e.handle(mod(modLeftOption, both), at(350))
	expectEvents(t, "release 401ms after the first press", e.handle(mod(modLeftOption, setOf(modLeftCommand)), at(401)))
}

func TestModifierToggleCombinationFiresOnFirstRelease(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_command+left_option"}, false)
	both := setOf(modLeftCommand, modLeftOption)
	e.handle(mod(modLeftOption, setOf(modLeftOption)), at(0))
	e.handle(mod(modLeftCommand, both), at(50))
	expectEvents(t, "first release", e.handle(mod(modLeftCommand, setOf(modLeftOption)), at(100)), HotkeyToggle)
	expectEvents(t, "second release", e.handle(mod(modLeftOption, 0), at(120)))
}

func TestRegularKeyCancelsModifierToggle(t *testing.T) {
	for _, binding := range []string{"left_command", "left_shift", "left_control+left_option"} {
		h, _ := parseHotkey(binding)
		e := engineWith(t, HotkeyConfig{Toggle: binding}, false)
		var held modifierSet
		var got []HotkeyEvent
		for _, m := range allSoloModifiers {
			if h.keys.has(m) {
				held |= setOf(m)
				got = append(got, e.handle(mod(m, held), at(0)).events...)
			}
		}
		got = append(got, e.handle(down(codeA, kA, held), at(10)).events...)
		got = append(got, e.handle(up(codeA, kA, held), at(20)).events...)
		for _, m := range allSoloModifiers {
			if h.keys.has(m) {
				held = held.without(m)
				got = append(got, e.handle(mod(m, held), at(30)).events...)
			}
		}
		if len(got) != 0 {
			t.Errorf("%s fired %v for a normal shortcut", binding, kinds(got))
		}
	}
}

func TestKeyHeldBeforeTheModifierCancelsTheToggle(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_shift"}, false)
	shift := setOf(modLeftShift)
	e.handle(down(codeA, kA, 0), at(0))
	e.handle(mod(modLeftShift, shift), at(10))
	e.handle(up(codeA, kA, shift), at(20))
	expectEvents(t, "release after a held key", e.handle(mod(modLeftShift, 0), at(30)))
	e.handle(mod(modLeftShift, shift), at(40))
	expectEvents(t, "clean tap", e.handle(mod(modLeftShift, 0), at(50)), HotkeyToggle)
}

func TestMouseButtonOrScrollCancelsModifierToggle(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_command"}, false)
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(0))
	expectEvents(t, "pointer", e.handle(rawKeyEvent{kind: rawPointer}, at(10)))
	expectEvents(t, "release after pointer", e.handle(mod(modLeftCommand, 0), at(20)))
	// Cancellation must not poison the next clean tap.
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(30))
	expectEvents(t, "clean tap", e.handle(mod(modLeftCommand, 0), at(40)), HotkeyToggle)
}

func TestReleasingAnExtraModifierDoesNotToggle(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_command"}, false)
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(0))
	e.handle(mod(modLeftOption, setOf(modLeftCommand, modLeftOption)), at(10))
	expectEvents(t, "release extra", e.handle(mod(modLeftOption, setOf(modLeftCommand)), at(20)))
	expectEvents(t, "release bound", e.handle(mod(modLeftCommand, 0), at(30)))
}

func TestSecureInputCancelsModifierToggleButNotSoloPushToTalk(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_shift"}, false)
	ev := mod(modLeftShift, setOf(modLeftShift))
	ev.secure = true
	e.handle(ev, at(0))
	expectEvents(t, "secure release", e.handle(mod(modLeftShift, 0), at(10)))

	e = engineWith(t, HotkeyConfig{PushToTalk: "right_option"}, false)
	ev = mod(modRightOption, setOf(modRightOption))
	ev.secure = true
	expectEvents(t, "ptt down under secure input", e.handle(ev, at(0)), HotkeyPushToTalkDown)
	ev = mod(modRightOption, 0)
	ev.secure = true
	expectEvents(t, "ptt up under secure input", e.handle(ev, at(10)), HotkeyPushToTalkUp)
}

func TestReconcileKeepsReallyHeldKeys(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "left_command"}, false)
	e.handle(down(codeTab, hotkeyKey{kind: keyTab}, 0), at(0))
	e.handle(down(codeA, kA, 0), at(0))
	e.reconcilePressed(func(c uint16) bool { return c == codeA })
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(10))
	expectEvents(t, "A still held", e.handle(mod(modLeftCommand, 0), at(20)))
	e.reconcilePressed(func(uint16) bool { return false })
	e.handle(mod(modLeftCommand, setOf(modLeftCommand)), at(30))
	expectEvents(t, "nothing held", e.handle(mod(modLeftCommand, 0), at(40)), HotkeyToggle)
}

// ---------------------------------------------------------------------------
// Push-to-talk
// ---------------------------------------------------------------------------

func TestSoloPushToTalkStartsOnPressAndStopsOnRelease(t *testing.T) {
	e := engineWith(t, HotkeyConfig{PushToTalk: "right_option"}, false)
	expectEvents(t, "press", e.handle(mod(modRightOption, setOf(modRightOption)), at(0)), HotkeyPushToTalkDown)
	// A long hold does not matter for push-to-talk, and other keys do not stop it.
	expectEvents(t, "key while held", e.handle(down(codeA, kA, setOf(modRightOption)), at(100)))
	expectEvents(t, "release", e.handle(mod(modRightOption, 0), at(5000)), HotkeyPushToTalkUp)
}

func TestSoloPushToTalkIgnoresTheOtherSide(t *testing.T) {
	e := engineWith(t, HotkeyConfig{PushToTalk: "right_option"}, false)
	expectEvents(t, "left option", e.handle(mod(modLeftOption, setOf(modLeftOption)), at(0)))
	expectEvents(t, "left option up", e.handle(mod(modLeftOption, 0), at(10)))
}

func TestModifierCombinationPushToTalk(t *testing.T) {
	e := engineWith(t, HotkeyConfig{PushToTalk: "left_control+left_option"}, false)
	both := setOf(modLeftControl, modLeftOption)
	expectEvents(t, "first", e.handle(mod(modLeftControl, setOf(modLeftControl)), at(0)))
	expectEvents(t, "both", e.handle(mod(modLeftOption, both), at(10)), HotkeyPushToTalkDown)
	expectEvents(t, "extra modifier", e.handle(mod(modLeftShift, both|setOf(modLeftShift)), at(20)), HotkeyPushToTalkUp)
	expectEvents(t, "extra released", e.handle(mod(modLeftShift, both), at(30)), HotkeyPushToTalkDown)
	expectEvents(t, "one released", e.handle(mod(modLeftOption, setOf(modLeftControl)), at(40)), HotkeyPushToTalkUp)
}

func TestChordPushToTalkTakesTheKeyAndStopsOnKeyOrModifierRelease(t *testing.T) {
	e := engineWith(t, HotkeyConfig{PushToTalk: "cmd+/"}, true)
	cmd := setOf(modLeftCommand)
	d := e.handle(down(codeSlash, kSlash, cmd), at(0))
	expectEvents(t, "chord", d, HotkeyPushToTalkDown)
	if !d.swallow {
		t.Error("the chord key reached the app")
	}
	if d := e.handle(repeat(codeSlash, kSlash, cmd), at(50)); !d.swallow || len(d.events) != 0 {
		t.Errorf("repeat: %+v", d)
	}
	d = e.handle(up(codeSlash, kSlash, cmd), at(100))
	expectEvents(t, "key up", d, HotkeyPushToTalkUp)
	if !d.swallow {
		t.Error("the release reached the app")
	}

	e.handle(down(codeSlash, kSlash, cmd), at(200))
	expectEvents(t, "modifier released first", e.handle(mod(modLeftCommand, 0), at(250)), HotkeyPushToTalkUp)
	if d := e.handle(up(codeSlash, kSlash, 0), at(300)); len(d.events) != 0 || !d.swallow {
		t.Errorf("late key up: %+v", d)
	}
}

func TestChangingBindingsReleasesAHeldPushToTalk(t *testing.T) {
	e := engineWith(t, HotkeyConfig{PushToTalk: "right_option"}, false)
	e.handle(mod(modRightOption, setOf(modRightOption)), at(0))
	got := e.configure(mustBindings(t, HotkeyConfig{PushToTalk: "right_command"}))
	if !reflect.DeepEqual(kinds(got), []HotkeyEventKind{HotkeyPushToTalkUp}) {
		t.Errorf("configure returned %v", kinds(got))
	}
	if got := e.releaseHeld(); len(got) != 0 {
		t.Errorf("a second release: %v", kinds(got))
	}
}

// ---------------------------------------------------------------------------
// Chords
// ---------------------------------------------------------------------------

func TestChordsFireOnceAndAreTakenWithRepeatsAndRelease(t *testing.T) {
	e := engineWith(t, HotkeyConfig{
		Toggle:       "opt+space",
		PasteLast:    "ctrl+cmd+v",
		CycleStyle:   "ctrl+opt+tab",
		StyleHotkeys: map[string]string{"formal": "ctrl+opt+f1"},
	}, true)
	opt := setOf(modLeftOption)
	d := e.handle(down(codeSpace, kSpace, opt), at(0))
	expectEvents(t, "toggle", d, HotkeyToggle)
	if !d.swallow {
		t.Error("toggle chord reached the app")
	}
	if d := e.handle(repeat(codeSpace, kSpace, opt), at(30)); !d.swallow || len(d.events) != 0 {
		t.Errorf("repeat: %+v", d)
	}
	if d := e.handle(up(codeSpace, kSpace, opt), at(60)); !d.swallow || len(d.events) != 0 {
		t.Errorf("release: %+v", d)
	}
	// Space alone is the app's.
	if d := e.handle(down(codeSpace, kSpace, 0), at(100)); d.swallow || len(d.events) != 0 {
		t.Errorf("plain space: %+v", d)
	}
	e.handle(up(codeSpace, kSpace, 0), at(110))
	// Extra modifiers do not match.
	if d := e.handle(down(codeSpace, kSpace, opt|setOf(modLeftShift)), at(120)); len(d.events) != 0 {
		t.Errorf("shift+opt+space fired %v", kinds(d.events))
	}
	expectEvents(t, "paste last", e.handle(down(codeV, kV, setOf(modRightControl, modLeftCommand)), at(200)), HotkeyPasteLast)

	d = e.handle(down(10, fKey(1), setOf(modLeftControl, modRightOption)), at(300))
	expectEvents(t, "style", d, HotkeyStyle)
	if len(d.events) == 1 && d.events[0].StyleID != "formal" {
		t.Errorf("style id %q", d.events[0].StyleID)
	}
}

func TestListenOnlyEngineNeverSwallows(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "opt+space", EnterSends: true, TakeEscape: true}, false)
	e.setRecording(true)
	for _, ev := range []rawKeyEvent{
		down(codeSpace, kSpace, setOf(modLeftOption)),
		down(codeReturn, kReturn, 0),
		down(codeEsc, kEsc, 0),
	} {
		if d := e.handle(ev, at(0)); d.swallow {
			t.Errorf("swallowed %+v", ev)
		}
	}
}

func TestFnChordNeedsFn(t *testing.T) {
	e := engineWith(t, HotkeyConfig{Toggle: "fn+f5"}, true)
	expectEvents(t, "f5 without fn", e.handle(down(20, fKey(5), 0), at(0)))
	e.handle(up(20, fKey(5), 0), at(10))
	expectEvents(t, "fn+f5", e.handle(down(20, fKey(5), setOf(modFn)), at(20)), HotkeyToggle)
	// A chord without fn still matches when the system sets fn on its own (F keys, arrows).
	e = engineWith(t, HotkeyConfig{Toggle: "shift+f5"}, true)
	expectEvents(t, "shift+f5 with system fn", e.handle(down(20, fKey(5), setOf(modLeftShift, modFn)), at(0)), HotkeyToggle)
}

// ---------------------------------------------------------------------------
// Esc and Enter while recording
// ---------------------------------------------------------------------------

func TestDoubleEscapeCancelsOnlyWhileRecording(t *testing.T) {
	e := engineWith(t, HotkeyConfig{CancelMode: "double_escape", DoubleEscapeMs: 400}, false)
	e.handle(down(codeEsc, kEsc, 0), at(0))
	expectEvents(t, "not recording", e.handle(down(codeEsc, kEsc, 0), at(100)))

	e.setRecording(true)
	expectEvents(t, "first", e.handle(down(codeEsc, kEsc, 0), at(1000)))
	e.handle(up(codeEsc, kEsc, 0), at(1050))
	expectEvents(t, "second at the window edge", e.handle(down(codeEsc, kEsc, 0), at(1400)), HotkeyCancel)
	// A cancel consumes both presses.
	expectEvents(t, "third", e.handle(down(codeEsc, kEsc, 0), at(1500)))
	// A slow press starts a new pair.
	expectEvents(t, "slow", e.handle(down(codeEsc, kEsc, 0), at(2500)))
	expectEvents(t, "pair", e.handle(down(codeEsc, kEsc, 0), at(2600)), HotkeyCancel)
}

func TestDoubleEscapeIgnoresRepeatsAndModifiedEsc(t *testing.T) {
	e := engineWith(t, HotkeyConfig{}, false)
	e.setRecording(true)
	e.handle(down(codeEsc, kEsc, 0), at(0))
	expectEvents(t, "auto-repeat", e.handle(repeat(codeEsc, kEsc, 0), at(50)))
	expectEvents(t, "cmd+esc", e.handle(down(codeEsc, kEsc, setOf(modLeftCommand)), at(100)))
	// Shift does not make Esc unplain.
	expectEvents(t, "shift+esc", e.handle(down(codeEsc, kEsc, setOf(modLeftShift)), at(150)), HotkeyCancel)
}

func TestSetRecordingForgetsHalfADoubleEscape(t *testing.T) {
	e := engineWith(t, HotkeyConfig{}, false)
	e.setRecording(true)
	e.handle(down(codeEsc, kEsc, 0), at(0))
	e.setRecording(true)
	expectEvents(t, "after reset", e.handle(down(codeEsc, kEsc, 0), at(100)))
}

func TestSingleEscapeAndOff(t *testing.T) {
	e := engineWith(t, HotkeyConfig{CancelMode: "single_escape"}, false)
	e.setRecording(true)
	expectEvents(t, "single", e.handle(down(codeEsc, kEsc, 0), at(0)), HotkeyCancel)
	expectEvents(t, "single again", e.handle(down(codeEsc, kEsc, 0), at(5000)), HotkeyCancel)

	e = engineWith(t, HotkeyConfig{CancelMode: "off"}, false)
	e.setRecording(true)
	e.handle(down(codeEsc, kEsc, 0), at(0))
	expectEvents(t, "off", e.handle(down(codeEsc, kEsc, 0), at(50)))
}

func TestTakeEscapeSwallowsPlainEscWhileRecording(t *testing.T) {
	e := engineWith(t, HotkeyConfig{TakeEscape: true}, true)
	if d := e.handle(down(codeEsc, kEsc, 0), at(0)); d.swallow {
		t.Error("Esc outside a recording was taken")
	}
	e.handle(up(codeEsc, kEsc, 0), at(10))
	e.setRecording(true)
	if d := e.handle(down(codeEsc, kEsc, 0), at(100)); !d.swallow {
		t.Error("Esc while recording reached the app")
	}
	if d := e.handle(repeat(codeEsc, kEsc, 0), at(150)); !d.swallow {
		t.Error("its repeat reached the app")
	}
	// The recording stops (the cancel), then the held Esc is released.
	e.setRecording(false)
	if d := e.handle(up(codeEsc, kEsc, 0), at(200)); !d.swallow {
		t.Error("its release reached the app")
	}
	if d := e.handle(down(codeEsc, kEsc, 0), at(300)); d.swallow {
		t.Error("the next Esc after the recording was taken")
	}
	e.handle(up(codeEsc, kEsc, 0), at(310))
	e.setRecording(true)
	if d := e.handle(down(codeEsc, kEsc, setOf(modLeftCommand)), at(400)); d.swallow {
		t.Error("cmd+Esc was taken")
	}
}

func TestEnterSendsSubmitsOnceAndTakesTheKey(t *testing.T) {
	e := engineWith(t, HotkeyConfig{EnterSends: true}, true)
	if d := e.handle(down(codeReturn, kReturn, 0), at(0)); d.swallow || len(d.events) != 0 {
		t.Errorf("Enter outside a recording: %+v", d)
	}
	e.handle(up(codeReturn, kReturn, 0), at(10))
	e.setRecording(true)
	d := e.handle(down(codeReturn, kReturn, setOf(modRightOption)), at(100))
	expectEvents(t, "enter", d, HotkeyEnterSend)
	if !d.swallow {
		t.Error("Enter reached the app")
	}
	if d := e.handle(repeat(codeReturn, kReturn, 0), at(150)); !d.swallow || len(d.events) != 0 {
		t.Errorf("repeat: %+v", d)
	}
	if d := e.handle(up(codeReturn, kReturn, 0), at(200)); !d.swallow {
		t.Error("the release reached the app")
	}
	d = e.handle(down(codeReturn, kReturn, 0), at(300))
	expectEvents(t, "second press in the same recording", d)
	if !d.swallow {
		t.Error("second press reached the app")
	}
}

func TestEnterSendsOffPassesEnter(t *testing.T) {
	e := engineWith(t, HotkeyConfig{}, true)
	e.setRecording(true)
	if d := e.handle(down(codeReturn, kReturn, 0), at(0)); d.swallow || len(d.events) != 0 {
		t.Errorf("%+v", d)
	}
}

func TestEnterHeldFromBeforeTheRecordingDoesNotSubmit(t *testing.T) {
	e := engineWith(t, HotkeyConfig{EnterSends: true}, true)
	e.handle(down(codeReturn, kReturn, 0), at(0))
	e.setRecording(true)
	if d := e.handle(repeat(codeReturn, kReturn, 0), at(50)); d.swallow || len(d.events) != 0 {
		t.Errorf("its repeat: %+v", d)
	}
	if d := e.handle(up(codeReturn, kReturn, 0), at(100)); d.swallow {
		t.Errorf("its release: %+v", d)
	}
	expectEvents(t, "a new press", e.handle(down(codeReturn, kReturn, 0), at(200)), HotkeyEnterSend)
}

func TestModifierSetString(t *testing.T) {
	if got := setOf(modLeftOption, modFn).String(); !strings.Contains(got, "fn") || !strings.Contains(got, "left_option") {
		t.Errorf("String() = %q", got)
	}
}
