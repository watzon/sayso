package platform

// Global hotkeys on X11 and Wayland. Ported from
// sayso-platform-linux/src/hotkeys/mod.rs. Two kinds of backend work
// together, and the session decides which ones run:
//
//	Job                                    X11 session   Wayland session                  No display
//	Chords (toggle, paste last, styles,    XGrabKey      GlobalShortcuts portal,          evdev
//	chord push-to-talk)                                  else evdev
//	Listen stream (modifier-only toggle    evdev         evdev                            evdev
//	and push-to-talk, Esc and Enter while
//	recording)
//
// A chord backend takes the key, so the focused app does not get it. When
// chords fall back to the evdev stream, the key also reaches the app, and Esc
// and Enter are never taken on Linux. The evdev stream needs read access to
// /dev/input (the input group). Commands on the Unix socket (`sayso
// --toggle`) work in every session. Linux cannot see the Fn key, so hotkeys
// with Fn are refused.

import (
	"errors"
	"log"
	"sync"
	"time"
)

type chordBackend uint8

const (
	chordsNone chordBackend = iota
	chordsX11
	chordsPortal
	chordsListen
)

type linuxHotkeys struct {
	session linuxSession

	mu         sync.Mutex
	engine     *hotkeyEngine
	translator evdevTranslator
	cfg        HotkeyConfig
	bindings   hotkeyBindings
	events     chan HotkeyEvent
	stopped    chan struct{}
	evdev      *evdevReader
	x11        *x11Grabs
	portal     *portalShortcutsClient
	ipc        *commandListener
	chords     chordBackend
}

func newLinuxHotkeys(s linuxSession) *linuxHotkeys {
	return &linuxHotkeys{session: s, engine: newHotkeyEngine()}
}

func (h *linuxHotkeys) Configure(cfg HotkeyConfig) error {
	for _, v := range append([]string{cfg.Toggle, cfg.PushToTalk, cfg.PasteLast, cfg.CycleStyle, cfg.Incognito}, mapValues(cfg.StyleHotkeys)...) {
		if err := checkLinuxHotkey(v); err != nil {
			return err
		}
	}
	b, err := parseBindings(cfg)
	if err != nil {
		return err
	}
	h.mu.Lock()
	h.cfg, h.bindings = cfg, b
	started := h.events != nil
	h.mu.Unlock()
	if started {
		return h.apply()
	}
	return nil
}

func mapValues(m map[string]string) []string {
	out := make([]string, 0, len(m))
	for _, k := range sortedKeys(m) {
		out = append(out, m[k])
	}
	return out
}

func (h *linuxHotkeys) SetRecording(recording bool) {
	h.mu.Lock()
	h.engine.setRecording(recording)
	h.mu.Unlock()
}

func (h *linuxHotkeys) Start(onEvent func(HotkeyEvent)) error {
	if onEvent == nil {
		return errors.New("hotkeys: no event handler")
	}
	h.mu.Lock()
	if h.events != nil {
		h.mu.Unlock()
		return errors.New("hotkeys: already started")
	}
	h.events = make(chan HotkeyEvent, 256)
	h.stopped = make(chan struct{})
	events, stopped := h.events, h.stopped
	h.mu.Unlock()
	go func() {
		for {
			select {
			case ev := <-events:
				onEvent(ev)
			case <-stopped:
				return
			}
		}
	}()

	var problems []error
	ipc, err := listenCommands(h.deliverOne)
	if err != nil {
		problems = append(problems, err)
	}
	ev, err := startEvdev(h.onEvdev)
	if err != nil {
		problems = append(problems, err)
	}
	var x11 *x11Grabs
	var portal *portalShortcutsClient
	switch h.session.kind {
	case sessionX11:
		if x11, err = startX11Grabs(h.deliverOne); err != nil {
			problems = append(problems, err)
		}
	case sessionWayland:
		if portal, err = startPortalShortcuts(h.deliverOne); err != nil {
			problems = append(problems, err)
		}
	}
	h.mu.Lock()
	h.ipc, h.evdev, h.x11, h.portal = ipc, ev, x11, portal
	h.mu.Unlock()
	for _, p := range problems {
		log.Printf("hotkeys: %v", p)
	}
	if ipc == nil && ev == nil && x11 == nil && portal == nil {
		_ = h.Stop()
		return errors.Join(append([]error{errors.New("hotkeys: no backend could start")}, problems...)...)
	}
	// An error here names bindings that could not be set (a chord another app
	// holds); the manager runs, and the other bindings work.
	return h.apply()
}

