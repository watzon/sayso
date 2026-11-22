package platform

// The platform-neutral part of the global hotkeys: the hotkey string format,
// and the decisions on raw key events. No system calls here, so every rule
// is unit tested with plain values. The native listeners turn each system
// event into a rawKeyEvent and do what hotkeyEngine.handle answers.
//
// Ported from sayso-core/src/hotkey.rs, sayso-platform-common (esc.rs,
// enter_send.rs), and sayso-platform-macos/src/tap_logic.rs.

import (
	"fmt"
	"sort"
	"strings"
	"time"
)

// keyKind is the kind of a regular (non-modifier) key.
type keyKind uint8

const (
	keyNone keyKind = iota
	keySpace
	keyEscape
	keyReturn
	keyTab
	keyBackspace
	keyLetter
	keyDigit
	keyF
	keyPunct
)

// hotkeyKey is one regular key. ch holds the lowercase letter, the digit
// ('0' to '9'), or the punctuation character; n holds the F key number.
type hotkeyKey struct {
	kind keyKind
	ch   byte
	n    uint8
}

// punctuationKeys are the punctuation keys of a US keyboard, by the
// character they type without Shift. The native key tables follow this order.
const punctuationKeys = "`-=[]\\;',./"

func letterKey(c byte) hotkeyKey { return hotkeyKey{kind: keyLetter, ch: c} }
func digitKey(d byte) hotkeyKey  { return hotkeyKey{kind: keyDigit, ch: '0' + d} }
func fKey(n uint8) hotkeyKey     { return hotkeyKey{kind: keyF, n: n} }
func punctKey(c byte) hotkeyKey  { return hotkeyKey{kind: keyPunct, ch: c} }

func parseKey(token string) (hotkeyKey, bool) {
	t := strings.ToLower(token)
	switch t {
	case "space":
		return hotkeyKey{kind: keySpace}, true
	case "esc", "escape":
		return hotkeyKey{kind: keyEscape}, true
	case "return", "enter":
		return hotkeyKey{kind: keyReturn}, true
	case "tab":
		return hotkeyKey{kind: keyTab}, true
	case "backspace", "delete":
		return hotkeyKey{kind: keyBackspace}, true
	}
	if len(t) == 1 {
		c := t[0]
		switch {
		case c >= 'a' && c <= 'z':
			return letterKey(c), true
		case c >= '0' && c <= '9':
			return digitKey(c - '0'), true
		case strings.IndexByte(punctuationKeys, c) >= 0:
			return punctKey(c), true
		}
		return hotkeyKey{}, false
	}
	if t[0] == 'f' && len(t) <= 3 {
		n := 0
		for _, c := range t[1:] {
			if c < '0' || c > '9' {
				return hotkeyKey{}, false
			}
			n = n*10 + int(c-'0')
		}
		if n >= 1 && n <= 20 {
			return fKey(uint8(n)), true
		}
	}
	return hotkeyKey{}, false
}

func (k hotkeyKey) String() string {
	switch k.kind {
	case keySpace:
		return "space"
	case keyEscape:
		return "esc"
	case keyReturn:
		return "return"
	case keyTab:
		return "tab"
	case keyBackspace:
		return "backspace"
	case keyLetter, keyDigit, keyPunct:
		return string(k.ch)
	case keyF:
		return fmt.Sprintf("f%d", k.n)
	}
	return "none"
}

// soloModifier is one side-specific modifier key. The values number the
// bits of a modifierSet.
type soloModifier uint8

const (
	modRightOption soloModifier = iota
	modLeftOption
	modRightCommand
	modLeftCommand
	modRightControl
	modLeftControl
	modRightShift
	modLeftShift
	modFn
)

// allSoloModifiers in the order of the config format.
var allSoloModifiers = [9]soloModifier{
	modFn, modLeftCommand, modLeftControl, modLeftOption, modLeftShift,
	modRightCommand, modRightControl, modRightOption, modRightShift,
}

var soloTokens = map[soloModifier]string{
	modRightOption:  "right_option",
	modLeftOption:   "left_option",
	modRightCommand: "right_command",
	modLeftCommand:  "left_command",
	modRightControl: "right_control",
	modLeftControl:  "left_control",
	modRightShift:   "right_shift",
	modLeftShift:    "left_shift",
	modFn:           "fn",
}

