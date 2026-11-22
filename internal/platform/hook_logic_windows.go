package platform

import "time"

// The decision logic behind the low-level keyboard hook (hook_logic.rs, esc.rs
// and enter_send.rs of sayso-platform-windows). No system calls, so it is unit
// tested with plain values.
//
// Two Windows details shape it:
//   - The hook has no auto-repeat flag. A held key sends more key-down events
//     with no key-up between them, so the logic remembers which keys are down.
//   - On keyboard layouts with AltGr, Right Alt comes with a fake Left Ctrl
//     whose scan code has the 0x200 bit. The logic ignores that Ctrl, so Right
//     Alt works as a solo push-to-talk key.
//
// Chord bindings (toggle, paste last, cycle style, incognito, style hotkeys)
// fire on the key down and the hook keeps their key from the app, as
// RegisterHotKey does in the Rust app.

const (
	// llkhfInjected: the event came from SendInput or keybd_event.
	llkhfInjected = 0x10
	// llkhfExtended: the key is an extended key (right Ctrl and Alt, arrows).
	llkhfExtended = 0x01
	// altGrCtrlScan is the scan code bit of the fake Left Ctrl of AltGr.
	altGrCtrlScan = 0x200
)

// rawKey is one keyboard event from the hook.
type rawKey struct {
	vk    uint16 // sided virtual-key code
	scan  uint32
	flags uint32
	down  bool
	held  modKeys // the modifiers down before this event
}

// hookOutcome is what the hook does for one event.
type hookOutcome struct {
	events []HotkeyEvent
	// mask: send the mask key now, so the release of a used Alt or Win key
	// opens no menu.
	mask bool
	// swallow keeps the event from the app.
	swallow bool
}

type pttKind int

const (
	pttNone pttKind = iota
	pttSolo
	pttModifierOnly
	pttChord
)

type pttBinding struct {
	kind pttKind
	vk   uint16
	set  modifierSet
	mods modifiers
}

// chordBinding is a chord that fires one event.
type chordBinding struct {
	vk    uint16
	mods  modifiers
	event HotkeyEvent
}

// keySet is which of the 256 key codes are down.
type keySet [4]uint64

func (k *keySet) press(vk uint16) (wasDown bool) {
	w, b := (vk>>6)&3, uint64(1)<<(vk&63)
	wasDown = k[w]&b != 0
	k[w] |= b
	return wasDown
}
func (k *keySet) has(vk uint16) bool { return k[(vk>>6)&3]&(uint64(1)<<(vk&63)) != 0 }
func (k *keySet) release(vk uint16)  { k[(vk>>6)&3] &^= uint64(1) << (vk & 63) }
func (k *keySet) hasRegularKey() bool {
	for vk := 0; vk < 256; vk++ {
		if k.has(uint16(vk)) && !isModifierVK(uint16(vk)) {
			return true
		}
	}
	return false
}

// escDetector decides when Esc cancels: at once, or on a second press within
// the window.
type escDetector struct {
	single  bool
	off     bool
	window  time.Duration
	last    time.Time
	hasLast bool
}

func (e *escDetector) reset() { e.hasLast = false }

func (e *escDetector) press(now time.Time) bool {
	if e.off {
		return false
	}
	if e.single {
		e.hasLast = false
		return true
	}
	double := e.hasLast && now.Sub(e.last) <= e.window
	e.last, e.hasLast = now, !double
	return double
}

// heldTake keeps Enter or Esc from the app while recording: the down, its
// repeats, and its release.
type heldTake struct {
	on, recording, held, sent bool
}

func (h *heldTake) setRecording(r bool) {
	h.recording = r
	if r {
		h.held, h.sent = false, false
	}
}

type hookLogic struct {
	ptt         pttBinding
	pttDown     bool
	toggleTap   modifierSet // a modifier-only toggle; 0 when none
	chords      []chordBinding
	recording   bool
	esc         escDetector
	capture     modifierCapture
	pressed     keySet
	altGrCtrl   bool
	enter, take heldTake
	// swallowUp holds the chord keys whose release is kept from the app.
	swallowUp keySet
}

func newHookLogic() *hookLogic {
	return &hookLogic{esc: escDetector{window: 400 * time.Millisecond}}
}

