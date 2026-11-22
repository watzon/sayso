package platform

// X11 through the pure-Go xgb protocol library: XGrabKey for chords, XTest
// for synthetic keys, and the active window for the context.

import (
	"errors"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/jezek/xgb"
	"github.com/jezek/xgb/xproto"
	"github.com/jezek/xgb/xtest"
)

// X11 modifier masks. Super is Mod4 and NumLock Mod2 on every common layout.
const (
	x11ShiftMask   = xproto.ModMaskShift
	x11LockMask    = xproto.ModMaskLock
	x11ControlMask = xproto.ModMaskControl
	x11Mod1Mask    = xproto.ModMask1 // Alt
	x11Mod2Mask    = xproto.ModMask2 // NumLock
	x11Mod4Mask    = xproto.ModMask4 // Super
)

func x11ModMask(m modifiers) uint16 {
	var mask uint16
	if m.shift {
		mask |= x11ShiftMask
	}
	if m.control {
		mask |= x11ControlMask
	}
	if m.option {
		mask |= x11Mod1Mask
	}
	if m.command {
		mask |= x11Mod4Mask
	}
	return mask
}

// x11ChordAction is what a grabbed chord does.
type x11ChordAction struct {
	event HotkeyEvent
	ptt   bool // push-to-talk: down on press, up on release
}

type x11Keycode = xproto.Keycode

type x11ChordKey struct {
	keycode xproto.Keycode
	mods    uint16
}

// x11Grabs owns one X connection with grabbed chords and its event loop.
type x11Grabs struct {
	conn    *xgb.Conn
	root    xproto.Window
	emit    func(HotkeyEvent)
	mu      sync.Mutex
	actions map[x11ChordKey]x11ChordAction
	down    map[xproto.Keycode]bool
	// pending holds key releases that may be half of an auto-repeat pair:
	// X11 sends release+press with the same time for each repeat.
	pending map[xproto.Keycode]*x11PendingRelease
	done    chan struct{}
}

type x11PendingRelease struct {
	time  xproto.Timestamp
	timer *time.Timer
}

// x11RepeatWindow is how long a release waits for the press of an
// auto-repeat pair.
const x11RepeatWindow = 40 * time.Millisecond

func startX11Grabs(emit func(HotkeyEvent)) (*x11Grabs, error) {
	conn, err := xgb.NewConn()
	if err != nil {
		return nil, fmt.Errorf("X11: %w", err)
	}
	g := &x11Grabs{
		conn:    conn,
		root:    xproto.Setup(conn).DefaultScreen(conn).Root,
		emit:    emit,
		actions: map[x11ChordKey]x11ChordAction{},
		down:    map[xproto.Keycode]bool{},
		pending: map[xproto.Keycode]*x11PendingRelease{},
		done:    make(chan struct{}),
	}
	go g.loop()
	return g, nil
}

// bind replaces the grabbed chords. Chords that another app holds already
// are named in the error; the rest are grabbed.
func (g *x11Grabs) bind(chords map[x11ChordKey]x11ChordAction) error {
	g.mu.Lock()
	defer g.mu.Unlock()
	for key := range g.actions {
		for _, extra := range []uint16{0, x11LockMask, x11Mod2Mask, x11LockMask | x11Mod2Mask} {
			xproto.UngrabKey(g.conn, key.keycode, g.root, key.mods|extra)
		}
	}
	g.actions = map[x11ChordKey]x11ChordAction{}
	var taken []string
	for key, action := range chords {
		ok := true
		// Grab with and without Caps Lock and Num Lock, which X11 counts as modifiers.
		for _, extra := range []uint16{0, x11LockMask, x11Mod2Mask, x11LockMask | x11Mod2Mask} {
			err := xproto.GrabKeyChecked(g.conn, true, g.root, key.mods|extra, key.keycode,
				xproto.GrabModeAsync, xproto.GrabModeAsync).Check()
			if err != nil {
				ok = false
			}
		}
		if ok {
			g.actions[key] = action
		} else {
			for _, extra := range []uint16{0, x11LockMask, x11Mod2Mask, x11LockMask | x11Mod2Mask} {
				xproto.UngrabKey(g.conn, key.keycode, g.root, key.mods|extra)
			}
			taken = append(taken, fmt.Sprintf("keycode %d mods %#x", key.keycode, key.mods))
		}
	}
	if len(taken) > 0 {
		return fmt.Errorf("X11: another app holds %s", strings.Join(taken, ", "))
	}
	return nil
}

func (g *x11Grabs) loop() {
	defer close(g.done)
	for {
		ev, err := g.conn.WaitForEvent()
		if ev == nil && err == nil {
			return // connection closed
		}
		switch e := ev.(type) {
		case xproto.KeyPressEvent:
			g.press(e.Detail, e.State, e.Time)
		case xproto.KeyReleaseEvent:
			g.release(e.Detail, e.State, e.Time)
		}
	}
}

func (g *x11Grabs) lookup(code xproto.Keycode, state uint16) (x11ChordAction, bool) {
	mods := state & (x11ShiftMask | x11ControlMask | x11Mod1Mask | x11Mod4Mask)
	a, ok := g.actions[x11ChordKey{code, mods}]
	return a, ok
}