func parseSolo(token string) (soloModifier, bool) {
	token = strings.TrimSpace(token)
	for _, m := range allSoloModifiers {
		if strings.EqualFold(token, soloTokens[m]) {
			return m, true
		}
	}
	return 0, false
}

// modifierSet is a set of side-specific modifier keys.
type modifierSet uint16

func setOf(mods ...soloModifier) modifierSet {
	var s modifierSet
	for _, m := range mods {
		s |= 1 << m
	}
	return s
}

func (s modifierSet) has(m soloModifier) bool            { return s&(1<<m) != 0 }
func (s modifierSet) isSubsetOf(o modifierSet) bool      { return s&o == s }
func (s modifierSet) without(m soloModifier) modifierSet { return s &^ (1 << m) }

func (s modifierSet) len() int {
	n := 0
	for v := s; v != 0; v &= v - 1 {
		n++
	}
	return n
}

// only is the single modifier of a set of one.
func (s modifierSet) only() (soloModifier, bool) {
	if s.len() != 1 {
		return 0, false
	}
	for _, m := range allSoloModifiers {
		if s.has(m) {
			return m, true
		}
	}
	return 0, false
}

// modifiers is the side-independent form of a set.
func (s modifierSet) modifiers() modifiers {
	return modifiers{
		command:  s&setOf(modLeftCommand, modRightCommand) != 0,
		option:   s&setOf(modLeftOption, modRightOption) != 0,
		control:  s&setOf(modLeftControl, modRightControl) != 0,
		shift:    s&setOf(modLeftShift, modRightShift) != 0,
		function: s.has(modFn),
	}
}

func (s modifierSet) String() string {
	var parts []string
	for _, m := range allSoloModifiers {
		if s.has(m) {
			parts = append(parts, soloTokens[m])
		}
	}
	return strings.Join(parts, "+")
}

// modifiers are the side-independent modifiers of a chord. On Linux and
// Windows, option is Alt and command is the Super (Windows) key.
type modifiers struct {
	command, option, control, shift, function bool
}

func (m modifiers) isEmpty() bool {
	return !(m.command || m.option || m.control || m.shift || m.function)
}

// plain is true when no Command, Control, or Option is down. Only a plain Esc cancels.
func (m modifiers) plain() bool { return !(m.command || m.control || m.option) }

func (m modifiers) withoutFn() modifiers { m.function = false; return m }

type hotkeyKind uint8

const (
	hotkeyChord hotkeyKind = iota + 1
	// hotkeyModifierOnly is one modifier on its own ("right_option") or several
	// held together ("left_command+left_option").
	hotkeyModifierOnly
)

type hotkey struct {
	kind hotkeyKind
	mods modifiers   // chord only
	key  hotkeyKey   // chord only
	keys modifierSet // modifier-only only
}

func (h hotkey) String() string {
	if h.kind == hotkeyModifierOnly {
		return h.keys.String()
	}
	var parts []string
	for _, p := range []struct {
		on   bool
		name string
	}{{h.mods.function, "fn"}, {h.mods.control, "ctrl"}, {h.mods.option, "opt"}, {h.mods.shift, "shift"}, {h.mods.command, "cmd"}} {
		if p.on {
			parts = append(parts, p.name)
		}
	}
	return strings.Join(append(parts, h.key.String()), "+")
}

// overlaps is true when the same key press can fire both bindings: the same
// hotkey, or two modifier-only sets where one holds the other.
func (h hotkey) overlaps(o hotkey) bool {
	if h == o {
		return true
	}
	if h.kind == hotkeyModifierOnly && o.kind == hotkeyModifierOnly {
		return h.keys.isSubsetOf(o.keys) || o.keys.isSubsetOf(h.keys)
	}
	return false
}