// apply hands the bindings to the backends that run.
func (h *linuxHotkeys) apply() error {
	h.mu.Lock()
	cfg, b := h.cfg, h.bindings
	switch {
	case h.x11 != nil:
		h.chords = chordsX11
	case h.portal != nil:
		h.chords = chordsPortal
	case h.evdev != nil:
		h.chords = chordsListen
	default:
		h.chords = chordsNone
	}
	backend, x11, portal := h.chords, h.x11, h.portal
	released := h.engine.configure(engineBindingsFor(b, backend))
	h.mu.Unlock()
	h.deliver(released)

	chords := chordList(cfg)
	switch backend {
	case chordsX11:
		keys := map[x11ChordKey]x11ChordAction{}
		for _, c := range chords {
			code, ok := linuxCodeOfKey(c.hotkey.key)
			if !ok {
				continue
			}
			keys[x11ChordKey{keycodeX11(code), x11ModMask(c.hotkey.mods)}] = c.action
		}
		return x11.bind(keys)
	case chordsPortal:
		var bindings []portalBinding
		for _, c := range chords {
			bindings = append(bindings, portalBindingFor(c.hotkey, c.action))
		}
		portal.bind(bindings)
	case chordsNone:
		if len(chords) > 0 {
			return errors.New("hotkeys: no backend for chords; use `sayso --toggle` and the other commands")
		}
	}
	return nil
}

// engineBindingsFor is what the listen stream handles. A chord backend owns
// every chord, including a chord push-to-talk.
func engineBindingsFor(b hotkeyBindings, backend chordBackend) hotkeyBindings {
	if backend == chordsX11 || backend == chordsPortal {
		b.chords = nil
		if b.ptt != nil && b.ptt.kind == hotkeyChord {
			b.ptt = nil
		}
	}
	return b
}

type linuxChord struct {
	hotkey hotkey
	action x11ChordAction
}

// chordList is every chord binding of a checked config.
func chordList(cfg HotkeyConfig) []linuxChord {
	var out []linuxChord
	add := func(value string, action x11ChordAction) {
		if value == "" {
			return
		}
		if h, err := parseHotkey(value); err == nil && h.kind == hotkeyChord {
			out = append(out, linuxChord{h, action})
		}
	}
	add(cfg.Toggle, x11ChordAction{event: HotkeyEvent{Kind: HotkeyToggle}})
	add(cfg.PushToTalk, x11ChordAction{ptt: true})
	add(cfg.PasteLast, x11ChordAction{event: HotkeyEvent{Kind: HotkeyPasteLast}})
	add(cfg.CycleStyle, x11ChordAction{event: HotkeyEvent{Kind: HotkeyCycleStyle}})
	add(cfg.Incognito, x11ChordAction{event: HotkeyEvent{Kind: HotkeyToggleIncognito}})
	for _, id := range sortedKeys(cfg.StyleHotkeys) {
		add(cfg.StyleHotkeys[id], x11ChordAction{event: HotkeyEvent{Kind: HotkeyStyle, StyleID: id}})
	}
	return out
}

func keycodeX11(evdevCode uint16) x11Keycode { return x11Keycode(evdevCode + x11KeycodeOffset) }

func (h *linuxHotkeys) onEvdev(in evdevInput) {
	h.mu.Lock()
	raw, ok := h.translator.translate(in)
	if !ok {
		h.mu.Unlock()
		return
	}
	d := h.engine.handle(raw, time.Now())
	h.mu.Unlock()
	h.deliver(d.events)
}

func (h *linuxHotkeys) deliverOne(ev HotkeyEvent) { h.deliver([]HotkeyEvent{ev}) }

func (h *linuxHotkeys) deliver(events []HotkeyEvent) {
	if len(events) == 0 {
		return
	}
	h.mu.Lock()
	ch := h.events
	h.mu.Unlock()
	if ch == nil {
		return
	}
	for _, ev := range events {
		select {
		case ch <- ev:
		default:
			log.Printf("hotkeys: event queue full, dropped %v", ev.Kind)
		}
	}
}

func (h *linuxHotkeys) Stop() error {
	h.mu.Lock()
	ipc, ev, x11, portal := h.ipc, h.evdev, h.x11, h.portal
	h.ipc, h.evdev, h.x11, h.portal = nil, nil, nil, nil
	if h.stopped != nil {
		close(h.stopped)
	}
	h.events, h.stopped = nil, nil
	h.engine.releaseHeld()
	h.translator = evdevTranslator{}
	h.mu.Unlock()
	if ipc != nil {
		ipc.close()
	}
	if ev != nil {
		ev.close()
	}
	if x11 != nil {
		x11.close()
	}
	if portal != nil {
		portal.close()
	}
	return nil
}

// linuxInputMonitoring: the listen stream can run (evdev), or chords can be
// grabbed (X11).
func linuxInputMonitoring(s linuxSession) bool {
	return evdevReadable() || s.kind == sessionX11
}