func (g *x11Grabs) press(code xproto.Keycode, state uint16, t xproto.Timestamp) {
	g.mu.Lock()
	if p := g.pending[code]; p != nil && p.time == t {
		// The second half of an auto-repeat pair: the key is still down.
		p.timer.Stop()
		delete(g.pending, code)
		g.mu.Unlock()
		return
	}
	a, ok := g.lookup(code, state)
	if !ok || g.down[code] {
		g.mu.Unlock()
		return
	}
	g.down[code] = true
	g.mu.Unlock()
	if a.ptt {
		g.emit(HotkeyEvent{Kind: HotkeyPushToTalkDown})
	} else {
		g.emit(a.event)
	}
}

func (g *x11Grabs) release(code xproto.Keycode, state uint16, t xproto.Timestamp) {
	g.mu.Lock()
	defer g.mu.Unlock()
	if !g.down[code] {
		return
	}
	// Push-to-talk is the only action on release. The modifiers may already
	// be up, so the action is found by key code alone.
	var ptt bool
	for key, a := range g.actions {
		if key.keycode == code && a.ptt {
			ptt = true
		}
	}
	if p := g.pending[code]; p != nil {
		p.timer.Stop()
	}
	p := &x11PendingRelease{time: t}
	p.timer = time.AfterFunc(x11RepeatWindow, func() {
		g.mu.Lock()
		if g.pending[code] != p {
			g.mu.Unlock()
			return
		}
		delete(g.pending, code)
		g.down[code] = false
		g.mu.Unlock()
		if ptt {
			g.emit(HotkeyEvent{Kind: HotkeyPushToTalkUp})
		}
	})
	g.pending[code] = p
}

func (g *x11Grabs) close() {
	g.conn.Close()
	<-g.done
}

// ---------------------------------------------------------------------------
// XTest keys
// ---------------------------------------------------------------------------

// x11PressKeys presses the evdev codes in order and releases them in reverse,
// through XTest. Modifiers that the user still holds are not released first.
func x11PressKeys(codes ...uint16) error {
	conn, err := xgb.NewConn()
	if err != nil {
		return err
	}
	defer conn.Close()
	if err := xtest.Init(conn); err != nil {
		return fmt.Errorf("XTest: %w", err)
	}
	root := xproto.Setup(conn).DefaultScreen(conn).Root
	fake := func(typ byte, code uint16) error {
		return xtest.FakeInputChecked(conn, typ, byte(code+x11KeycodeOffset), 0, root, 0, 0, 0).Check()
	}
	for _, c := range codes {
		if err := fake(xproto.KeyPress, c); err != nil {
			return err
		}
	}
	time.Sleep(8 * time.Millisecond)
	for i := len(codes) - 1; i >= 0; i-- {
		if err := fake(xproto.KeyRelease, codes[i]); err != nil {
			return err
		}
	}
	// A round trip makes sure that the server got the events before the connection closes.
	_, err = xproto.GetInputFocus(conn).Reply()
	return err
}

// ---------------------------------------------------------------------------
// Active window
// ---------------------------------------------------------------------------

// x11ActiveWindowClass is the WM_CLASS class and the PID of the active window.
func x11ActiveWindowClass() (class string, pid int, err error) {
	conn, err := xgb.NewConn()
	if err != nil {
		return "", 0, err
	}
	defer conn.Close()
	root := xproto.Setup(conn).DefaultScreen(conn).Root
	atom := func(name string) xproto.Atom {
		r, err := xproto.InternAtom(conn, true, uint16(len(name)), name).Reply()
		if err != nil || r == nil {
			return xproto.AtomNone
		}
		return r.Atom
	}
	active := atom("_NET_ACTIVE_WINDOW")
	if active == xproto.AtomNone {
		return "", 0, errors.New("X11: the window manager does not report the active window")
	}
	r, err := xproto.GetProperty(conn, false, root, active, xproto.AtomWindow, 0, 1).Reply()
	if err != nil || r == nil || len(r.Value) < 4 {
		return "", 0, errors.New("X11: no active window")
	}
	win := xproto.Window(xgb.Get32(r.Value))
	if win == 0 {
		return "", 0, errors.New("X11: no active window")
	}
	if c, err := xproto.GetProperty(conn, false, win, xproto.AtomWmClass, xproto.AtomString, 0, 256).Reply(); err == nil && c != nil {
		class = parseWMClass(c.Value)
	}
	if a := atom("_NET_WM_PID"); a != xproto.AtomNone {
		if p, err := xproto.GetProperty(conn, false, win, a, xproto.AtomCardinal, 0, 1).Reply(); err == nil && p != nil && len(p.Value) >= 4 {
			pid = int(xgb.Get32(p.Value))
		}
	}
	return class, pid, nil
}

// parseWMClass reads "instance\x00class\x00". The class names the app.
func parseWMClass(value []byte) string {
	var parts []string
	for _, p := range strings.Split(string(value), "\x00") {
		if p != "" {
			parts = append(parts, p)
		}
	}
	switch len(parts) {
	case 0:
		return ""
	case 1:
		return parts[0]
	}
	return parts[1]
}