// configure applies a hotkey config. It returns a push-to-talk release when a
// held push-to-talk is dropped by the change, and the first binding error.
func (l *hookLogic) configure(cfg HotkeyConfig) (released []HotkeyEvent, err error) {
	released = l.releaseHeld()
	l.ptt, l.toggleTap, l.chords = pttBinding{}, 0, nil
	keep := func(e error) {
		if err == nil {
			err = e
		}
	}
	chord := func(s string, ev HotkeyEvent) {
		if s == "" {
			return
		}
		h, e := parseHotkey(s)
		if e != nil {
			keep(e)
			return
		}
		if h.shape != shapeChord || h.mods.function {
			if set, ok := h.modifierKeys(); ok && ev.Kind == HotkeyToggle && canDetect(set) {
				l.toggleTap = set
				return
			}
			keep(errBinding(s))
			return
		}
		vk, ok := vkOf(h.key)
		if !ok {
			keep(errBinding(s))
			return
		}
		l.chords = append(l.chords, chordBinding{vk: vk, mods: h.mods, event: ev})
	}
	chord(cfg.Toggle, HotkeyEvent{Kind: HotkeyToggle})
	chord(cfg.PasteLast, HotkeyEvent{Kind: HotkeyPasteLast})
	chord(cfg.CycleStyle, HotkeyEvent{Kind: HotkeyCycleStyle})
	chord(cfg.Incognito, HotkeyEvent{Kind: HotkeyToggleIncognito})
	for id, s := range cfg.StyleHotkeys {
		chord(s, HotkeyEvent{Kind: HotkeyStyle, StyleID: id})
	}
	if cfg.PushToTalk != "" {
		h, e := parseHotkey(cfg.PushToTalk)
		switch {
		case e != nil:
			keep(e)
		case h.shape == shapeSolo:
			if vk, ok := soloVK(h.solo); ok {
				l.ptt = pttBinding{kind: pttSolo, vk: vk}
			} else {
				keep(errBinding(cfg.PushToTalk))
			}
		case h.shape == shapeModifierOnly && canDetect(h.set):
			l.ptt = pttBinding{kind: pttModifierOnly, set: h.set}
		case h.shape == shapeChord && !h.mods.function:
			if vk, ok := vkOf(h.key); ok {
				l.ptt = pttBinding{kind: pttChord, vk: vk, mods: h.mods}
			}
		default:
			keep(errBinding(cfg.PushToTalk))
		}
	}
	window := time.Duration(cfg.DoubleEscapeMs) * time.Millisecond
	if window <= 0 {
		window = 400 * time.Millisecond
	}
	l.esc = escDetector{single: cfg.CancelMode == "single_escape", off: cfg.CancelMode == "off", window: window}
	l.enter.on = cfg.EnterSends
	l.take.on = cfg.TakeEscape
	l.capture.clear()
	return released, err
}

type errBinding string

func (e errBinding) Error() string { return "Windows cannot listen for the hotkey " + string(e) }

func (l *hookLogic) setRecording(r bool) {
	l.recording = r
	l.esc.reset()
	l.enter.setRecording(r)
	l.take.setRecording(r)
}

// releaseHeld forgets a held push-to-talk, for example when the hook stops.
func (l *hookLogic) releaseHeld() []HotkeyEvent {
	l.capture.clear()
	if l.pttDown {
		l.pttDown = false
		return []HotkeyEvent{{Kind: HotkeyPushToTalkUp}}
	}
	return nil
}

// cancelModifierTap: a mouse button or wheel makes the held modifiers a gesture, not a tap.
func (l *hookLogic) cancelModifierTap() { l.capture.clear() }

// needsMouse: only a modifier-only toggle cares about the mouse.
func (l *hookLogic) needsMouse() bool { return l.toggleTap != 0 }

// prune forgets keys whose release happened where the hook could not see it
// (the secure desktop). isDown is the async key state, from before the event.
func (l *hookLogic) prune(current uint16, isDown func(uint16) bool) {
	for v := 0; v < 256; v++ {
		vk := uint16(v)
		swallowed := (vk == vkReturn && l.enter.held) || (vk == vkEscape && l.take.held) || l.swallowUp.has(vk)
		mod := isModifierVK(vk)
		if (vk != current || mod) && !swallowed && l.pressed.has(vk) && !isDown(vk) {
			l.pressed.release(vk)
			if mod {
				l.capture.clear()
			}
		}
	}
}

func plainKeys(held modKeys) bool {
	m := held.modifiers()
	return !(m.command || m.control || m.option)
}

