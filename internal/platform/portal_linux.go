package platform

// Chords through the XDG GlobalShortcuts portal, for Wayland sessions (GNOME
// 48 and later, KDE Plasma, Hyprland). Ported from
// sayso-platform-linux/src/hotkeys/portal.rs.
//
// The portal asks the user to confirm the shortcuts in its own dialog the
// first time, and the user can choose other keys there. Activated and
// Deactivated give press and release, so push-to-talk works for chords too.
// The compositor takes the keys, so the focused app does not get them.

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"log"
	"strings"
	"sync"
	"time"

	"github.com/godbus/dbus/v5"
)

const (
	portalDest      = "org.freedesktop.portal.Desktop"
	portalPath      = dbus.ObjectPath("/org/freedesktop/portal/desktop")
	portalShortcuts = "org.freedesktop.portal.GlobalShortcuts"
	portalRequest   = "org.freedesktop.portal.Request"
	portalSession   = "org.freedesktop.portal.Session"
	// portalBindWait is how long a bind waits, which includes the time the
	// user spends in the portal dialog.
	portalBindWait = 5 * time.Minute
	portalCallWait = 10 * time.Second
)

// portalBinding is one shortcut to bind. The portal remembers the user's
// choice under its id.
type portalBinding struct {
	id          string
	description string
	trigger     string
	action      x11ChordAction
}

type portalShortcutsClient struct {
	conn    *dbus.Conn
	emit    func(HotkeyEvent)
	signals chan *dbus.Signal

	mu       sync.Mutex
	session  dbus.ObjectPath
	bound    map[string]x11ChordAction
	pttDown  bool
	waiters  map[dbus.ObjectPath]chan map[string]dbus.Variant
	bindings chan []portalBinding
	done     chan struct{}
}

// styleSlug keeps lowercase letters, digits, and '-' of a style id.
func styleSlug(id string) string {
	var b strings.Builder
	for _, c := range strings.ToLower(id) {
		if c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-' {
			b.WriteRune(c)
		} else {
			b.WriteByte('-')
		}
	}
	return b.String()
}

// portalBindingFor names a chord for the portal, as the Rust version does,
// so the user's choices carry over.
func portalBindingFor(h hotkey, action x11ChordAction) portalBinding {
	b := portalBinding{trigger: portalTrigger(h), action: action}
	switch {
	case action.ptt:
		b.id, b.description = "push-to-talk", "Record while you hold the keys"
	case action.event.Kind == HotkeyToggle:
		b.id, b.description = "toggle", "Start or stop a dictation"
	case action.event.Kind == HotkeyPasteLast:
		b.id, b.description = "paste-last", "Paste the last dictation"
	case action.event.Kind == HotkeyCycleStyle:
		b.id, b.description = "cycle-style", "Switch to the next style"
	case action.event.Kind == HotkeyToggleIncognito:
		b.id, b.description = "incognito", "Turn incognito on or off"
	case action.event.Kind == HotkeyStyle:
		b.id = "style-" + styleSlug(action.event.StyleID)
		b.description = "Start one dictation in the " + action.event.StyleID + " style"
	}
	return b
}

// startPortalShortcuts connects and checks that the portal has the
// GlobalShortcuts interface.
func startPortalShortcuts(emit func(HotkeyEvent)) (*portalShortcutsClient, error) {
	conn, err := dbus.ConnectSessionBus()
	if err != nil {
		return nil, fmt.Errorf("portal: no session bus: %w", err)
	}
	if _, err := conn.Object(portalDest, portalPath).GetProperty(portalShortcuts + ".version"); err != nil {
		conn.Close()
		return nil, fmt.Errorf("portal: GlobalShortcuts is not available: %w", err)
	}
	p := &portalShortcutsClient{
		conn:     conn,
		emit:     emit,
		signals:  make(chan *dbus.Signal, 64),
		waiters:  map[dbus.ObjectPath]chan map[string]dbus.Variant{},
		bound:    map[string]x11ChordAction{},
		bindings: make(chan []portalBinding, 1),
		done:     make(chan struct{}),
	}
	for _, iface := range []string{portalShortcuts, portalRequest} {
		if err := conn.AddMatchSignal(dbus.WithMatchInterface(iface)); err != nil {
			conn.Close()
			return nil, fmt.Errorf("portal: %w", err)
		}
	}
	conn.Signal(p.signals)
	go p.signalLoop()
	go p.bindLoop()
	return p, nil
}

// bind replaces the bound shortcuts. The result comes later, after the
// portal dialog; failures go to the log.
func (p *portalShortcutsClient) bind(bindings []portalBinding) {
	select {
	case <-p.bindings: // a newer set replaces one not yet sent
	default:
	}
	p.bindings <- bindings
}

func (p *portalShortcutsClient) bindLoop() {
	for {
		select {
		case <-p.done:
			return
		case b := <-p.bindings:
			if err := p.rebind(b); err != nil {
				log.Printf("hotkeys: %v", err)
			}
		}
	}
}