// parseHotkey reads "opt+space", "ctrl+cmd+v", a single modifier such as
// "right_option" or "fn", or a combination such as "left_command+left_option".
// The Windows and Linux names (alt, win, super) are accepted too.
func parseHotkey(s string) (hotkey, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return hotkey{}, fmt.Errorf("the hotkey is empty")
	}
	parts := strings.Split(s, "+")
	var keys modifierSet
	modifierOnly := true
	for _, part := range parts {
		m, ok := parseSolo(part)
		if !ok {
			modifierOnly = false
			break
		}
		if keys.has(m) {
			return hotkey{}, fmt.Errorf("a hotkey needs exactly one key after its modifiers")
		}
		keys |= 1 << m
	}
	if modifierOnly {
		return hotkey{kind: hotkeyModifierOnly, keys: keys}, nil
	}
	h := hotkey{kind: hotkeyChord}
	found := false
	for _, part := range parts {
		part = strings.TrimSpace(part)
		switch strings.ToLower(part) {
		case "cmd", "command", "super", "win", "meta", "logo":
			h.mods.command = true
		case "opt", "option", "alt":
			h.mods.option = true
		case "ctrl", "control":
			h.mods.control = true
		case "shift":
			h.mods.shift = true
		case "fn":
			h.mods.function = true
		default:
			if found {
				return hotkey{}, fmt.Errorf("a hotkey needs exactly one key after its modifiers")
			}
			k, ok := parseKey(part)
			if !ok {
				return hotkey{}, fmt.Errorf("unknown key name %q", part)
			}
			h.key, found = k, true
		}
	}
	if !found {
		return hotkey{}, fmt.Errorf("a hotkey needs exactly one key after its modifiers")
	}
	return h, nil
}

// ---------------------------------------------------------------------------
// Small state machines
// ---------------------------------------------------------------------------

// tapWindow is how long a modifier-only toggle tap may last, from its first
// modifier press to the release.
const tapWindow = 400 * time.Millisecond

// modifierCapture follows a modifier-only gesture: modifiers pressed with no
// regular key in between.
type modifierCapture struct {
	candidate    modifierSet
	active       bool
	tapStarted   time.Time
	tapStartedOK bool
}

func (c *modifierCapture) clear() { *c = modifierCapture{} }

// pressTap records a press of a timed toggle tap. The clock starts at the
// first modifier and is not renewed when more modifiers join.
func (c *modifierCapture) pressTap(held modifierSet, now time.Time) {
	started := now
	if c.tapStartedOK {
		started = c.tapStarted
	}
	c.press(held)
	c.tapStarted, c.tapStartedOK = started, c.active
}

// releaseTap is the release of a timed tap: the gesture, when the release
// came within tapWindow of the first press.
func (c *modifierCapture) releaseTap(m soloModifier, now time.Time) (modifierSet, bool) {
	if !c.tapStartedOK || now.Sub(c.tapStarted) > tapWindow {
		c.clear()
		return 0, false
	}
	return c.release(m)
}

func (c *modifierCapture) press(held modifierSet) {
	if held.len() == 1 || (c.active && c.candidate.isSubsetOf(held)) {
		c.candidate, c.active = held, true
	} else {
		c.candidate, c.active = 0, false
	}
}

func (c *modifierCapture) release(m soloModifier) (modifierSet, bool) {
	if !c.active || !c.candidate.has(m) {
		return 0, false
	}
	keys := c.candidate
	c.clear()
	return keys, keys != 0
}

// escDetector detects a cancel: a double Esc inside a window, or every Esc
// in single mode.
type escDetector struct {
	single  bool
	window  time.Duration
	last    time.Time
	hasLast bool
}

func (d *escDetector) configure(single bool, window time.Duration) {
	*d = escDetector{single: single, window: window}
}

func (d *escDetector) reset() { d.hasLast = false }

// press feeds one Esc key-down (no auto-repeat). True when it completes a cancel.
func (d *escDetector) press(now time.Time) bool {
	if d.single {
		d.hasLast = false
		return true
	}
	double := d.hasLast && now.Sub(d.last) <= d.window
	if double {
		d.hasLast = false
	} else {
		d.last, d.hasLast = now, true
	}
	return double
}

// escTake decides when Sayso takes Esc from the app while a dictation
// records, so the Esc that cancels does not also close a dialog.
type escTake struct {
	on, recording, held bool
}

func (t *escTake) setRecording(recording bool) {
	t.recording = recording
	if recording {
		// A release that the tap did not see must not take the next Esc.
		t.held = false
	}
}

// key is one Esc event. True: the app must not get it.
func (t *escTake) key(down, repeat, plain bool) bool {
	if !down {
		held := t.held
		t.held = false
		return held
	}
	if repeat {
		return t.held
	}
	if t.on && t.recording && plain {
		t.held = true
		return true
	}
	return false
}

// enterSend is Send with Enter: while a dictation records, Enter stops it and
// Sayso presses Enter after the insert. The key itself must not reach the app.
type enterSend struct {
	on, recording, held, sent bool
}