func (l *hookLogic) handle(ev rawKey, now time.Time) hookOutcome {
	var out hookOutcome
	// Sayso's own SendInput and other programs' synthetic keys never trigger a hotkey.
	if ev.flags&llkhfInjected != 0 {
		return out
	}
	if ev.vk == vkLControl && ev.scan&altGrCtrlScan != 0 {
		l.altGrCtrl = ev.down
		return out
	}
	repeat := false
	if ev.down {
		repeat = l.pressed.press(ev.vk)
	} else {
		l.pressed.release(ev.vk)
	}
	held := ev.held.with(ev.vk, ev.down)
	if l.altGrCtrl {
		held = held.with(vkLControl, l.pressed.has(vkLControl))
	}
	if ev.vk == vkReturn {
		out.swallow = l.enterKey(ev.down, repeat, &out)
	}
	if ev.vk == vkEscape {
		out.swallow = l.escTake(ev.down, repeat, plainKeys(held))
	}
	if isModifierVK(ev.vk) {
		l.modifierEvent(ev, repeat, held, now, &out)
		return out
	}
	if !ev.down {
		if l.swallowUp.has(ev.vk) {
			l.swallowUp.release(ev.vk)
			out.swallow = true
		}
		if l.ptt.kind == pttChord && ev.vk == l.ptt.vk {
			l.setPTT(false, &out)
		}
		return out
	}
	l.capture.clear()
	if repeat {
		if l.swallowUp.has(ev.vk) {
			out.swallow = true
		}
		return out
	}
	mods := held.modifiers()
	if l.ptt.kind == pttChord && ev.vk == l.ptt.vk && mods == l.ptt.mods {
		l.setPTT(true, &out)
	}
	for _, c := range l.chords {
		if c.vk == ev.vk && c.mods == mods {
			out.events = append(out.events, c.event)
			out.swallow = true
			l.swallowUp.press(ev.vk)
			// The Alt or Win of the chord must open no menu when it goes up.
			out.mask = mods.option || mods.command
			break
		}
	}
	if ev.vk == vkEscape && l.recording && plainKeys(held) && l.esc.press(now) {
		out.events = append(out.events, HotkeyEvent{Kind: HotkeyCancel})
	}
	return out
}

func (l *hookLogic) modifierEvent(ev rawKey, repeat bool, held modKeys, now time.Time, out *hookOutcome) {
	keys := modifierKeysOf(held)
	if l.toggleTap != 0 {
		if ev.down && !repeat {
			if !l.pressed.hasRegularKey() {
				l.capture.pressTap(keys, now)
			}
			out.mask = keys == l.toggleTap && l.capture.active && anyReleaseHasMeaning(l.toggleTap)
		} else if !ev.down {
			if m, ok := soloFromVK(ev.vk); ok {
				if h, ok := l.capture.releaseTap(m, now); ok {
					if set, _ := h.modifierKeys(); set == l.toggleTap {
						out.events = append(out.events, HotkeyEvent{Kind: HotkeyToggle})
					}
				}
			}
		}
	}
	switch l.ptt.kind {
	case pttModifierOnly:
		down := keys == l.ptt.set
		if down && !l.pttDown {
			out.mask = anyReleaseHasMeaning(l.ptt.set)
		}
		l.setPTT(down, out)
	case pttSolo:
		if l.ptt.vk == ev.vk {
			if ev.down && !repeat {
				l.setPTT(true, out)
				out.mask = releaseHasMeaning(ev.vk)
			} else if !ev.down {
				l.setPTT(false, out)
			}
		}
	case pttChord:
		// The user let go of a modifier before the key.
		if l.pttDown && held.modifiers() != l.ptt.mods {
			l.setPTT(false, out)
		}
	}
}

func (l *hookLogic) setPTT(down bool, out *hookOutcome) {
	if l.pttDown == down {
		return
	}
	l.pttDown = down
	kind := HotkeyPushToTalkUp
	if down {
		kind = HotkeyPushToTalkDown
	}
	out.events = append(out.events, HotkeyEvent{Kind: kind})
}

// enterKey: with enter_sends, Enter while recording submits once and never
// reaches the app (its repeats and release neither).
func (l *hookLogic) enterKey(down, repeat bool, out *hookOutcome) bool {
	e := &l.enter
	if !down {
		was := e.held
		e.held = false
		return was
	}
	if repeat {
		return e.held
	}
	if e.on && e.recording {
		e.held = true
		if !e.sent {
			e.sent = true
			out.events = append(out.events, HotkeyEvent{Kind: HotkeyEnterSend})
		}
		return true
	}
	return false
}

// escTake: with take_escape, a plain Esc while recording never reaches the app.
func (l *hookLogic) escTake(down, repeat, plain bool) bool {
	t := &l.take
	if !down {
		was := t.held
		t.held = false
		return was
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