func (p *portalShortcutsClient) rebind(bindings []portalBinding) error {
	p.mu.Lock()
	old := p.session
	p.session = ""
	p.bound = map[string]x11ChordAction{}
	releasePTT := p.pttDown
	p.pttDown = false
	p.mu.Unlock()
	if releasePTT {
		p.emit(HotkeyEvent{Kind: HotkeyPushToTalkUp})
	}
	if old != "" {
		p.conn.Object(portalDest, old).Call(portalSession+".Close", 0)
	}
	if len(bindings) == 0 {
		return nil
	}
	results, err := p.request(portalCallWait, "CreateSession", func(token string) []any {
		return []any{map[string]dbus.Variant{
			"handle_token":         dbus.MakeVariant(token),
			"session_handle_token": dbus.MakeVariant("sayso" + token),
		}}
	})
	if err != nil {
		return fmt.Errorf("portal CreateSession: %w", err)
	}
	var session dbus.ObjectPath
	switch v := results["session_handle"].Value().(type) {
	case string:
		session = dbus.ObjectPath(v)
	case dbus.ObjectPath:
		session = v
	}
	if !session.IsValid() {
		return errors.New("portal CreateSession: no session handle")
	}
	type shortcut struct {
		ID      string
		Options map[string]dbus.Variant
	}
	shortcuts := make([]shortcut, 0, len(bindings))
	actions := map[string]x11ChordAction{}
	for _, b := range bindings {
		shortcuts = append(shortcuts, shortcut{b.id, map[string]dbus.Variant{
			"description":       dbus.MakeVariant(b.description),
			"preferred_trigger": dbus.MakeVariant(b.trigger),
		}})
		actions[b.id] = b.action
	}
	if _, err := p.request(portalBindWait, "BindShortcuts", func(token string) []any {
		return []any{session, shortcuts, "", map[string]dbus.Variant{"handle_token": dbus.MakeVariant(token)}}
	}); err != nil {
		p.conn.Object(portalDest, session).Call(portalSession+".Close", 0)
		return fmt.Errorf("portal BindShortcuts: %w", err)
	}
	p.mu.Lock()
	p.session, p.bound = session, actions
	p.mu.Unlock()
	return nil
}

// request calls a portal method that answers with a Request object, and
// waits for its Response.
func (p *portalShortcutsClient) request(wait time.Duration, method string, args func(token string) []any) (map[string]dbus.Variant, error) {
	var raw [8]byte
	_, _ = rand.Read(raw[:])
	token := "sayso" + hex.EncodeToString(raw[:])
	sender := strings.ReplaceAll(strings.TrimPrefix(p.conn.Names()[0], ":"), ".", "_")
	path := dbus.ObjectPath("/org/freedesktop/portal/desktop/request/" + sender + "/" + token)
	ch := make(chan map[string]dbus.Variant, 1)
	p.mu.Lock()
	p.waiters[path] = ch
	p.mu.Unlock()
	defer func() {
		p.mu.Lock()
		delete(p.waiters, path)
		p.mu.Unlock()
	}()
	var handle dbus.ObjectPath
	if err := p.conn.Object(portalDest, portalPath).Call(portalShortcuts+"."+method, 0, args(token)...).Store(&handle); err != nil {
		return nil, err
	}
	if handle != path {
		// An old portal picks its own path.
		p.mu.Lock()
		p.waiters[handle] = ch
		p.mu.Unlock()
		defer func() {
			p.mu.Lock()
			delete(p.waiters, handle)
			p.mu.Unlock()
		}()
	}
	select {
	case results, ok := <-ch:
		if !ok {
			return nil, errors.New("cancelled or refused")
		}
		return results, nil
	case <-time.After(wait):
		return nil, errors.New("no answer in time")
	case <-p.done:
		return nil, errors.New("closed")
	}
}

func (p *portalShortcutsClient) signalLoop() {
	for {
		var s *dbus.Signal
		select {
		case <-p.done:
			return
		case s = <-p.signals:
		}
		if s == nil {
			return
		}
		switch s.Name {
		case portalRequest + ".Response":
			p.mu.Lock()
			ch := p.waiters[s.Path]
			p.mu.Unlock()
			if ch == nil || len(s.Body) < 2 {
				continue
			}
			code, _ := s.Body[0].(uint32)
			results, _ := s.Body[1].(map[string]dbus.Variant)
			if code != 0 {
				close(ch)
			} else {
				ch <- results
			}
		case portalShortcuts + ".Activated", portalShortcuts + ".Deactivated":
			if len(s.Body) < 2 {
				continue
			}
			session, _ := s.Body[0].(dbus.ObjectPath)
			id, _ := s.Body[1].(string)
			p.shortcut(session, id, s.Name == portalShortcuts+".Activated")
		}
	}
}

func (p *portalShortcutsClient) shortcut(session dbus.ObjectPath, id string, activated bool) {
	p.mu.Lock()
	action, ok := p.bound[id]
	if !ok || session != p.session {
		p.mu.Unlock()
		return
	}
	var ev *HotkeyEvent
	switch {
	case action.ptt && activated && !p.pttDown:
		p.pttDown = true
		ev = &HotkeyEvent{Kind: HotkeyPushToTalkDown}
	case action.ptt && !activated && p.pttDown:
		p.pttDown = false
		ev = &HotkeyEvent{Kind: HotkeyPushToTalkUp}
	case !action.ptt && activated:
		e := action.event
		ev = &e
	}
	p.mu.Unlock()
	if ev != nil {
		p.emit(*ev)
	}
}

func (p *portalShortcutsClient) close() {
	close(p.done)
	p.mu.Lock()
	session := p.session
	p.mu.Unlock()
	if session != "" {
		p.conn.Object(portalDest, session).Call(portalSession+".Close", 0)
	}
	p.conn.RemoveSignal(p.signals)
	p.conn.Close()
}