func (e *enterSend) setRecording(recording bool) {
	e.recording = recording
	if recording {
		e.held = false
		e.sent = false
	}
}

// key is one Return or keypad Enter event, with any modifiers. It answers
// whether to take the key and whether to send the dictation.
func (e *enterSend) key(down, repeat bool) (swallow, submit bool) {
	if !down {
		held := e.held
		e.held = false
		return held, false
	}
	if repeat {
		// The repeats of an Enter that Sayso took stay Sayso's.
		return e.held, false
	}
	if e.on && e.recording {
		e.held = true
		submit = !e.sent
		e.sent = true
		return true, submit
	}
	return false, false
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

type rawKind uint8

const (
	rawKeyDown rawKind = iota + 1
	rawKeyUp
	// rawModifiers is a change of the modifier keys (macOS flagsChanged, or a
	// modifier key press or release elsewhere).
	rawModifiers
	// rawPointer is a mouse button press or a scroll.
	rawPointer
)

// rawKeyEvent is one native event, reduced to what the engine uses.
type rawKeyEvent struct {
	kind rawKind
	// code is the native key code. The engine tracks held and taken keys by it.
	code uint16
	// key is the regular key, kind keyNone for keys Sayso does not name.
	// Keypad Enter maps to keyReturn.
	key hotkeyKey
	// held are the side-specific modifier keys down after this event.
	held modifierSet
	// mods are the side-independent modifiers of the event.
	mods modifiers
	// changed is the modifier key of a rawModifiers event, when known.
	changed    soloModifier
	hasChanged bool
	repeat     bool
	// secure: Secure Event Input is on (macOS), so regular keys are hidden.
	secure bool
}

// keyDecision is what the native listener does with one event.
type keyDecision struct {
	events  []HotkeyEvent
	swallow bool
}

type chordBinding struct {
	mods  modifiers
	key   hotkeyKey
	event HotkeyEvent
}

type cancelMode uint8

const (
	cancelDoubleEscape cancelMode = iota
	cancelSingleEscape
	cancelOff
)

// hotkeyBindings is a parsed and checked HotkeyConfig.
type hotkeyBindings struct {
	toggleKeys modifierSet // modifier-only toggle, 0 when none
	ptt        *hotkey
	chords     []chordBinding
	cancel     cancelMode
	escWindow  time.Duration
	enterSends bool
	takeEscape bool
}

// parseBindings checks a HotkeyConfig. Toggle and push-to-talk may be
// modifier-only; the other actions need a chord.
func parseBindings(cfg HotkeyConfig) (hotkeyBindings, error) {
	b := hotkeyBindings{
		enterSends: cfg.EnterSends,
		takeEscape: cfg.TakeEscape,
		escWindow:  time.Duration(cfg.DoubleEscapeMs) * time.Millisecond,
	}
	if b.escWindow <= 0 {
		b.escWindow = 400 * time.Millisecond
	}
	switch cfg.CancelMode {
	case "", "double_escape":
		b.cancel = cancelDoubleEscape
	case "single_escape":
		b.cancel = cancelSingleEscape
	case "off":
		b.cancel = cancelOff
	default:
		return b, fmt.Errorf("unknown cancel mode %q", cfg.CancelMode)
	}
	var toggle *hotkey
	if cfg.Toggle != "" {
		h, err := parseHotkey(cfg.Toggle)
		if err != nil {
			return b, fmt.Errorf("toggle hotkey: %w", err)
		}
		toggle = &h
		if h.kind == hotkeyModifierOnly {
			b.toggleKeys = h.keys
		} else {
			b.chords = append(b.chords, chordBinding{h.mods, h.key, HotkeyEvent{Kind: HotkeyToggle}})
		}
	}
	if cfg.PushToTalk != "" {
		h, err := parseHotkey(cfg.PushToTalk)
		if err != nil {
			return b, fmt.Errorf("push-to-talk hotkey: %w", err)
		}
		if toggle != nil && h.overlaps(*toggle) {
			return b, fmt.Errorf("push-to-talk %q and toggle %q share a gesture", cfg.PushToTalk, cfg.Toggle)
		}
		b.ptt = &h
	}
	chordOnly := []struct {
		name, value string
		event       HotkeyEvent
	}{
		{"paste-last", cfg.PasteLast, HotkeyEvent{Kind: HotkeyPasteLast}},
		{"cycle-style", cfg.CycleStyle, HotkeyEvent{Kind: HotkeyCycleStyle}},
		{"incognito", cfg.Incognito, HotkeyEvent{Kind: HotkeyToggleIncognito}},
	}
	for _, id := range sortedKeys(cfg.StyleHotkeys) {
		chordOnly = append(chordOnly, struct {
			name, value string
			event       HotkeyEvent
		}{"style " + id, cfg.StyleHotkeys[id], HotkeyEvent{Kind: HotkeyStyle, StyleID: id}})
	}
	for _, c := range chordOnly {
		if c.value == "" {
			continue
		}
		h, err := parseHotkey(c.value)
		if err != nil {
			return b, fmt.Errorf("%s hotkey: %w", c.name, err)
		}
		if h.kind != hotkeyChord {
			return b, fmt.Errorf("%s hotkey %q: a modifier key on its own works only for toggle and push-to-talk", c.name, c.value)
		}
		b.chords = append(b.chords, chordBinding{h.mods, h.key, c.event})
	}
	return b, nil
}

func sortedKeys(m map[string]string) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// hotkeyEngine holds every decision on raw key events. It is not safe for
// concurrent use; the native listener guards it with a mutex.
type hotkeyEngine struct {
	b hotkeyBindings
	// canSwallow: the native listener can take events from the app (an
	// active macOS tap, an X11 grab). Without it the decisions still fire
	// events, but every key also reaches the app.
	canSwallow bool
	recording  bool
	pttDown    bool
	esc        escDetector
	take       escTake
	enter      enterSend
	capture    modifierCapture
	pressed    []uint16
	// taken are keys of chords that fired on key down: their repeats and
	// their release go to no app either.
	taken map[uint16]bool
}

func newHotkeyEngine() *hotkeyEngine {
	e := &hotkeyEngine{taken: map[uint16]bool{}}
	e.esc.configure(false, 400*time.Millisecond)
	return e
}

// configure applies new bindings. It returns a push-to-talk release when a
// held push-to-talk is dropped by the change.
func (e *hotkeyEngine) configure(b hotkeyBindings) []HotkeyEvent {
	released := e.releaseHeld()
	e.b = b
	e.esc.configure(b.cancel == cancelSingleEscape, b.escWindow)
	e.enter.on = b.enterSends
	e.take.on = b.takeEscape && e.canSwallow
	e.capture.clear()
	return released
}

// setCanSwallow tells the engine whether the listener can take events.
func (e *hotkeyEngine) setCanSwallow(on bool) {
	e.canSwallow = on
	e.take.on = e.b.takeEscape && on
}

func (e *hotkeyEngine) setRecording(recording bool) {
	e.recording = recording
	e.esc.reset()
	e.take.setRecording(recording)
	e.enter.setRecording(recording)
}

// releaseHeld forgets held keys after the listener may have missed releases
// (the system disabled the tap, or the listener restarted).
func (e *hotkeyEngine) releaseHeld() []HotkeyEvent {
	e.pressed = e.pressed[:0]
	e.capture.clear()
	if e.pttDown {
		e.pttDown = false
		return []HotkeyEvent{{Kind: HotkeyPushToTalkUp}}
	}
	return nil
}

// reconcilePressed drops held keys whose release was hidden (Secure Event
// Input on macOS). Keys that really are down stay: a modifier tap begun while
// a regular key is held must not toggle.
func (e *hotkeyEngine) reconcilePressed(isDown func(code uint16) bool) {
	kept := e.pressed[:0]
	for _, c := range e.pressed {
		if isDown(c) {
			kept = append(kept, c)
		}
	}
	e.pressed = kept
}

func (e *hotkeyEngine) chordMatches(c chordBinding, ev rawKeyEvent) bool {
	return ev.key.kind != keyNone && ev.key == c.key &&
		ev.mods.withoutFn() == c.mods.withoutFn() &&
		(!c.mods.function || ev.mods.function)
}

func (e *hotkeyEngine) pttChord() (chordBinding, bool) {
	if e.b.ptt == nil || e.b.ptt.kind != hotkeyChord {
		return chordBinding{}, false
	}
	return chordBinding{mods: e.b.ptt.mods, key: e.b.ptt.key}, true
}

func (e *hotkeyEngine) setPTT(down bool, d *keyDecision) {
	if e.pttDown == down {
		return
	}
	e.pttDown = down
	kind := HotkeyPushToTalkUp
	if down {
		kind = HotkeyPushToTalkDown
	}
	d.events = append(d.events, HotkeyEvent{Kind: kind})
}

func (e *hotkeyEngine) handle(ev rawKeyEvent, now time.Time) keyDecision {
	var d keyDecision
	switch ev.kind {
	case rawKeyDown:
		if !containsCode(e.pressed, ev.code) {
			e.pressed = append(e.pressed, ev.code)
		}
		e.keyDown(ev, now, &d)
	case rawKeyUp:
		e.pressed = removeCode(e.pressed, ev.code)
		e.keyUp(ev, &d)
	case rawModifiers:
		e.modifiersChanged(ev, now, &d)
	case rawPointer:
		e.capture.clear()
	}
	d.swallow = d.swallow && e.canSwallow
	return d
}

func (e *hotkeyEngine) modifiersChanged(ev rawKeyEvent, now time.Time, d *keyDecision) {
	held := ev.held
	if ev.secure || len(e.pressed) > 0 {
		// The gesture must hold only modifiers, with no hidden keys.
		e.capture.clear()
	} else if e.b.toggleKeys != 0 {
		if ev.hasChanged {
			if held.has(ev.changed) {
				e.capture.pressTap(held, now)
			} else if keys, ok := e.capture.releaseTap(ev.changed, now); ok && keys == e.b.toggleKeys {
				d.events = append(d.events, HotkeyEvent{Kind: HotkeyToggle})
			}
		} else {
			e.capture.clear()
		}
	}
	if e.b.ptt == nil {
		return
	}
	switch p := *e.b.ptt; {
	case p.kind == hotkeyModifierOnly && p.keys.len() > 1:
		e.setPTT(held == p.keys, d)
	case p.kind == hotkeyModifierOnly:
		if m, _ := p.keys.only(); ev.hasChanged && ev.changed == m {
			e.setPTT(held.has(m), d)
		}
	case p.kind == hotkeyChord && e.pttDown && ev.mods.withoutFn() != p.mods.withoutFn():
		// The user let go of a modifier before the key.
		e.setPTT(false, d)
	}
}

func (e *hotkeyEngine) keyDown(ev rawKeyEvent, now time.Time, d *keyDecision) {
	e.capture.clear()
	if e.taken[ev.code] {
		if ev.repeat {
			d.swallow = true
			return
		}
		// A new press: the listener missed the release of the last one.
		delete(e.taken, ev.code)
	}
	if !ev.repeat {
		if c, ok := e.pttChord(); ok && e.chordMatches(c, ev) {
			e.taken[ev.code] = true
			e.setPTT(true, d)
			d.swallow = true
			return
		}
		for _, c := range e.b.chords {
			if e.chordMatches(c, ev) {
				e.taken[ev.code] = true
				d.events = append(d.events, c.event)
				d.swallow = true
				return
			}
		}
	}
	switch ev.key.kind {
	case keyReturn:
		swallow, submit := e.enter.key(true, ev.repeat)
		if submit {
			d.events = append(d.events, HotkeyEvent{Kind: HotkeyEnterSend})
		}
		d.swallow = swallow
	case keyEscape:
		plain := ev.mods.plain()
		d.swallow = e.take.key(true, ev.repeat, plain)
		if !ev.repeat && e.recording && plain && e.b.cancel != cancelOff && e.esc.press(now) {
			d.events = append(d.events, HotkeyEvent{Kind: HotkeyCancel})
		}
	}
}

func (e *hotkeyEngine) keyUp(ev rawKeyEvent, d *keyDecision) {
	if e.taken[ev.code] {
		delete(e.taken, ev.code)
		if c, ok := e.pttChord(); ok && ev.key == c.key {
			e.setPTT(false, d)
		}
		d.swallow = true
		return
	}
	switch ev.key.kind {
	case keyReturn:
		d.swallow, _ = e.enter.key(false, false)
	case keyEscape:
		d.swallow = e.take.key(false, false, ev.mods.plain())
	}
}

func containsCode(codes []uint16, c uint16) bool {
	for _, x := range codes {
		if x == c {
			return true
		}
	}
	return false
}

func removeCode(codes []uint16, c uint16) []uint16 {
	out := codes[:0]
	for _, x := range codes {
		if x != c {
			out = append(out, x)
		}
	}
	return out
}
